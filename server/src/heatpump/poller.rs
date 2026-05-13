//! Heat pump status polling loop
//!
//! Reads the latest `HEATPUMP_STATUS` and `CTC_OUTDOOR_TEMP` values from the
//! shared sensor cache (filled by `storage::poller`) and feeds them into the
//! `HeatPumpStats` tracker. We deliberately do not issue our own Modbus reads
//! here: the sensor-cache poller is the single Modbus reader for these two
//! registers, which avoids actor-mutex contention and duplicated round-trips.
//!
//! Resolution is therefore bounded by the sensor cache's tick (5 s by default),
//! not this loop's `poll_interval_secs`. Setting `poll_interval_secs` faster
//! than the cache tick just re-samples the same values.

use std::time::Duration;

use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace};

use crate::heatpump::stats::HeatPumpStats;
use crate::storage::{Sensor, Store};

/// Run the heat pump status polling loop.
///
/// On each tick, reads the latest `HpStatus` and `Outdoor` values from the
/// sensor cache and forwards them to `stats.update_state`. The cache may
/// still be empty for a few seconds after startup; in that case we skip
/// the update with a debug log.
///
/// # Arguments
/// * `store` - Sensor cache (single source of truth for `HEATPUMP_STATUS` and
///   `CTC_OUTDOOR_TEMP`)
/// * `stats` - Heat pump statistics tracker
/// * `poll_interval_secs` - How often to consult the cache
pub async fn run_poll_loop(
    store: Store,
    stats: HeatPumpStats,
    poll_interval_secs: u64,
    cancel: CancellationToken,
) {
    let mut ticker = interval(Duration::from_secs(poll_interval_secs));
    // Match the sensor-cache poller — a backlog of ticks after a stall isn't
    // useful when we're just re-reading the same cached values.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    info!(
        "Heat pump status polling started (interval: {}s, source: sensor cache)",
        poll_interval_secs
    );

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                info!("Heat pump poller: shutdown signal received");
                return;
            }
            _ = ticker.tick() => {}
        }

        let hp_status = store.latest_sample(Sensor::HpStatus).map(|(_, v)| v);
        let outdoor_temp = store.latest_sample(Sensor::Outdoor).map(|(_, v)| v);

        if let Some(hp_status_value) = hp_status {
            // Status is returned as f32 but is actually an integer code
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let status_code = hp_status_value as u16;

            trace!(
                "Heat pump poll: status={}, outdoor_temp={:?}",
                status_code, outdoor_temp
            );

            stats.update_state(status_code, outdoor_temp);
        } else {
            // Invalidate the in-progress cycle so the outage period isn't
            // counted as operating time. The next successful poll will
            // re-sync state cleanly.
            debug!("Failed to read heat pump status — marking state unknown");
            stats.mark_poll_failed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[tokio::test]
    async fn poller_skips_when_cache_empty() {
        // Fresh store has no samples — the loop's empty-cache branch invokes
        // mark_poll_failed silently. Asserting the loop respects the cancel
        // token and exits in bounded time is the contract we care about.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();
        let stats =
            HeatPumpStats::new_with_store_and_tz(store.clone(), chrono_tz::Europe::Stockholm);

        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run_poll_loop(store, stats, 1, cancel.clone()));

        // tokio::time::interval fires immediately on first tick, so one body
        // iteration is guaranteed by the time we cancel.
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("poller should stop within 1s of cancel")
            .expect("task should not panic");
    }

    #[tokio::test]
    async fn poller_updates_stats_from_cache() {
        // Seed the cache with HpStatus=1 (Ready, OFF). The poll loop should
        // forward that to stats and initialize the tracker, observable via
        // get_summary().
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();
        let stats =
            HeatPumpStats::new_with_store_and_tz(store.clone(), chrono_tz::Europe::Stockholm);

        store
            .record_sample(Sensor::HpStatus, SystemTime::now(), 1.0)
            .expect("seed HpStatus");

        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run_poll_loop(
            store.clone(),
            stats.clone(),
            1,
            cancel.clone(),
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("poller should stop within 1s of cancel")
            .expect("task should not panic");

        // HpStatus=1 (Ready) → compressor off. The poll initialized stats
        // without recording a start (initialization sync, not OFF→ON).
        let summary = stats.get_summary();
        assert!(!summary.compressor_on);
        assert_eq!(summary.starts.this_hour, 0);
    }
}
