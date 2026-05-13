//! Reconciliation poller: periodically reads the pump's actual state from
//! Homey, compares it to the desired state published by the
//! [`SmartGrid`](crate::smartgrid) actor, and pushes a corrective
//! `set_pump_onoff(desired)` if they diverge.
//!
//! Drift sources this catches: Homey restart resets the Zigbee plug, the
//! user toggles the plug from the Homey app, or a previous push failed.
//!
//! The actor publishes desired state via a `tokio::sync::watch::Sender`; this
//! poller subscribes via the corresponding `Receiver` so it always reads the
//! latest desired value even if the mode flipped between two polls.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::HomeyClient;
use super::cache::HomeyPumpCache;

/// Reconciliation loop. Sleep `period`, then read Homey and reconcile.
///
/// Designed to be wrapped by [`supervisor::spawn_with_shutdown`] like the
/// other long-running tasks in `main.rs`, so panics are caught and a
/// cancellation token cleans up reliably.
pub async fn run(
    client: HomeyClient,
    cache: Arc<HomeyPumpCache>,
    desired_rx: watch::Receiver<bool>,
    period: Duration,
    cancel: CancellationToken,
) {
    let mut ticker = interval(period);
    // A backlog of missed ticks after a stall isn't useful — reconciliation
    // is idempotent and we'd rather wait for the next clean tick than burst.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // First tick fires immediately; consume it so the first reconcile waits
    // one full period (lets the actor's startup-seed push land first).
    ticker.tick().await;

    info!(
        "Homey pump poller started (interval = {} s)",
        period.as_secs()
    );

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                info!("Homey pump poller: shutdown signal received");
                return;
            }
            _ = ticker.tick() => {}
        }
        tick(&client, &cache, &desired_rx).await;
    }
}

async fn tick(client: &HomeyClient, cache: &HomeyPumpCache, desired_rx: &watch::Receiver<bool>) {
    let desired = *desired_rx.borrow();
    match client.get_pump_onoff().await {
        Ok(actual) => {
            cache.write_fresh(actual).await;
            if actual != desired {
                info!("Pump drift detected (actual={actual}, desired={desired}) — reconciling");
                if let Err(e) = client.set_pump_onoff(desired).await {
                    warn!("Reconcile push failed: {e}");
                } else {
                    cache.write_fresh(desired).await;
                }
            }
        }
        Err(e) => {
            warn!("Homey poll failed: {e} — cache marked stale");
            cache.mark_stale().await;
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests exercise `tick` directly so we don't have to wrestle with
    //! tokio sleep timing. The full `run` loop is exercised end-to-end by
    //! the smoke-test script in the plan.

    use super::super::test_support::{MockState, SharedMock, make_client, spawn_mock};
    use super::*;
    use std::sync::Mutex;

    #[tokio::test(flavor = "current_thread")]
    async fn tick_reconciles_on_mismatch() {
        // Actual=true on the plug, desired=false → poller should push false.
        let state: SharedMock = Arc::new(Mutex::new(MockState {
            pump_on: true,
            ..MockState::default()
        }));
        let addr = spawn_mock(state.clone()).await;
        let client = make_client(addr);
        let cache = Arc::new(HomeyPumpCache::new());
        let (_tx, rx) = watch::channel(false);

        tick(&client, &cache, &rx).await;

        {
            let s = state.lock().unwrap();
            assert_eq!(
                s.set_calls,
                vec![false],
                "poller should push the desired value once on mismatch"
            );
        }
        // After reconcile, cache reflects desired.
        let snap = cache.read().await;
        assert_eq!(snap.actual, Some(false));
        assert!(!snap.stale);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_no_push_when_match() {
        // Actual=true on plug, desired=true → poller should only read.
        let state: SharedMock = Arc::new(Mutex::new(MockState {
            pump_on: true,
            ..MockState::default()
        }));
        let addr = spawn_mock(state.clone()).await;
        let client = make_client(addr);
        let cache = Arc::new(HomeyPumpCache::new());
        let (_tx, rx) = watch::channel(true);

        tick(&client, &cache, &rx).await;

        {
            let s = state.lock().unwrap();
            assert!(s.set_calls.is_empty(), "no push expected on match");
            assert_eq!(s.get_calls, 1, "exactly one GET");
        }
        let snap = cache.read().await;
        assert_eq!(snap.actual, Some(true));
        assert!(!snap.stale);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_marks_stale_on_get_failure() {
        let state: SharedMock = Arc::new(Mutex::new(MockState {
            pump_on: true,
            get_returns_error: true,
            ..MockState::default()
        }));
        let addr = spawn_mock(state.clone()).await;
        let client = make_client(addr);
        let cache = Arc::new(HomeyPumpCache::new());
        // Pre-seed with a fresh value so we can verify mark_stale preserves
        // `actual`.
        cache.write_fresh(true).await;
        let (_tx, rx) = watch::channel(true);

        tick(&client, &cache, &rx).await;

        {
            let s = state.lock().unwrap();
            assert!(s.set_calls.is_empty(), "no push attempted when GET fails");
        }
        let snap = cache.read().await;
        assert!(snap.stale, "cache should be marked stale");
        assert_eq!(snap.actual, Some(true), "actual preserved across stale");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_observes_latest_desired_via_watch() {
        // Mutate the watch channel between two ticks and verify the second
        // tick reconciles to the new desired.
        let state: SharedMock = Arc::new(Mutex::new(MockState {
            pump_on: true,
            ..MockState::default()
        }));
        let addr = spawn_mock(state.clone()).await;
        let client = make_client(addr);
        let cache = Arc::new(HomeyPumpCache::new());
        let (tx, rx) = watch::channel(true);

        tick(&client, &cache, &rx).await; // desired=true, actual=true → no push
        {
            let s = state.lock().unwrap();
            assert!(s.set_calls.is_empty());
        }

        tx.send(false).unwrap();
        tick(&client, &cache, &rx).await; // desired=false, actual still true → push

        let s = state.lock().unwrap();
        assert_eq!(s.set_calls, vec![false]);
    }
}
