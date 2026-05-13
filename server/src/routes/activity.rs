//! Activity-timeline lane derivation.
//!
//! `GET /api/v1/heatpump/activity?hours=24` returns lane segments
//! (Heating / DHW / Brine) derived on demand by walking the in-memory
//! series rings for `SystemStatus`, `HpStatus`, and `BrinePump`.
//!
//! Lane predicates:
//! * **Heating** — `SystemStatus ∈ {0, 1, 4, 8, 10}` AND compressor on
//!   (`HpStatus ∈ {3, 4, 5}`).
//! * **DHW** — `SystemStatus ∈ {5, 10}` AND compressor on.
//! * **Brine** — `BrinePump > 0`. Independent of compressor.

use crate::error::ApiError;
use crate::routes::series_window;
use crate::storage::{Sensor, Store};
use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

pub fn routes(store: Store) -> Router {
    Router::new()
        .route("/api/v1/heatpump/activity", get(get_activity))
        .with_state(store)
}

#[derive(Debug, Deserialize)]
struct ActivityQuery {
    hours: Option<u32>,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Lane {
    Heating,
    Dhw,
    Brine,
}

#[derive(Debug, Serialize)]
struct ActivitySegment {
    lane: Lane,
    start_iso: String,
    end_iso: String,
}

async fn get_activity(
    State(store): State<Store>,
    Query(q): Query<ActivityQuery>,
) -> Result<axum::Json<Vec<ActivitySegment>>, ApiError> {
    let hours = q.hours.unwrap_or(24).clamp(1, 24);
    let (from, now_i64, to) = series_window(hours)?;

    let sys = store.series_range(Sensor::SystemStatus, from, to);
    let hp = store.series_range(Sensor::HpStatus, from, to);
    let bp = store.series_range(Sensor::BrinePump, from, to);

    Ok(axum::Json(segment(&sys, &hp, &bp, from, now_i64)))
}

/// Walk three time-sorted sample lists in parallel and emit segments per
/// lane. The series are independent — we don't try to align timestamps;
/// instead, for each lane we step through *its* sample list and emit
/// runs where the lane predicate (computed from the most recent sample
/// of each underlying signal at that instant) is true.
///
/// `from` is the window-open instant: if a lane is already on at the
/// first observed sample, its segment opens at `from` rather than at
/// that first sample. This avoids truncating cycles that began before
/// the window.
fn segment(
    sys: &[(i64, f32)],
    hp: &[(i64, f32)],
    bp: &[(i64, f32)],
    from: i64,
    now: i64,
) -> Vec<ActivitySegment> {
    // Build a merged time axis: every unique timestamp where any signal
    // changes. Walk it once and emit segments for each lane.
    let mut times: Vec<i64> = sys
        .iter()
        .chain(hp.iter())
        .chain(bp.iter())
        .map(|(t, _)| *t)
        .collect();
    times.sort_unstable();
    times.dedup();
    if times.is_empty() {
        return Vec::new();
    }

    let mut sys_i = 0usize;
    let mut hp_i = 0usize;
    let mut bp_i = 0usize;

    let mut last_sys: f32 = f32::NAN;
    let mut last_hp: f32 = f32::NAN;
    let mut last_brine: f32 = f32::NAN;

    let mut open: [Option<i64>; 3] = [None, None, None];
    let mut out: Vec<ActivitySegment> = Vec::new();
    // At the first tick, a lane that's already on must have begun before
    // the window opened; attribute its start to `from` rather than the
    // first sample's timestamp. Subsequent off→on transitions use `t`.
    let mut first_tick = true;

    for &t in &times {
        while sys_i < sys.len() && sys[sys_i].0 <= t {
            last_sys = sys[sys_i].1;
            sys_i += 1;
        }
        while hp_i < hp.len() && hp[hp_i].0 <= t {
            last_hp = hp[hp_i].1;
            hp_i += 1;
        }
        while bp_i < bp.len() && bp[bp_i].0 <= t {
            last_brine = bp[bp_i].1;
            bp_i += 1;
        }

        let on = [
            heating_on(last_sys, last_hp),
            dhw_on(last_sys, last_hp),
            brine_on(last_brine),
        ];

        let open_start = if first_tick { from } else { t };
        for (lane_idx, (&is_on, slot)) in on.iter().zip(open.iter_mut()).enumerate() {
            match (is_on, *slot) {
                (true, None) => *slot = Some(open_start),
                (false, Some(start)) => {
                    out.push(ActivitySegment {
                        lane: lane_from_idx(lane_idx),
                        start_iso: iso(start),
                        end_iso: iso(t),
                    });
                    *slot = None;
                }
                _ => {}
            }
        }
        first_tick = false;
    }

    // Close any open runs at `now`.
    for (lane_idx, slot) in open.iter().enumerate() {
        if let Some(start) = *slot {
            out.push(ActivitySegment {
                lane: lane_from_idx(lane_idx),
                start_iso: iso(start),
                end_iso: iso(now),
            });
        }
    }

    out
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn heating_on(sys: f32, hp: f32) -> bool {
    if sys.is_nan() || hp.is_nan() {
        return false;
    }
    let sys = sys as u16;
    let hp = hp as u16;
    matches!(sys, 0 | 1 | 4 | 8 | 10) && matches!(hp, 3..=5)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn dhw_on(sys: f32, hp: f32) -> bool {
    if sys.is_nan() || hp.is_nan() {
        return false;
    }
    let sys = sys as u16;
    let hp = hp as u16;
    matches!(sys, 5 | 10) && matches!(hp, 3..=5)
}

fn brine_on(bp: f32) -> bool {
    !bp.is_nan() && bp > 0.0
}

fn lane_from_idx(i: usize) -> Lane {
    match i {
        0 => Lane::Heating,
        1 => Lane::Dhw,
        _ => Lane::Brine,
    }
}

fn iso(unix_secs: i64) -> String {
    // chrono::DateTime::from_timestamp only returns None for values outside
    // roughly year ±9999. Reachable only from a corrupt clock or malformed
    // segment input; surface it in the log rather than silently emitting 1970.
    DateTime::<Utc>::from_timestamp(unix_secs, 0)
        .unwrap_or_else(|| {
            warn!("activity::iso: out-of-range unix_secs {unix_secs}; falling back to epoch");
            DateTime::<Utc>::from_timestamp(0, 0).unwrap()
        })
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_series_yields_empty() {
        let out = segment(&[], &[], &[], 0, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn single_dhw_run_emits_one_segment() {
        // SystemStatus=5 (DHW) at t=10, HpStatus=3 (Heating) at t=10,
        // SystemStatus=7 (Off) at t=100, HpStatus=1 (Ready) at t=100.
        let sys = vec![(10, 5.0), (100, 7.0)];
        let hp = vec![(10, 3.0), (100, 1.0)];
        let bp = vec![];
        let out = segment(&sys, &hp, &bp, 0, 100);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].lane, Lane::Dhw);
    }

    #[test]
    fn overlapping_heating_and_brine_emit_separate_segments() {
        // Heating predicate true for t∈[10, 100): SystemStatus=4, HpStatus=3.
        // Brine pump > 0 for t∈[10, 100). Closes when both hit zero/off at 100.
        let sys = vec![(10, 4.0), (100, 7.0)];
        let hp = vec![(10, 3.0), (100, 1.0)];
        let bp = vec![(10, 50.0), (100, 0.0)];
        let out = segment(&sys, &hp, &bp, 0, 100);
        assert_eq!(out.len(), 2);
        let lanes: Vec<Lane> = out.iter().map(|s| s.lane).collect();
        assert!(lanes.contains(&Lane::Heating));
        assert!(lanes.contains(&Lane::Brine));
    }

    #[test]
    fn open_run_closes_at_now() {
        // SystemStatus=5 (DHW), HpStatus=3 — both ON, no closing samples.
        // The segment should close at `now`.
        let sys = vec![(10, 5.0)];
        let hp = vec![(10, 3.0)];
        let bp = vec![];
        let out = segment(&sys, &hp, &bp, 0, 500);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].lane, Lane::Dhw);
        assert!(out[0].end_iso.contains(':')); // valid ISO string
    }

    #[test]
    fn brine_runs_independent_of_compressor() {
        // Brine pump on, but HpStatus off — Brine segment still emitted.
        let sys = vec![];
        let hp = vec![];
        let bp = vec![(10, 30.0), (60, 0.0)];
        let out = segment(&sys, &hp, &bp, 0, 60);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].lane, Lane::Brine);
    }

    /// Window-edge regression: a cycle that was already active at `from`
    /// must surface as a segment starting at `from`, not at the first
    /// in-window sample.
    #[test]
    fn segment_begins_at_window_open_when_cycle_already_active() {
        // Conceptual window: [from, now). The first in-window sample at
        // t=100 already shows heating ON, which means the cycle must have
        // begun before `from`. The activity timeline should show that.
        let from: i64 = 0;
        let now: i64 = 1000;
        let sys = vec![(100, 4.0), (500, 7.0)];
        let hp = vec![(100, 3.0), (500, 1.0)];
        let bp = vec![];

        let out = segment(&sys, &hp, &bp, from, now);
        assert_eq!(out.len(), 1, "exactly one heating segment");
        assert_eq!(out[0].lane, Lane::Heating);
        assert_eq!(
            out[0].start_iso,
            iso(from),
            "heating segment should begin at the window open (`from` = {from}), \
             not at the first in-window sample"
        );
    }

    /// The window-edge fix must not regress cycles that genuinely begin
    /// inside the window. An off→on transition after the first tick must
    /// still emit a segment starting at the actual transition time.
    #[test]
    fn segment_starts_at_transition_not_from_when_cycle_begins_in_window() {
        // First tick at t=100 shows OFF. Lane turns on at t=500.
        let from: i64 = 0;
        let now: i64 = 1000;
        let sys = vec![(100, 7.0), (500, 4.0), (900, 7.0)];
        let hp = vec![(100, 1.0), (500, 3.0), (900, 1.0)];
        let bp = vec![];

        let out = segment(&sys, &hp, &bp, from, now);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].lane, Lane::Heating);
        assert_eq!(
            out[0].start_iso,
            iso(500),
            "segment should start at the genuine transition, not at `from`"
        );
        assert_eq!(out[0].end_iso, iso(900));
    }
}
