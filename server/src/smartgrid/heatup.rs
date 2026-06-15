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

/// Why the warm-by heat-up finished and the system should re-block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoneReason {
    /// Tank-top temperature reached the target.
    TargetReached,
    /// The compressor finished its cycle (left the heating state after having
    /// been seen heating).
    CompressorStopped,
    /// The `max_duration` safety cap elapsed — re-block regardless.
    MaxDuration,
}

/// A resolved warm-by request handed from the route to the actor. The route
/// has already done the temp-aware sizing, so the actor performs no reads.
#[derive(Debug, Clone, Copy)]
pub struct WarmByCommand {
    /// When to flip to Normal and start heating. `None` means the tank is
    /// already at/above target — just block, no heat-up.
    pub heatup_start: Option<SystemTime>,
    /// Target tank-top temperature (°C) the watcher heats toward.
    pub target_c: f32,
    /// Safety cap on the heat-up phase before forcing a re-block.
    pub max_duration: Duration,
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
/// heat-up is sized to close the gap from the predicted temperature up to the
/// target (`(target - predicted) / heat_rate`, clamped to `max_duration`).
/// Because the heat-up is placed close to the deadline, heating to target from
/// the deadline-predicted temperature lands the tank at target around the
/// deadline. A non-positive heat rate falls back to `max_duration`.
#[must_use]
pub fn estimate_heatup(
    current_c: f32,
    target_c: f32,
    heat_rate_c_per_min: f32,
    cooldown_c_per_min: f32,
    minutes_until_deadline: f32,
    max_duration: Duration,
) -> Option<Duration> {
    let predicted_at_deadline = current_c - cooldown_c_per_min * minutes_until_deadline.max(0.0);
    if predicted_at_deadline >= target_c {
        return None;
    }
    if heat_rate_c_per_min <= 0.0 {
        return Some(max_duration);
    }
    let minutes = (target_c - predicted_at_deadline) / heat_rate_c_per_min;
    let secs = (minutes * 60.0).max(0.0);
    Some(Duration::from_secs_f32(secs).min(max_duration))
}

/// Whether a heat-pump status reading indicates active heating.
#[must_use]
pub fn is_heating(hp_status: Option<i64>) -> bool {
    hp_status == Some(HP_STATUS_HEATING)
}

/// Decide whether the heat-up is done this tick. Pure: the watcher passes the
/// latest readings plus the accumulated `seen_heating` latch and elapsed time.
///
/// Checked in order:
/// 1. `elapsed >= max_duration` → [`DoneReason::MaxDuration`] (always
///    evaluable, so a stuck sensor still re-blocks eventually).
/// 2. tank temp `>= target` → [`DoneReason::TargetReached`].
/// 3. compressor was seen heating and is no longer → [`DoneReason::CompressorStopped`].
///
/// A failed read (`None`) simply skips its check this tick — no false trigger.
/// `seen_heating` must already reflect this tick (caller folds in
/// [`is_heating`] before calling), so a tick where the compressor is *still*
/// heating never reports `CompressorStopped`.
#[must_use]
pub fn evaluate_heatup_done(
    temp_c: Option<f32>,
    hp_status: Option<i64>,
    seen_heating: bool,
    elapsed: Duration,
    target_c: f32,
    max_duration: Duration,
) -> Option<DoneReason> {
    if elapsed >= max_duration {
        return Some(DoneReason::MaxDuration);
    }
    if let Some(t) = temp_c
        && t >= target_c
    {
        return Some(DoneReason::TargetReached);
    }
    if seen_heating
        && let Some(s) = hp_status
        && s != HP_STATUS_HEATING
    {
        return Some(DoneReason::CompressorStopped);
    }
    None
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
        assert!(estimate_heatup(50.0, 48.0, 0.4, 0.05, 30.0, Duration::from_mins(90)).is_none());
        // Exactly at target with zero cooldown horizon also skips.
        assert!(estimate_heatup(48.0, 48.0, 0.4, 0.0, 0.0, Duration::from_mins(90)).is_none());
    }

    #[test]
    fn estimate_does_not_skip_when_tank_will_cool_below_target() {
        // Warm now (50 °C) but the deadline is 8 h out: at 0.05 °C/min the tank
        // loses 24 °C → ~26 °C, well below the 48 °C target → must heat.
        let d = estimate_heatup(50.0, 48.0, 0.4, 0.05, 480.0, Duration::from_mins(90))
            .expect("must schedule a heat-up, not skip");
        // gap = 48 - (50 - 24) = 22 °C at 0.4 °C/min = 55 min.
        assert_float_eq(
            d.as_secs_f32(),
            55.0 * 60.0,
            "heat-up sized for cooldown deficit",
        );
    }

    #[test]
    fn estimate_sizes_from_temp_gap_and_rate() {
        // No cooldown horizon: 48 - 24 = 24 °C at 0.4 °C/min = 60 min.
        let d = estimate_heatup(24.0, 48.0, 0.4, 0.0, 0.0, Duration::from_mins(90)).expect("some");
        assert_float_eq(d.as_secs_f32(), 3600.0, "60 min heat-up");
    }

    #[test]
    fn estimate_clamps_to_max_duration() {
        // 48 - 0 = 48 °C at 0.4 = 120 min, capped at 90.
        let d = estimate_heatup(0.0, 48.0, 0.4, 0.0, 0.0, Duration::from_mins(90)).expect("some");
        assert_eq!(d, Duration::from_mins(90));
    }

    #[test]
    fn estimate_non_positive_rate_falls_back_to_cap() {
        let d = estimate_heatup(20.0, 48.0, 0.0, 0.0, 0.0, Duration::from_mins(90)).expect("some");
        assert_eq!(d, Duration::from_mins(90));
    }

    #[test]
    fn done_on_max_duration_even_with_no_reads() {
        let got = evaluate_heatup_done(
            None,
            None,
            false,
            Duration::from_mins(90),
            48.0,
            Duration::from_mins(90),
        );
        assert_eq!(got, Some(DoneReason::MaxDuration));
    }

    #[test]
    fn done_on_target_reached() {
        let got = evaluate_heatup_done(
            Some(48.0),
            Some(HP_STATUS_HEATING),
            true,
            Duration::from_mins(10),
            48.0,
            Duration::from_mins(90),
        );
        assert_eq!(got, Some(DoneReason::TargetReached));
    }

    #[test]
    fn compressor_stop_only_after_seen_heating() {
        // Not yet seen heating (compressor hasn't spun up) → no trigger.
        let early = evaluate_heatup_done(
            Some(30.0),
            Some(0),
            false,
            Duration::from_mins(1),
            48.0,
            Duration::from_mins(90),
        );
        assert_eq!(early, None);
        // After it was seen heating and has now stopped → CompressorStopped.
        let later = evaluate_heatup_done(
            Some(40.0),
            Some(0),
            true,
            Duration::from_mins(20),
            48.0,
            Duration::from_mins(90),
        );
        assert_eq!(later, Some(DoneReason::CompressorStopped));
    }

    #[test]
    fn still_heating_does_not_stop() {
        let got = evaluate_heatup_done(
            Some(40.0),
            Some(HP_STATUS_HEATING),
            true,
            Duration::from_mins(20),
            48.0,
            Duration::from_mins(90),
        );
        assert_eq!(got, None);
    }

    #[test]
    fn transient_read_failure_no_trigger() {
        // Both reads failed mid-heat-up, cap not reached → keep waiting.
        let got = evaluate_heatup_done(
            None,
            None,
            true,
            Duration::from_mins(20),
            48.0,
            Duration::from_mins(90),
        );
        assert_eq!(got, None);
    }

    #[test]
    fn is_heating_matches_only_status_three() {
        assert!(is_heating(Some(HP_STATUS_HEATING)));
        assert!(!is_heating(Some(0)));
        assert!(!is_heating(Some(2)));
        assert!(!is_heating(None));
    }
}
