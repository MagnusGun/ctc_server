//! Pure decision logic for the "Block + warm-by deadline" one-shot heat-up.
//!
//! The scheduler is split so the I/O-free decisions live here and can be
//! unit-tested in isolation, while the actor (`actor.rs`) owns the timer and
//! GPIO state machine and the watcher (`heatup_watcher.rs`) owns the 60s poll
//! loop. Nothing in this file performs I/O.
//!
//! Flow: the route reads the current tank temperature, calls
//! [`estimate_heatup`] to size (or skip) the heat-up, and resolves a
//! [`WarmByCommand`]. The actor blocks immediately, waits until
//! `heatup_start`, flips to Normal so the heat pump charges the tank, then
//! the watcher calls [`evaluate_heatup_done`] each tick to decide when to
//! re-block.

use std::time::{Duration, SystemTime};

/// Heat-pump status register (`HEATPUMP_STATUS`, reg 62017) value that means
/// the compressor is actively heating. Other values are off/ready/wait.
pub const HP_STATUS_HEATING: i64 = 3;

/// Modbus register for the hot-water tank-top temperature (`CTC_ACTUAL_TEMP_DHW`).
pub const REG_DHW_UPPER: u16 = 62276;

/// Modbus register for the heat-pump compressor status (`HEATPUMP_STATUS`).
pub const REG_HP_STATUS: u16 = 62017;

/// A resolved warm-by request handed from the route to the actor. The route
/// has already done the temp-aware sizing, so the actor performs no reads.
#[derive(Debug, Clone, Copy)]
pub struct WarmByCommand {
    /// When to flip to Normal and start heating. `None` means the tank will
    /// still be warm enough at the deadline — just block, no heat-up.
    pub heatup_start: Option<SystemTime>,
}

/// Estimate how long the tank needs to heat to reach `target_c` **at the
/// deadline**, or `None` when the tank will still be warm enough then (skip).
///
/// The decision is deadline-aware: a tank that is warm *now* can be cold by a
/// deadline hours away, so we predict the temperature at the deadline using a
/// standby cooldown rate:
///
/// ```text
/// predicted_at_deadline = current - cooldown_rate * minutes_until_deadline
/// ```
///
/// If that prediction is still ≥ target the heat-up is skipped; otherwise the
/// returned duration sizes the gap from the predicted temperature up to the
/// target (`(target - predicted) / heat_rate`). This value is **only a hint**:
/// it sets the length of the cheap window the scheduler looks for and the
/// "est. N min" shown in the preview. It does **not** stop the heat-up — the
/// heat pump's own cycle does that (see [`heatup_complete`]). A non-positive
/// heat rate falls back to a neutral 60-minute hint.
#[must_use]
pub fn estimate_heatup(
    current_c: f32,
    target_c: f32,
    heat_rate_c_per_min: f32,
    cooldown_c_per_min: f32,
    minutes_until_deadline: f32,
) -> Option<Duration> {
    let predicted_at_deadline = current_c - cooldown_c_per_min * minutes_until_deadline.max(0.0);
    if predicted_at_deadline >= target_c {
        return None;
    }
    if heat_rate_c_per_min <= 0.0 {
        return Some(Duration::from_hours(1));
    }
    let minutes = (target_c - predicted_at_deadline) / heat_rate_c_per_min;
    let secs = (minutes * 60.0).max(0.0);
    Some(Duration::from_secs_f32(secs))
}

/// Whether a heat-pump status reading indicates active heating.
#[must_use]
pub fn is_heating(hp_status: Option<i64>) -> bool {
    hp_status == Some(HP_STATUS_HEATING)
}

/// True once the heat pump has run and then turned off — i.e. the heater's
/// own cycle has decided the charge is complete, so the server should re-block.
///
/// The server does **not** decide when heating is "enough"; it only watches
/// for the compressor to finish. `seen_heating` must already fold in this
/// tick's status (via [`is_heating`]), so a tick where the compressor is still
/// heating never reports complete. A failed status read (`None`) is treated as
/// "not stopped" — keep waiting rather than re-block on a transient error.
#[must_use]
pub fn heatup_complete(hp_status: Option<i64>, seen_heating: bool) -> bool {
    seen_heating && matches!(hp_status, Some(s) if s != HP_STATUS_HEATING)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_float_eq(a: f32, b: f32, msg: &str) {
        assert!((a - b).abs() < f32::EPSILON, "{msg}: expected {b}, got {a}");
    }

    #[test]
    fn estimate_skips_only_when_warm_through_the_deadline() {
        // Warm now AND a near deadline (no meaningful cooldown) → skip.
        assert!(estimate_heatup(50.0, 48.0, 0.4, 0.05, 30.0).is_none());
        // Exactly at target with zero cooldown horizon also skips.
        assert!(estimate_heatup(48.0, 48.0, 0.4, 0.0, 0.0).is_none());
    }

    #[test]
    fn estimate_does_not_skip_when_tank_will_cool_below_target() {
        // Warm now (50 °C) but the deadline is 8 h out: at 0.05 °C/min the tank
        // loses 24 °C → ~26 °C, well below the 48 °C target → must heat.
        let d = estimate_heatup(50.0, 48.0, 0.4, 0.05, 480.0)
            .expect("must schedule a heat-up, not skip");
        // gap = 48 - (50 - 24) = 22 °C at 0.4 °C/min = 55 min.
        assert_float_eq(
            d.as_secs_f32(),
            55.0 * 60.0,
            "heat-up hint sized for cooldown deficit",
        );
    }

    #[test]
    fn estimate_sizes_from_temp_gap_and_rate() {
        // No cooldown horizon: 48 - 24 = 24 °C at 0.4 °C/min = 60 min.
        let d = estimate_heatup(24.0, 48.0, 0.4, 0.0, 0.0).expect("some");
        assert_float_eq(d.as_secs_f32(), 3600.0, "60 min heat-up hint");
    }

    #[test]
    fn estimate_non_positive_rate_falls_back_to_neutral_hint() {
        let d = estimate_heatup(20.0, 48.0, 0.0, 0.0, 0.0).expect("some");
        assert_eq!(d, Duration::from_hours(1));
    }

    #[test]
    fn complete_only_after_seen_heating_then_stopped() {
        // Not yet seen heating (compressor hasn't spun up) → not complete.
        assert!(!heatup_complete(Some(0), false));
        // Seen heating and now stopped → complete (the heater decided).
        assert!(heatup_complete(Some(0), true));
    }

    #[test]
    fn still_heating_is_not_complete() {
        assert!(!heatup_complete(Some(HP_STATUS_HEATING), true));
    }

    #[test]
    fn transient_read_failure_is_not_complete() {
        // Status read failed this tick → keep waiting, don't re-block.
        assert!(!heatup_complete(None, true));
    }

    #[test]
    fn is_heating_matches_only_status_three() {
        assert!(is_heating(Some(HP_STATUS_HEATING)));
        assert!(!is_heating(Some(0)));
        assert!(!is_heating(Some(2)));
        assert!(!is_heating(None));
    }
}
