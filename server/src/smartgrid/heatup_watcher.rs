//! Phase-B watcher for the warm-by heat-up.
//!
//! Spawned by the actor once the heat-up has started (mode flipped to Normal).
//! Polls the tank-top temperature and compressor status every 60s and, when
//! [`evaluate_heatup_done`] reports a stop trigger, posts [`HeatupDoneFire`]
//! back to the actor so it can re-block.
//!
//! Mirrors the DHW Bath watcher (`dhw/watcher.rs`): 60s tick with
//! `MissedTickBehavior::Delay`, the immediate first tick consumed so the first
//! evaluation lands at `+60s`, and a `tokio::time::Instant` time source so
//! `start_paused` tests can drive expiry via `tokio::time::advance`.
//!
//! [`HeatupDoneFire`]: super::actor::SmartGridCmd::HeatupDoneFire

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::dhw::actor::ModbusWriter;

use super::actor::SmartGridCmd;
use super::heatup::{REG_DHW_UPPER, REG_HP_STATUS, evaluate_heatup_done, is_heating};

/// Run the phase-B heat-up watcher until a stop trigger fires (or the actor
/// shuts down and the channel closes).
pub async fn run_heatup_watcher(
    started_at: tokio::time::Instant,
    target_c: f32,
    max_duration: Duration,
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

        // Failed reads return None and simply skip their check this tick; the
        // Instant-based max_duration cap still guarantees an eventual re-block.
        let temp_c = modbus.read_scaled(REG_DHW_UPPER).await.ok();
        // Status is a small enumerated integer (0..=3); the rounded scaled
        // read always fits an i64, so the truncation lint is a non-issue here.
        #[allow(clippy::cast_possible_truncation)]
        let hp_status = modbus
            .read_scaled(REG_HP_STATUS)
            .await
            .ok()
            .map(|f| f.round() as i64);
        seen_heating = seen_heating || is_heating(hp_status);

        if let Some(reason) = evaluate_heatup_done(
            temp_c,
            hp_status,
            seen_heating,
            started_at.elapsed(),
            target_c,
            max_duration,
        ) {
            let _ = self_tx
                .send(SmartGridCmd::HeatupDoneFire { generation, reason })
                .await;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smartgrid::heatup::DoneReason;

    /// Modbus fake returning a fixed temperature for any read.
    struct FakeReader(f32);
    #[async_trait::async_trait]
    impl ModbusWriter for FakeReader {
        async fn write_scaled(&self, _addr: u16, _v: f32) -> Result<(), String> {
            Ok(())
        }
        async fn read_scaled(&self, _addr: u16) -> Result<f32, String> {
            Ok(self.0)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_posts_done_when_target_reached() {
        let (tx, mut rx) = mpsc::channel(8);
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeReader(50.0)); // ≥ target
        tokio::spawn(run_heatup_watcher(
            tokio::time::Instant::now(),
            48.0,
            Duration::from_mins(90),
            7,
            modbus,
            tx,
        ));
        // Past the consumed first tick into the first evaluation tick.
        tokio::time::advance(Duration::from_secs(61)).await;
        let cmd = rx.recv().await.expect("watcher posts a done-fire");
        match cmd {
            SmartGridCmd::HeatupDoneFire { generation, reason } => {
                assert_eq!(generation, 7);
                assert_eq!(reason, DoneReason::TargetReached);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn watcher_caps_at_max_duration_when_cold() {
        let (tx, mut rx) = mpsc::channel(8);
        // Cold tank, compressor never seen heating → only the cap can fire.
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeReader(20.0));
        tokio::spawn(run_heatup_watcher(
            tokio::time::Instant::now(),
            48.0,
            Duration::from_mins(15),
            9,
            modbus,
            tx,
        ));
        tokio::time::advance(Duration::from_secs(16 * 60)).await;
        let cmd = rx.recv().await.expect("watcher posts a done-fire");
        match cmd {
            SmartGridCmd::HeatupDoneFire { generation, reason } => {
                assert_eq!(generation, 9);
                assert_eq!(reason, DoneReason::MaxDuration);
            }
            _ => panic!("unexpected command"),
        }
    }
}
