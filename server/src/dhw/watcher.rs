//! DHW boost watcher tasks.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};

use crate::dhw::actor::ModbusWriter;
use crate::dhw::error::CancelReason;

/// One-shot watcher for the Shower preset: sleep `duration`, clear the boost
/// override, notify the actor that the Shower is complete. The heater's own
/// `61503` timer expires in parallel — we deliberately do not write `0` to
/// `61503` on natural completion (flash-frugal).
pub async fn run_shower_watcher(
    boost_override_tx: watch::Sender<Option<bool>>,
    duration: Duration,
    done_tx: oneshot::Sender<()>,
) {
    tokio::time::sleep(duration).await;
    let _ = boost_override_tx.send(None);
    let _ = done_tx.send(());
}

/// Side-channel events the Bath watcher emits to the actor (besides
/// `notify_done`). Currently only carries immersion-gate transitions so the
/// actor can keep its in-memory `DhwBoostState.immersion_engaged` and the
/// on-disk persisted state in sync without sharing a mutex with the watcher.
///
/// This is the "option (c)" lane in the Task-12 design: the watcher owns its
/// own gate state and broadcasts crossings back rather than touching shared
/// state. Far fewer test churn than wrapping `self.state` in an `Arc<Mutex<_>>`
/// (option b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BathWatcherEvent {
    /// Immersion-gate crossed; payload is the new `engaged` flag.
    ImmersionEngaged(bool),
}

/// 60s-tick watcher for the Bath preset.
///
/// Each tick (re-)evaluates the three stop triggers and the immersion gate.
/// The watcher does **not** own the `boost_override_tx` watch channel or the
/// `SgController` — those are touched by the actor's stop sequence (Task 13)
/// once `notify_done` fires. The watcher's only side effect is writing
/// `61591` when the immersion gate crosses.
///
/// Stop triggers (checked in this order each tick):
///   1. `started_at.elapsed() >= duration` → `CancelReason::TimerExpired`.
///   2. Cached `Sensor::Room` < `cfg.boost_room_temp_bail_c` → `RoomTooCold`.
///   3. Current `PriceLevel` ∉ `{VeryCheap, Cheap}` → `PriceLeftCheap`.
///
/// On any trigger the watcher sends the reason over `notify_done` and exits.
/// Manual cancel comes via `DhwCmd::Cancel` and is handled by the actor.
///
/// Time:
///   * Tick interval is exactly 60s.
///   * `MissedTickBehavior::Delay` so a stalled scheduler doesn't burst.
///   * The first immediate tick is consumed up front so the first evaluation
///     happens at `+60s`, not `t=0`. This matters because the actor seeds
///     `immersion_engaged` once on entry; we shouldn't re-decide on the same
///     spot price at `t=0`.
///
/// Time-source: `tokio::time::Instant` so `start_paused = true` tests can
/// drive timer expiry via `tokio::time::advance(...)`.
///
/// Argument count exceeds clippy's default of 7 because every dependency is
/// genuinely distinct (no natural grouping that would survive Task 13's
/// stop-sequence rework). Allowing this rather than inventing a synthetic
/// builder struct just for the call site.
#[allow(clippy::too_many_arguments)]
pub async fn run_bath_watcher(
    duration: Duration,
    started_at: tokio::time::Instant,
    initial_immersion_engaged: bool,
    modbus: Arc<dyn ModbusWriter>,
    store: crate::storage::Store,
    price: Arc<crate::energy::price::PriceState>,
    cfg: crate::config::DhwConfig,
    event_tx: mpsc::Sender<BathWatcherEvent>,
    notify_done: oneshot::Sender<CancelReason>,
) {
    let mut tick = tokio::time::interval(Duration::from_mins(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Consume the immediate first tick so the first eval lands at +60s.
    let _ = tick.tick().await;

    // Restore gate state on watcher start (defensive — covers a recovery path
    // that respawns the watcher with a previously engaged gate).
    let mut gate = crate::dhw::immersion::ImmersionGate::with_engaged(
        cfg.immersion_allow_price_sek_per_kwh,
        cfg.immersion_hysteresis_sek_per_kwh,
        initial_immersion_engaged,
    );

    loop {
        tick.tick().await;

        // 1. Timer expiry — cheapest check, most likely cause.
        if started_at.elapsed() >= duration {
            let _ = notify_done.send(CancelReason::TimerExpired);
            return;
        }

        // 2. Room-temp bail (cached sample; no fresh Modbus read).
        if let Some((_, room)) = store.latest_sample(crate::storage::Sensor::Room)
            && room < cfg.boost_room_temp_bail_c
        {
            let _ = notify_done.send(CancelReason::RoomTooCold);
            return;
        }

        // 3. Price-band bail.
        let snap = price.get_current();
        let level = snap.as_ref().and_then(|p| p.level);
        // f64 spot_sek -> f32 for the immersion gate; SEK/kWh resolution is
        // well within f32 precision.
        #[allow(clippy::cast_possible_truncation)]
        let spot_f32 = snap.as_ref().map(|p| p.spot_sek as f32);
        let cheap = matches!(
            level,
            Some(
                crate::energy::price::PriceLevel::VeryCheap
                    | crate::energy::price::PriceLevel::Cheap
            )
        );
        if !cheap {
            let _ = notify_done.send(CancelReason::PriceLeftCheap);
            return;
        }

        // 4. Immersion gate re-evaluation (only when we have a spot price;
        //    `cheap` is true here so `snap` is Some, but guard explicitly).
        if let Some(spot) = spot_f32 {
            match gate.evaluate(spot, cheap) {
                crate::dhw::immersion::ImmersionDecision::Engage => {
                    match modbus
                        .write_scaled(61591, cfg.immersion_kw_when_allowed)
                        .await
                    {
                        Ok(()) => {
                            let _ = event_tx
                                .send(BathWatcherEvent::ImmersionEngaged(true))
                                .await;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "DHW immersion Engage write failed: {e}; reverting gate for retry"
                            );
                            gate.revert();
                        }
                    }
                }
                crate::dhw::immersion::ImmersionDecision::Disengage => {
                    match modbus.write_scaled(61591, 0.0).await {
                        Ok(()) => {
                            let _ = event_tx
                                .send(BathWatcherEvent::ImmersionEngaged(false))
                                .await;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "DHW immersion Disengage write failed: {e}; reverting gate for retry"
                            );
                            gate.revert();
                        }
                    }
                }
                crate::dhw::immersion::ImmersionDecision::NoChange => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test(start_paused = true)]
    async fn shower_watcher_fires_pump_restore_after_duration() {
        let (boost_tx, _boost_rx) = watch::channel(Some(false));
        let (done_tx, done_rx) = oneshot::channel();
        let handle = tokio::spawn(run_shower_watcher(
            boost_tx.clone(),
            Duration::from_mins(30),
            done_tx,
        ));
        tokio::time::advance(Duration::from_secs(1801)).await;
        done_rx.await.unwrap();
        assert_eq!(*boost_tx.subscribe().borrow(), None);
        handle.abort();
    }

    /// Shared write-log type used by the fake modbus + assertions.
    type WriteLog = Arc<Mutex<Vec<(u16, f32)>>>;

    /// Minimal modbus fake for watcher tests. Records `(addr, value)` writes
    /// in order so tests can assert immersion engages/disengages produced the
    /// expected `61591` traffic.
    struct FakeModbus {
        calls: WriteLog,
    }

    impl FakeModbus {
        fn new() -> (Self, WriteLog) {
            let calls: WriteLog = Arc::default();
            (
                Self {
                    calls: calls.clone(),
                },
                calls,
            )
        }
    }

    #[async_trait::async_trait]
    impl ModbusWriter for FakeModbus {
        async fn write_scaled(&self, addr: u16, v: f32) -> Result<(), String> {
            self.calls.lock().unwrap().push((addr, v));
            Ok(())
        }
        async fn read_scaled(&self, _addr: u16) -> Result<f32, String> {
            Ok(0.0)
        }
    }

    fn make_store() -> (tempfile::TempDir, crate::storage::Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = crate::storage::Store::open(dir.path().join("ctc.redb")).expect("open store");
        (dir, store)
    }

    /// Seed the in-memory price cache with a single slot covering "now".
    fn price_with_current(
        level: crate::energy::price::PriceLevel,
        spot_sek: f64,
    ) -> Arc<crate::energy::price::PriceState> {
        let now = chrono::Utc::now();
        let start = now - chrono::Duration::minutes(1);
        let end = now + chrono::Duration::minutes(14);
        let mut point = crate::energy::price::PricePoint::from_spot(
            start.to_rfc3339(),
            end.to_rfc3339(),
            spot_sek,
            0.0,
            0.0,
        );
        point.level = Some(level);
        let s = crate::energy::price::PriceState::new("SE3".to_string());
        s.update_prices(vec![point], vec![]);
        Arc::new(s)
    }

    fn assert_float_eq(a: f32, b: f32, msg: &str) {
        assert!(
            (a - b).abs() < 1e-4,
            "{msg}: expected {b}, got {a} (delta {})",
            (a - b).abs()
        );
    }

    /// Update the price cache mid-test to flip the level / spot.
    fn replace_price(
        state: &crate::energy::price::PriceState,
        level: Option<crate::energy::price::PriceLevel>,
        spot_sek: f64,
    ) {
        let now = chrono::Utc::now();
        let start = now - chrono::Duration::minutes(1);
        let end = now + chrono::Duration::minutes(14);
        let mut point = crate::energy::price::PricePoint::from_spot(
            start.to_rfc3339(),
            end.to_rfc3339(),
            spot_sek,
            0.0,
            0.0,
        );
        point.level = level;
        state.update_prices(vec![point], vec![]);
    }

    #[tokio::test(start_paused = true)]
    async fn bath_watcher_emits_timer_expired_at_duration() {
        let (modbus_raw, _calls) = FakeModbus::new();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(modbus_raw);
        let (_dir, store) = make_store();
        // Room temp safely above bail threshold so it doesn't pre-empt the
        // timer trigger.
        store
            .record_sample(
                crate::storage::Sensor::Room,
                std::time::SystemTime::now(),
                20.0,
            )
            .unwrap();
        let price = price_with_current(crate::energy::price::PriceLevel::VeryCheap, 0.30);
        let cfg = crate::config::DhwConfig::default();
        let (ev_tx, _ev_rx) = mpsc::channel::<BathWatcherEvent>(8);
        let (done_tx, done_rx) = oneshot::channel();
        let started_at = tokio::time::Instant::now();

        let handle = tokio::spawn(run_bath_watcher(
            Duration::from_mins(2), // 2 minutes
            started_at,
            false,
            modbus,
            store,
            price,
            cfg,
            ev_tx,
            done_tx,
        ));
        // Advance past 2 minutes; two 60s ticks fire and on tick #2 elapsed
        // hits 120s.
        tokio::time::advance(Duration::from_secs(130)).await;
        let reason = done_rx.await.unwrap();
        assert_eq!(reason, CancelReason::TimerExpired);
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn bath_watcher_emits_room_too_cold_when_room_drops() {
        let (modbus_raw, _calls) = FakeModbus::new();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(modbus_raw);
        let (_dir, store) = make_store();
        // Room 5 °C below bail threshold (default 17 °C).
        store
            .record_sample(
                crate::storage::Sensor::Room,
                std::time::SystemTime::now(),
                12.0,
            )
            .unwrap();
        let price = price_with_current(crate::energy::price::PriceLevel::VeryCheap, 0.30);
        let cfg = crate::config::DhwConfig::default();
        let (ev_tx, _ev_rx) = mpsc::channel::<BathWatcherEvent>(8);
        let (done_tx, done_rx) = oneshot::channel();
        let started_at = tokio::time::Instant::now();

        let handle = tokio::spawn(run_bath_watcher(
            Duration::from_hours(1), // 1h — well beyond first tick
            started_at,
            false,
            modbus,
            store,
            price,
            cfg,
            ev_tx,
            done_tx,
        ));
        tokio::time::advance(Duration::from_secs(70)).await;
        let reason = done_rx.await.unwrap();
        assert_eq!(reason, CancelReason::RoomTooCold);
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn bath_watcher_emits_price_left_cheap_when_band_leaves() {
        let (modbus_raw, _calls) = FakeModbus::new();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(modbus_raw);
        let (_dir, store) = make_store();
        store
            .record_sample(
                crate::storage::Sensor::Room,
                std::time::SystemTime::now(),
                20.0,
            )
            .unwrap();
        // Start cheap so the bath watcher's first eval would survive, then
        // flip to Normal before the first tick.
        let price = price_with_current(crate::energy::price::PriceLevel::VeryCheap, 0.30);
        replace_price(&price, Some(crate::energy::price::PriceLevel::Normal), 0.80);
        let cfg = crate::config::DhwConfig::default();
        let (ev_tx, _ev_rx) = mpsc::channel::<BathWatcherEvent>(8);
        let (done_tx, done_rx) = oneshot::channel();
        let started_at = tokio::time::Instant::now();

        let handle = tokio::spawn(run_bath_watcher(
            Duration::from_hours(1),
            started_at,
            false,
            modbus,
            store,
            price,
            cfg,
            ev_tx,
            done_tx,
        ));
        tokio::time::advance(Duration::from_secs(70)).await;
        let reason = done_rx.await.unwrap();
        assert_eq!(reason, CancelReason::PriceLeftCheap);
        handle.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn bath_watcher_engages_immersion_when_spot_drops_below_on_threshold() {
        let (modbus_raw, calls) = FakeModbus::new();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(modbus_raw);
        let (_dir, store) = make_store();
        store
            .record_sample(
                crate::storage::Sensor::Room,
                std::time::SystemTime::now(),
                20.0,
            )
            .unwrap();
        // Spot=0.30 < on_threshold (0.50 - 0.05 = 0.45). Cheap band so the
        // price gate passes.
        let price = price_with_current(crate::energy::price::PriceLevel::VeryCheap, 0.30);
        let cfg = crate::config::DhwConfig::default();
        let (ev_tx, mut ev_rx) = mpsc::channel::<BathWatcherEvent>(8);
        let (done_tx, _done_rx) = oneshot::channel();
        let started_at = tokio::time::Instant::now();

        // Initial gate state: disengaged. The first tick should engage it.
        let handle = tokio::spawn(run_bath_watcher(
            Duration::from_hours(1),
            started_at,
            false,
            modbus,
            store,
            price,
            cfg,
            ev_tx,
            done_tx,
        ));
        tokio::time::advance(Duration::from_secs(70)).await;
        // Wait for the watcher to emit its engage event — once we've seen it
        // the 61591 write must already be in the log (the watcher writes
        // before it sends the event).
        let ev = ev_rx.recv().await.expect("ImmersionEngaged event");
        assert_eq!(ev, BathWatcherEvent::ImmersionEngaged(true));

        let writes = calls.lock().unwrap().clone();
        assert!(
            writes
                .iter()
                .any(|(a, v)| *a == 61591 && (*v - 3.0).abs() < 1e-4),
            "expected 61591=3.0 write on engage, got {writes:?}"
        );
        // Sanity: confirm via the float helper too.
        if let Some((_, v)) = writes.iter().find(|(a, _)| *a == 61591) {
            assert_float_eq(*v, 3.0, "61591 engage value");
        }
        handle.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn bath_watcher_disengages_immersion_when_spot_rises_above_off_threshold() {
        let (modbus_raw, calls) = FakeModbus::new();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(modbus_raw);
        let (_dir, store) = make_store();
        store
            .record_sample(
                crate::storage::Sensor::Room,
                std::time::SystemTime::now(),
                20.0,
            )
            .unwrap();
        // Spot=0.70 > off_threshold (0.50 + 0.05 = 0.55). Still classified
        // Cheap so the price-band gate passes but the gate should disengage.
        let price = price_with_current(crate::energy::price::PriceLevel::Cheap, 0.70);
        let cfg = crate::config::DhwConfig::default();
        let (ev_tx, mut ev_rx) = mpsc::channel::<BathWatcherEvent>(8);
        let (done_tx, _done_rx) = oneshot::channel();
        let started_at = tokio::time::Instant::now();

        // Initial gate state: engaged. The first tick should disengage it.
        let handle = tokio::spawn(run_bath_watcher(
            Duration::from_hours(1),
            started_at,
            true,
            modbus,
            store,
            price,
            cfg,
            ev_tx,
            done_tx,
        ));
        tokio::time::advance(Duration::from_secs(70)).await;
        let ev = ev_rx.recv().await.expect("ImmersionEngaged event");
        assert_eq!(ev, BathWatcherEvent::ImmersionEngaged(false));

        let writes = calls.lock().unwrap().clone();
        assert!(
            writes.iter().any(|(a, v)| *a == 61591 && v.abs() < 1e-4),
            "expected 61591=0 write on disengage, got {writes:?}"
        );
        if let Some((_, v)) = writes.iter().find(|(a, _)| *a == 61591) {
            assert_float_eq(*v, 0.0, "61591 disengage value");
        }
        handle.abort();
    }
}
