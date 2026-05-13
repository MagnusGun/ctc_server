//! Legacy module retained for its `GpioController`-level supersession tests.
//!
//! The `apply_mode` and `run_resume_task` helpers that used to live here have
//! moved into [`super::actor`], which owns all `SmartGrid` state in a single
//! task and processes commands serially (no more route-level mutex needed,
//! no more orphan timer tasks at shutdown).

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::smartgrid::SmartGridMode;
    use crate::smartgrid::gpio::GpioController;

    /// When a manual mode change bumps the generation between the moment a
    /// resume task is scheduled and the moment it fires, the supersession
    /// check inside `set_mode_if_not_superseded` must reject the flip back
    /// to Normal so the user's manual choice survives.
    #[test]
    fn supersession_rejects_resume_after_manual_change() {
        let mut gpio = GpioController::new_for_test(20, 21, false);
        let scheduled_generation = gpio.mode_generation();
        gpio.bump_mode_generation();
        let outcome = gpio.set_mode_if_not_superseded(SmartGridMode::Normal, scheduled_generation);
        assert!(
            matches!(outcome, Ok(false)),
            "expected Ok(false) (superseded), got {outcome:?}"
        );
    }

    /// Regression for the original `run_resume_task` end-to-end behaviour.
    /// The actor version performs the same check inside `on_resume_fire`;
    /// this test stays as a guard against the underlying `GpioController`
    /// safeguard being weakened.
    #[tokio::test(flavor = "current_thread")]
    async fn supersession_after_simulated_sleep() {
        let mut gpio = GpioController::new_for_test(20, 21, false);
        let scheduled_generation = gpio.mode_generation();

        let _fires_at = SystemTime::now() + Duration::from_millis(60);
        tokio::time::sleep(Duration::from_millis(60)).await;
        // Manual mode change before the simulated fire.
        gpio.bump_mode_generation();

        let outcome = gpio.set_mode_if_not_superseded(SmartGridMode::Normal, scheduled_generation);
        assert!(
            matches!(outcome, Ok(false)),
            "supersession check must reject the captured generation"
        );
        assert!(
            gpio.mode_changed_at().is_none(),
            "no mode write should have committed"
        );
    }
}
