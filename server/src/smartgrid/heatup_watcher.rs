//! Phase-B watcher for the warm-by heat-up.
//!
//! Spawned by the actor once the heat-up has started (mode flipped to Normal).
//! Polls the compressor status every 60s and, once the heat pump has run and
//! then turned off, posts [`HeatupDoneFire`] back to the actor so it re-blocks.
//!
//! The server does not decide when the tank is "warm enough" — the heat pump's
//! own charge cycle does. There is intentionally **no** time cap and no
//! target-temperature stop: the watcher only listens for the compressor to
//! finish. If the compressor never runs, the watcher keeps waiting (the user
//! accepted that trade-off); a manual mode change or shutdown aborts it.
//!
//! Mirrors the DHW Bath watcher (`dhw/watcher.rs`): 60s tick with
//! `MissedTickBehavior::Delay` and the immediate first tick consumed so the
//! first evaluation lands at `+60s` (the compressor needs time to spin up).
//!
//! [`HeatupDoneFire`]: super::actor::SmartGridCmd::HeatupDoneFire

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::dhw::actor::ModbusWriter;

use super::actor::SmartGridCmd;
use super::heatup::{REG_HP_STATUS, heatup_complete, is_heating};

/// Run the phase-B heat-up watcher until the compressor finishes its cycle (or
/// the actor shuts down / aborts it and the channel closes).
pub async fn run_heatup_watcher(
    generation: u64,
    modbus: Arc<dyn ModbusWriter>,
    self_tx: mpsc::Sender<SmartGridCmd>,
) {
    let mut tick = tokio::time::interval(Duration::from_mins(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Consume the immediate first tick so the first eval lands at +60s, not
    // t=0 (the compressor needs time to spin up before any reading is useful).
    let _ = tick.tick().await;

    let mut seen_heating = false;
    loop {
        tick.tick().await;

        // A failed read returns None and is treated as "not stopped" — keep
        // waiting rather than re-block on a transient Modbus error.
        // Status is a small enumerated integer (0..=3); the rounded scaled
        // read always fits an i64, so the truncation lint is a non-issue here.
        #[allow(clippy::cast_possible_truncation)]
        let hp_status = modbus
            .read_scaled(REG_HP_STATUS)
            .await
            .ok()
            .map(|f| f.round() as i64);
        seen_heating = seen_heating || is_heating(hp_status);

        if heatup_complete(hp_status, seen_heating) {
            let _ = self_tx
                .send(SmartGridCmd::HeatupDoneFire { generation })
                .await;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Modbus fake that returns a scripted sequence of status values, holding
    /// the last value once the script is exhausted.
    struct ScriptedReader {
        vals: Vec<f32>,
        idx: AtomicUsize,
    }
    impl ScriptedReader {
        fn new(vals: Vec<f32>) -> Self {
            Self {
                vals,
                idx: AtomicUsize::new(0),
            }
        }
    }
    #[async_trait::async_trait]
    impl ModbusWriter for ScriptedReader {
        async fn write_scaled(&self, _addr: u16, _v: f32) -> Result<(), String> {
            Ok(())
        }
        async fn read_scaled(&self, _addr: u16) -> Result<f32, String> {
            let i = self.idx.fetch_add(1, Ordering::SeqCst);
            Ok(self.vals[i.min(self.vals.len() - 1)])
        }
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_reblocks_when_compressor_stops() {
        let (tx, mut rx) = mpsc::channel(8);
        // Tick 1: status 3 (heating, seen_heating=true). Tick 2: status 0
        // (stopped) → complete.
        let modbus: Arc<dyn ModbusWriter> = Arc::new(ScriptedReader::new(vec![3.0, 0.0]));
        tokio::spawn(run_heatup_watcher(7, modbus, tx));
        tokio::time::advance(Duration::from_secs(61)).await; // first eval tick
        tokio::time::advance(Duration::from_mins(1)).await; // second eval tick
        let cmd = rx.recv().await.expect("watcher posts a done-fire");
        match cmd {
            SmartGridCmd::HeatupDoneFire { generation } => assert_eq!(generation, 7),
            _ => panic!("unexpected command"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_waits_while_compressor_runs() {
        let (tx, mut rx) = mpsc::channel(8);
        // Always heating → never completes.
        let modbus: Arc<dyn ModbusWriter> = Arc::new(ScriptedReader::new(vec![3.0]));
        tokio::spawn(run_heatup_watcher(7, modbus, tx));
        tokio::time::advance(Duration::from_mins(30)).await;
        assert!(
            rx.try_recv().is_err(),
            "must not re-block while the compressor is still heating"
        );
    }
}
