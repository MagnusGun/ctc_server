//! Step-response recorder.
//!
//! Watches the flow-temperature sensor for sudden step changes (≥ a configured
//! magnitude over the per-tick poll), then captures the return-temperature
//! response curve over the following observation window. Each completed event
//! is persisted via the redb [`Store`] for the dashboard's step-response
//! chart.
//!
//! Reads all sensor values from the in-memory sensor cache (no Modbus calls).
//! Resolution is therefore bounded by the cache tick (5 s default).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace};

use crate::storage::{Sensor, StepEventBlob, Store};

/// Minimum flow-temperature delta (°C) between consecutive ticks that counts
/// as a step. Below this, normal heat-curve drift is ignored.
const STEP_THRESHOLD_C: f32 = 1.0;

/// How long after a step we keep collecting samples (seconds). 30 min is the
/// envelope the dashboard chart was designed against.
const OBSERVE_SECS: u32 = 1800;

/// Close the event early once the return temperature has traversed at least
/// this fraction of the flow step. (0.9 → "90 % settled".)
const SETTLE_FRACTION: f32 = 0.9;

#[derive(Debug)]
struct ActiveEvent {
    started_at_unix: u64,
    flow_before: f32,
    flow_after: f32,
    return_before: f32,
    samples: Vec<(u32, f32, f32)>,
}

/// Run the step-response recorder loop.
///
/// # Arguments
/// * `store` - sensor cache + persistence layer
/// * `poll_interval_secs` - how often to consult the cache. Matching the
///   sensor poller cadence (5 s) is fine; the cache won't advance faster.
pub async fn run_recorder_loop(store: Store, poll_interval_secs: u64, cancel: CancellationToken) {
    let mut ticker = interval(Duration::from_secs(poll_interval_secs));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    info!(
        "Step-response recorder started (interval: {}s, threshold: {}°C, window: {}s)",
        poll_interval_secs, STEP_THRESHOLD_C, OBSERVE_SECS
    );

    let mut baseline_flow: Option<f32> = None;
    let mut active: Option<ActiveEvent> = None;

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                info!("Step-response recorder: shutdown signal received");
                return;
            }
            _ = ticker.tick() => {}
        }

        let Some((_, flow)) = store.latest_sample(Sensor::Flow) else {
            continue;
        };
        let Some((_, ret)) = store.latest_sample(Sensor::Return) else {
            continue;
        };
        if !flow.is_finite() || !ret.is_finite() {
            continue;
        }

        if let Some(ev) = active.as_mut() {
            let elapsed = elapsed_since(ev.started_at_unix);
            if elapsed == u32::MAX {
                // Clock stepped backward past the event start. Drop the
                // in-progress cycle rather than persist a bogus 4.2-billion-
                // second event; resume baselining from the current sample.
                info!("Step event: clock anomaly detected, discarding in-progress cycle");
                baseline_flow = Some(flow);
                active = None;
                continue;
            }
            ev.samples.push((elapsed, flow, ret));
            // Close on settle or window expiry.
            let span = ev.flow_after - ev.return_before;
            let observed = ret - ev.return_before;
            let settled = span != 0.0 && (observed / span) >= SETTLE_FRACTION;
            // span == 0 means flow_after equals the return baseline — there's
            // nothing to settle, so close immediately rather than waiting the
            // full OBSERVE_SECS window for a step that won't develop.
            let no_step = span == 0.0;
            if settled || no_step || elapsed >= OBSERVE_SECS {
                let blob = StepEventBlob {
                    started_at: ev.started_at_unix,
                    flow_before: ev.flow_before,
                    flow_after: ev.flow_after,
                    return_before: ev.return_before,
                    samples: std::mem::take(&mut ev.samples),
                };
                debug!(
                    "Step event closed at {}s ({} samples, settled={})",
                    elapsed,
                    blob.samples.len(),
                    settled
                );
                store.record_step_event(blob);
                baseline_flow = Some(flow);
                active = None;
            }
            continue;
        }

        // No active event — watch for a new step.
        if let Some(base) = baseline_flow {
            if (flow - base).abs() >= STEP_THRESHOLD_C {
                info!("Step detected: flow {:.2} -> {:.2}", base, flow);
                active = Some(ActiveEvent {
                    started_at_unix: unix_now(),
                    flow_before: base,
                    flow_after: flow,
                    return_before: ret,
                    samples: vec![(0, flow, ret)],
                });
                continue;
            }
            trace!("Flow stable: {:.2}", flow);
        }
        // Update baseline only when not in an event. Avoids drifting the
        // baseline mid-observation if a second step starts.
        baseline_flow = Some(flow);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Seconds elapsed since `start_unix`. Returns [`u32::MAX`] as a sentinel when
/// the system clock has stepped backward (NTP correction, manual adjustment)
/// past the event start. The previous `saturating_sub` masked backsteps as 0,
/// which kept `elapsed >= OBSERVE_SECS` permanently false and froze the
/// recorder in the in-progress cycle.
fn elapsed_since(start_unix: u64) -> u32 {
    let now = unix_now();
    if start_unix > now {
        return u32::MAX;
    }
    u32::try_from(now - start_unix).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn elapsed_since_returns_sentinel_when_clock_steps_back() {
        let now = unix_now();
        // Start "1 hour from now" — i.e. start_unix > now.
        let future_start = now + 3_600;
        assert_eq!(elapsed_since(future_start), u32::MAX);
    }

    #[test]
    fn elapsed_since_returns_zero_at_start() {
        let start = unix_now();
        // The current second hasn't elapsed yet, so 0 or 1 is acceptable
        // depending on tick timing. Definitely not the sentinel.
        let v = elapsed_since(start);
        assert!(v <= 1, "expected elapsed≈0, got {v}");
    }

    #[tokio::test]
    async fn detects_step_and_persists_event() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();

        // Pre-fill the cache so the first tick has both flow and return.
        let t0 = SystemTime::now();
        store.record_sample(Sensor::Flow, t0, 22.0).unwrap();
        store.record_sample(Sensor::Return, t0, 20.0).unwrap();

        let store_clone = store.clone();
        let handle = tokio::spawn(async move {
            run_recorder_loop(store_clone, 1, CancellationToken::new()).await;
        });

        // Let one baseline tick happen.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Inject a step in flow and let the recorder pick it up.
        store
            .record_sample(Sensor::Flow, SystemTime::now(), 26.0)
            .unwrap();
        store
            .record_sample(Sensor::Return, SystemTime::now(), 20.0)
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // Drive return up past the 90% settle threshold (span = 26 - 20 = 6,
        // so the recorder closes when return >= 20 + 0.9 * 6 = 25.4).
        store
            .record_sample(Sensor::Return, SystemTime::now(), 26.0)
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;

        handle.abort();
        let _ = handle.await;

        let events = store.recent_step_events(10);
        assert!(!events.is_empty(), "expected at least one step event");
        let ev = &events[0];
        assert!((ev.flow_before - 22.0).abs() < 0.5);
        assert!((ev.flow_after - 26.0).abs() < 0.5);
    }
}
