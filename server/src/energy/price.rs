//! Electricity price state management
//!
//! Manages spot price data from elprisetjustnu.se (Nord Pool).

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Serialize;

/// Thread-safe price state wrapper
#[derive(Clone)]
pub struct PriceState {
    inner: Arc<Mutex<PriceStateInner>>,
}

/// Internal price state
struct PriceStateInner {
    /// Current price point
    current: Option<PricePoint>,
    /// Today's prices (up to 96 entries for 15-min resolution)
    today: Vec<PricePoint>,
    /// Tomorrow's prices (up to 96 entries, available after ~13:00 CET)
    tomorrow: Vec<PricePoint>,
    /// Price zone (SE1, SE2, SE3, SE4)
    price_zone: String,
}

/// A scored contiguous run produced by `runs_within`.
struct RunInfo {
    start: PricePoint,
    end: chrono::DateTime<chrono::FixedOffset>,
    avg: f64,
}

/// A single spot price point
#[derive(Clone, Debug, Serialize)]
pub struct PricePoint {
    /// Start time of price period (ISO 8601)
    pub starts_at: String,
    /// End time of price period (ISO 8601)
    pub ends_at: String,

    // From elprisetjustnu.se (raw spot price)
    /// Spot price in SEK/kWh
    pub spot_sek: f64,
    /// Spot price in EUR/kWh
    pub spot_eur: f64,
    /// EUR->SEK exchange rate
    pub exchange_rate: f64,

    /// Price level (computed from spot percentiles)
    pub level: Option<PriceLevel>,
}

/// Price level classification
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PriceLevel {
    VeryCheap,
    Cheap,
    Normal,
    Expensive,
    VeryExpensive,
}

/// One candidate auto-resume run for the picker: a contiguous stretch of
/// length `run_duration` whose start the user may schedule a resume at.
#[derive(Clone, Debug, Serialize)]
pub struct ResumeCandidate {
    /// Run start (ISO 8601) — the schedulable resume instant.
    pub starts_at: String,
    /// Run end (ISO 8601) — start + accumulated slot durations.
    pub ends_at: String,
    /// Duration-weighted mean spot price over the run (SEK/kWh).
    pub avg_spot_sek: f64,
    /// Start slot's price level (drives the dashboard badge color).
    pub level: Option<PriceLevel>,
}

/// Statistics for a set of prices
#[derive(Clone, Debug, Serialize)]
pub struct PriceStatistics {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
}

/// True when `now` lies within the bucket's overall span
/// `[first.starts_at, last.ends_at)`. Empty or unparseable buckets cover no
/// instant. A bucket's calendar day is implicit in its slot timestamps, so this
/// is equivalent to "this bucket holds the data for now's local day".
fn bucket_covers(bucket: &[PricePoint], now: chrono::DateTime<chrono::Utc>) -> bool {
    let (Some(first), Some(last)) = (bucket.first(), bucket.last()) else {
        return false;
    };
    match (
        chrono::DateTime::parse_from_rfc3339(&first.starts_at),
        chrono::DateTime::parse_from_rfc3339(&last.ends_at),
    ) {
        (Ok(start), Ok(end)) => now >= start && now < end,
        _ => false,
    }
}

impl PriceState {
    /// Create a new price state
    pub fn new(price_zone: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PriceStateInner {
                current: None,
                today: Vec::new(),
                tomorrow: Vec::new(),
                price_zone,
            })),
        }
    }

    /// Update prices
    pub fn update_prices(&self, today: Vec<PricePoint>, tomorrow: Vec<PricePoint>) {
        let mut inner = self.inner.lock().unwrap();

        // Find current price based on time
        let now = chrono::Utc::now();
        let current = today.iter().find(|p| {
            if let (Ok(start), Ok(end)) = (
                chrono::DateTime::parse_from_rfc3339(&p.starts_at),
                chrono::DateTime::parse_from_rfc3339(&p.ends_at),
            ) {
                now >= start && now < end
            } else {
                false
            }
        });

        inner.current = current.cloned();
        inner.today = today;
        inner.tomorrow = tomorrow;
    }

    /// Get the current price point.
    ///
    /// Returns the slot whose `[starts_at, ends_at)` covers the current time
    /// from either `today` or `tomorrow`. We search both because the next
    /// fetch may not have rotated yet just after midnight — but only when the
    /// tomorrow slot is in the future relative to today's last entry, so we
    /// don't mask the fact that `today` is genuinely stale.
    pub fn get_current(&self) -> Option<PricePoint> {
        let inner = self.inner.lock().unwrap();
        let now = chrono::Utc::now();

        let covers_now = |p: &&PricePoint| -> bool {
            if let (Ok(start), Ok(end)) = (
                chrono::DateTime::parse_from_rfc3339(&p.starts_at),
                chrono::DateTime::parse_from_rfc3339(&p.ends_at),
            ) {
                now >= start && now < end
            } else {
                false
            }
        };

        if let Some(price) = inner.today.iter().find(covers_now) {
            return Some(price.clone());
        }

        // Today doesn't cover `now`. If today is empty or its range is in the
        // past, the fetch loop hasn't rotated yet — fall back to tomorrow.
        // If today's range is in the future, there is no current price
        // (clock skew or near-midnight window) — return None.
        let today_in_past = inner
            .today
            .last()
            .and_then(|p| chrono::DateTime::parse_from_rfc3339(&p.ends_at).ok())
            .is_some_and(|end| now >= end);
        if inner.today.is_empty() || today_in_past {
            return inner.tomorrow.iter().find(covers_now).cloned();
        }
        None
    }

    /// Get today's prices
    pub fn get_today(&self) -> Vec<PricePoint> {
        let inner = self.inner.lock().unwrap();
        inner.today.clone()
    }

    /// Get tomorrow's prices (if available)
    pub fn get_tomorrow(&self) -> Vec<PricePoint> {
        let inner = self.inner.lock().unwrap();
        inner.tomorrow.clone()
    }

    /// Resolve the `(today, tomorrow)` arrays to serve from `/api/v1/prices`,
    /// compensating for the fetch loop's lack of a midnight rotation.
    ///
    /// The loop labels buckets at fetch time and never rotates them at midnight,
    /// so between 00:00 and the daily fetch hour the `today` bucket still holds
    /// yesterday while `tomorrow` holds the real today — the same staleness
    /// [`Self::get_current`] already works around.
    ///
    /// - `today` bucket spans `now` -> serve `(today, tomorrow)` unchanged.
    /// - else `tomorrow` bucket spans `now` -> the loop has not rotated; promote
    ///   it to today and report no next-day data (the genuine next day is not
    ///   fetched until the daily fetch hour).
    /// - else no bucket covers `now` (empty state, or multi-day-stale outage) ->
    ///   `None`; the route turns this into 503 rather than serving a wrong day.
    pub fn resolve_served_prices(&self) -> Option<(Vec<PricePoint>, Vec<PricePoint>)> {
        let inner = self.inner.lock().unwrap();
        let now = chrono::Utc::now();
        if bucket_covers(&inner.today, now) {
            Some((inner.today.clone(), inner.tomorrow.clone()))
        } else if bucket_covers(&inner.tomorrow, now) {
            Some((inner.tomorrow.clone(), Vec::new()))
        } else {
            None
        }
    }

    /// Get the price zone
    pub fn price_zone(&self) -> String {
        let inner = self.inner.lock().unwrap();
        inner.price_zone.clone()
    }

    /// Calculate statistics for spot prices
    #[must_use]
    pub fn get_spot_statistics(prices: &[PricePoint]) -> Option<PriceStatistics> {
        calculate_statistics(prices, |p| p.spot_sek)
    }

    /// Get optimal (cheapest) hours in the next 24h
    pub fn get_optimal_hours(&self, count: usize) -> Vec<PricePoint> {
        let inner = self.inner.lock().unwrap();

        // Combine today (remaining) and tomorrow prices
        let now = chrono::Utc::now();
        let mut future_prices: Vec<PricePoint> = inner
            .today
            .iter()
            .chain(inner.tomorrow.iter())
            .filter(|p| {
                chrono::DateTime::parse_from_rfc3339(&p.starts_at).is_ok_and(|start| start > now)
            })
            .cloned()
            .collect();

        // Sort by spot price (ascending)
        future_prices.sort_by(|a, b| {
            a.spot_sek
                .partial_cmp(&b.spot_sek)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        future_prices.into_iter().take(count).collect()
    }

    /// Returns the moment the current run of cheap slots ends, capped by `window`.
    ///
    /// Used for `LowPrice` / `Overcapacity` auto-resume: the user buffers heat now
    /// because prices are cheap, so we want to flip back to Normal as soon as
    /// prices stop being cheap. "Cheap" = level is `Some(VeryCheap | Cheap)`.
    ///
    /// - No price data with usable levels in the window → `None`.
    /// - First non-cheap slot inside the window → return its `starts_at`.
    /// - All in-window slots are cheap → return `now + window` so we never
    ///   stay in a buffer mode indefinitely.
    pub fn cheap_window_end(&self, window: Duration) -> Option<SystemTime> {
        let inner = self.inner.lock().unwrap();

        let now = chrono::Utc::now();
        let chrono_window = chrono::Duration::from_std(window).ok()?;
        let cutoff = now + chrono_window;

        // Walk slots chronologically. We need price data with levels to reason about
        // "cheap", so a slot with `level: None` is treated as a hard stop on the run
        // (we can't know whether it's still cheap).
        let mut found_in_window = false;
        for slot in inner.today.iter().chain(inner.tomorrow.iter()) {
            let Ok(start) = chrono::DateTime::parse_from_rfc3339(&slot.starts_at) else {
                continue;
            };
            if start <= now {
                continue;
            }
            if start > cutoff {
                break;
            }
            found_in_window = true;
            let is_cheap = matches!(slot.level, Some(PriceLevel::VeryCheap | PriceLevel::Cheap));
            if !is_cheap {
                return Some(SystemTime::from(start));
            }
        }

        if found_in_window {
            // All slots in the window were cheap — schedule at end of window.
            Some(SystemTime::from(cutoff))
        } else {
            None
        }
    }

    /// Get the single cheapest future price slot whose start is within `window` from now.
    ///
    /// Serves two callers: it's the **fallback** for Blocking auto-resume
    /// when no contiguous run of `auto_resume_min_duration_minutes` fits
    /// inside the window (see [`Self::cheapest_run_within`]), and it backs
    /// the proposed-resume preview endpoint when its run-based pick is
    /// unavailable for the same reason.
    pub fn cheapest_within(&self, window: Duration) -> Option<PricePoint> {
        let inner = self.inner.lock().unwrap();

        let now = chrono::Utc::now();
        let cutoff = now + chrono::Duration::from_std(window).ok()?;

        inner
            .today
            .iter()
            .chain(inner.tomorrow.iter())
            .filter(|p| {
                chrono::DateTime::parse_from_rfc3339(&p.starts_at)
                    .is_ok_and(|start| start > now && start <= cutoff)
            })
            .min_by(|a, b| {
                a.spot_sek
                    .partial_cmp(&b.spot_sek)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    /// Start of the cheapest contiguous run of length `run_duration` inside `window`.
    ///
    /// Walks the chained today/tomorrow slot timeline and, for each future
    /// slot whose start is inside `window`, accumulates strictly-adjacent
    /// neighbours until the run covers `run_duration`. A slot is "adjacent"
    /// to its predecessor only when its `starts_at` equals the predecessor's
    /// `ends_at` exactly — a gap or overlap ends the run. Slots with
    /// unparseable timestamps end the run too. The score for a run is the
    /// duration-weighted average `spot_sek`; the helper returns the start
    /// slot of the cheapest qualifying run, or `None` when no run of the
    /// required length fits anywhere inside `window`.
    ///
    /// Used for Blocking-mode `SmartGrid` auto-resume: we want the heater to
    /// resume into a cheap stretch that lasts long enough for an actual
    /// recovery cycle, not just the single cheapest 15-min tick.
    pub fn cheapest_run_within(
        &self,
        window: Duration,
        run_duration: Duration,
    ) -> Option<PricePoint> {
        self.runs_within(window, run_duration)
            .into_iter()
            .min_by(|a, b| {
                a.avg
                    .partial_cmp(&b.avg)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|r| r.start)
    }

    /// Start of the cheapest contiguous run of length `run_duration` that
    /// **finishes at or before `deadline`** and **starts no earlier than
    /// `deadline - max_lead`**.
    ///
    /// This is the deadline-anchored sibling of [`Self::cheapest_run_within`],
    /// used by the "warm-by" heat-up scheduler: heat the tank during the
    /// cheapest window that completes by the user's deadline, bounded to a
    /// lead window so the tank is still warm at the deadline (cooldown over
    /// the gap between the run end and the deadline stays small).
    ///
    /// Boundaries are inclusive: a run whose `end` equals `deadline`, or whose
    /// start equals `deadline - max_lead`, qualifies. Returns `None` when the
    /// deadline is already in the past, when no contiguous run of the required
    /// length fits inside `[deadline - max_lead, deadline]`, or when a run
    /// would be truncated by the edge of available price data (the caller then
    /// falls back to starting the heat-up immediately).
    pub fn cheapest_run_ending_by(
        &self,
        deadline: SystemTime,
        max_lead: Duration,
        run_duration: Duration,
    ) -> Option<PricePoint> {
        // Window for `runs_within` is "now until the deadline", so its cutoff
        // lands exactly on the deadline. `duration_since` is `Err` when the
        // deadline is already past → None.
        let window = deadline.duration_since(SystemTime::now()).ok()?;
        let deadline_utc = chrono::DateTime::<chrono::Utc>::from(deadline);
        let lead_start = deadline_utc - chrono::Duration::from_std(max_lead).ok()?;

        self.runs_within(window, run_duration)
            .into_iter()
            .filter(|r| {
                // `runs_within` filters on the run START, so the end-bound and
                // the lower start-bound must both be re-checked here.
                r.end.with_timezone(&chrono::Utc) <= deadline_utc
                    && chrono::DateTime::parse_from_rfc3339(&r.start.starts_at)
                        .is_ok_and(|s| s.with_timezone(&chrono::Utc) >= lead_start)
            })
            .min_by(|a, b| {
                a.avg
                    .partial_cmp(&b.avg)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|r| r.start)
    }

    /// All qualifying runs of length `run_duration` whose start is inside
    /// `window`, each scored by duration-weighted average `spot_sek`. Shared
    /// by `cheapest_run_within` (single best) and `cheapest_runs_within`
    /// (ranked list). Adjacency is exact (`slots[i].ends_at == slots[i+1].starts_at`).
    #[allow(clippy::cast_precision_loss)]
    fn runs_within(&self, window: Duration, run_duration: Duration) -> Vec<RunInfo> {
        let inner = self.inner.lock().unwrap();
        let now = chrono::Utc::now();
        let Ok(chrono_window) = chrono::Duration::from_std(window) else {
            return Vec::new();
        };
        let cutoff = now + chrono_window;
        let Ok(target_secs) = i64::try_from(run_duration.as_secs()) else {
            return Vec::new();
        };

        let slots: Vec<&PricePoint> = inner.today.iter().chain(inner.tomorrow.iter()).collect();
        let mut runs: Vec<RunInfo> = Vec::new();
        for i in 0..slots.len() {
            let start_slot = slots[i];
            let Ok(start) = chrono::DateTime::parse_from_rfc3339(&start_slot.starts_at) else {
                continue;
            };
            if start <= now || start > cutoff {
                continue;
            }

            let mut total_secs: i64 = 0;
            let mut weighted_sum: f64 = 0.0;
            let mut prev_end: Option<chrono::DateTime<chrono::FixedOffset>> = None;
            let mut run_end: Option<chrono::DateTime<chrono::FixedOffset>> = None;
            let mut covered = false;
            for slot in &slots[i..] {
                let (Ok(s), Ok(e)) = (
                    chrono::DateTime::parse_from_rfc3339(&slot.starts_at),
                    chrono::DateTime::parse_from_rfc3339(&slot.ends_at),
                ) else {
                    break;
                };
                if let Some(prev) = prev_end
                    && s != prev
                {
                    break;
                }
                let secs = (e - s).num_seconds();
                if secs <= 0 {
                    break;
                }
                total_secs += secs;
                weighted_sum += slot.spot_sek * (secs as f64);
                prev_end = Some(e);
                run_end = Some(e);
                if total_secs >= target_secs {
                    covered = true;
                    break;
                }
            }

            if let (true, Some(end)) = (covered && total_secs > 0, run_end) {
                runs.push(RunInfo {
                    start: start_slot.clone(),
                    end,
                    avg: weighted_sum / (total_secs as f64),
                });
            }
        }
        runs
    }

    /// Top-`k` cheapest **non-overlapping** runs of length `run_duration`
    /// inside `window`, cheapest first. Greedy: take the cheapest run, drop
    /// every run overlapping it, repeat. Backs the dashboard resume picker.
    pub fn cheapest_runs_within(
        &self,
        window: Duration,
        run_duration: Duration,
        k: usize,
    ) -> Vec<ResumeCandidate> {
        let mut runs = self.runs_within(window, run_duration);
        runs.sort_by(|a, b| {
            a.avg
                .partial_cmp(&b.avg)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut chosen: Vec<(
            chrono::DateTime<chrono::FixedOffset>,
            chrono::DateTime<chrono::FixedOffset>,
        )> = Vec::new();
        let mut out: Vec<ResumeCandidate> = Vec::new();
        for run in runs {
            if out.len() >= k {
                break;
            }
            let Ok(start) = chrono::DateTime::parse_from_rfc3339(&run.start.starts_at) else {
                continue;
            };
            let end = run.end;
            let overlaps = chosen.iter().any(|(s, e)| start < *e && *s < end);
            if overlaps {
                continue;
            }
            chosen.push((start, end));
            out.push(ResumeCandidate {
                starts_at: run.start.starts_at.clone(),
                ends_at: end.to_rfc3339(),
                avg_spot_sek: run.avg,
                level: run.start.level,
            });
        }
        out
    }
}

/// Calculate statistics from price points using a selector function
fn calculate_statistics<F>(prices: &[PricePoint], selector: F) -> Option<PriceStatistics>
where
    F: Fn(&PricePoint) -> f64,
{
    if prices.is_empty() {
        return None;
    }

    let values: Vec<f64> = prices.iter().map(&selector).collect();
    calculate_statistics_from_values(&values)
}

/// Calculate statistics from a list of values
#[allow(clippy::cast_precision_loss)]
fn calculate_statistics_from_values(values: &[f64]) -> Option<PriceStatistics> {
    if values.is_empty() {
        return None;
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let mean = values.iter().sum::<f64>() / values.len() as f64;

    let median = if sorted.len().is_multiple_of(2) {
        let middle = sorted.len() / 2;
        f64::midpoint(sorted[middle - 1], sorted[middle])
    } else {
        sorted[sorted.len() / 2]
    };

    Some(PriceStatistics {
        min,
        max,
        mean,
        median,
    })
}

impl PricePoint {
    /// Create a new price point from spot data
    pub fn from_spot(
        starts_at: String,
        ends_at: String,
        spot_sek: f64,
        spot_eur: f64,
        exchange_rate: f64,
    ) -> Self {
        Self {
            starts_at,
            ends_at,
            spot_sek,
            spot_eur,
            exchange_rate,
            level: None,
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::PricePoint;

    /// Build a 15-min `PricePoint` starting `offset_mins` from now. The
    /// timestamp is captured per call, so two `slot(...)` invocations are
    /// not contiguous in wall-clock terms (each picks up its own `now()`).
    /// Use [`make_run`] when contiguity matters.
    pub fn slot(offset_mins: i64, spot_sek: f64) -> PricePoint {
        let start = chrono::Utc::now() + chrono::Duration::minutes(offset_mins);
        let end = start + chrono::Duration::minutes(15);
        PricePoint::from_spot(start.to_rfc3339(), end.to_rfc3339(), spot_sek, 0.0, 0.0)
    }

    /// Build strictly-contiguous 15-min slots whose first slot starts at
    /// `now + start_offset_mins`. Each slot's `ends_at` equals the next
    /// slot's `starts_at` exactly — the format `cheapest_run_within` needs.
    pub fn make_run(start_offset_mins: i64, prices: &[f64]) -> Vec<PricePoint> {
        let base = chrono::Utc::now() + chrono::Duration::minutes(start_offset_mins);
        let mut out = Vec::with_capacity(prices.len());
        let mut s = base;
        for &p in prices {
            let e = s + chrono::Duration::minutes(15);
            out.push(PricePoint::from_spot(
                s.to_rfc3339(),
                e.to_rfc3339(),
                p,
                0.0,
                0.0,
            ));
            s = e;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{make_run, slot};
    use super::*;

    fn assert_float_eq(a: f64, b: f64, msg: &str) {
        assert!((a - b).abs() < 0.0001, "{msg}: expected {b}, got {a}");
    }

    #[test]
    fn test_price_state_new() {
        let state = PriceState::new("SE3".to_string());
        assert_eq!(state.price_zone(), "SE3");
        assert!(state.get_current().is_none());
        assert!(state.get_today().is_empty());
        assert!(state.get_tomorrow().is_empty());
    }

    #[test]
    fn test_price_point_from_spot() {
        let point = PricePoint::from_spot(
            "2026-01-04T14:00:00+01:00".to_string(),
            "2026-01-04T14:15:00+01:00".to_string(),
            0.72,
            0.065,
            11.08,
        );

        assert_float_eq(point.spot_sek, 0.72, "spot_sek");
        assert_float_eq(point.spot_eur, 0.065, "spot_eur");
        assert!(point.level.is_none());
    }

    #[test]
    fn concurrent_reads_during_writes_stay_consistent() {
        // Smoke test: spam writers and readers in parallel against the same
        // PriceState. The Mutex makes torn reads impossible by construction,
        // so the assertion is that every snapshot we observe is one we wrote
        // (no half-applied update) and the test completes without deadlock.

        // Two distinct snapshots — writes alternate between them. Use unique
        // spot_sek values so readers can identify which snapshot they saw.
        let snapshot_a = vec![PricePoint::from_spot(
            "2026-01-04T00:00:00Z".to_string(),
            "2026-01-04T00:15:00Z".to_string(),
            1.0,
            0.0,
            0.0,
        )];
        let snapshot_b = vec![PricePoint::from_spot(
            "2026-01-04T00:00:00Z".to_string(),
            "2026-01-04T00:15:00Z".to_string(),
            2.0,
            0.0,
            0.0,
        )];

        let state = PriceState::new("SE3".to_string());
        // Seed with snapshot_a so readers never see an empty intermediate.
        state.update_prices(snapshot_a.clone(), Vec::new());

        let writer_a = {
            let state = state.clone();
            let snap = snapshot_a.clone();
            std::thread::spawn(move || {
                for _ in 0..500 {
                    state.update_prices(snap.clone(), Vec::new());
                }
            })
        };
        let writer_b = {
            let state = state.clone();
            let snap = snapshot_b.clone();
            std::thread::spawn(move || {
                for _ in 0..500 {
                    state.update_prices(snap.clone(), Vec::new());
                }
            })
        };
        let reader = {
            let state = state.clone();
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    let today = state.get_today();
                    // Every snapshot must be one of the two we wrote — never
                    // empty, never a mix.
                    assert_eq!(today.len(), 1, "torn snapshot length");
                    let spot = today[0].spot_sek;
                    assert!(
                        (spot - 1.0).abs() < f64::EPSILON || (spot - 2.0).abs() < f64::EPSILON,
                        "torn snapshot content: spot={spot}"
                    );
                }
            })
        };

        writer_a.join().expect("writer_a");
        writer_b.join().expect("writer_b");
        reader.join().expect("reader");

        // After all writes complete, the state must hold one of the snapshots.
        let final_today = state.get_today();
        assert_eq!(final_today.len(), 1);
    }

    #[test]
    fn test_calculate_statistics() {
        let prices = vec![
            PricePoint::from_spot(String::new(), String::new(), 0.5, 0.0, 0.0),
            PricePoint::from_spot(String::new(), String::new(), 1.0, 0.0, 0.0),
            PricePoint::from_spot(String::new(), String::new(), 0.75, 0.0, 0.0),
            PricePoint::from_spot(String::new(), String::new(), 1.25, 0.0, 0.0),
        ];

        let stats = calculate_statistics(&prices, |p| p.spot_sek).unwrap();
        assert_float_eq(stats.min, 0.5, "min");
        assert_float_eq(stats.max, 1.25, "max");
        assert_float_eq(stats.mean, 0.875, "mean");
        assert_float_eq(stats.median, 0.875, "median"); // (0.75 + 1.0) / 2
    }

    #[test]
    fn test_cheapest_within_empty_state() {
        let state = PriceState::new("SE3".to_string());
        assert!(state.cheapest_within(Duration::from_hours(8)).is_none());
    }

    #[test]
    fn test_cheapest_within_picks_min_in_window() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![
            slot(30, 1.50),
            slot(60, 0.20), // cheapest within 8h
            slot(120, 0.50),
            slot(240, 0.30),
        ];
        state.update_prices(today, vec![]);
        let pick = state.cheapest_within(Duration::from_hours(8)).unwrap();
        assert_float_eq(pick.spot_sek, 0.20, "cheapest spot");
    }

    #[test]
    fn test_cheapest_within_excludes_past_slots() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![
            slot(-60, 0.05), // very cheap but in the past — must be skipped
            slot(30, 1.00),
            slot(60, 0.40),
        ];
        state.update_prices(today, vec![]);
        let pick = state.cheapest_within(Duration::from_hours(8)).unwrap();
        assert_float_eq(pick.spot_sek, 0.40, "ignores past, picks future min");
    }

    #[test]
    fn test_cheapest_within_excludes_slots_beyond_window() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![
            slot(60, 1.00),
            slot(120, 0.80),
            slot(600, 0.10), // 10h ahead — outside the 8h window even though cheapest
        ];
        state.update_prices(today, vec![]);
        let pick = state.cheapest_within(Duration::from_hours(8)).unwrap();
        assert_float_eq(pick.spot_sek, 0.80, "stays inside window");
    }

    #[test]
    fn test_cheapest_within_spans_today_and_tomorrow() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![slot(30, 1.20), slot(60, 0.90)];
        let tomorrow = vec![slot(180, 0.25), slot(240, 0.75)];
        state.update_prices(today, tomorrow);
        let pick = state.cheapest_within(Duration::from_hours(8)).unwrap();
        assert_float_eq(pick.spot_sek, 0.25, "tomorrow slot wins");
    }

    #[test]
    fn test_cheapest_within_zero_window_returns_none() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![slot(30, 0.50)];
        state.update_prices(today, vec![]);
        assert!(state.cheapest_within(Duration::ZERO).is_none());
    }

    fn slot_with_level(offset_mins: i64, spot_sek: f64, level: PriceLevel) -> PricePoint {
        let mut p = slot(offset_mins, spot_sek);
        p.level = Some(level);
        p
    }

    #[test]
    fn test_cheapest_run_within_empty_state() {
        let state = PriceState::new("SE3".to_string());
        let result = state.cheapest_run_within(Duration::from_hours(8), Duration::from_mins(30));
        assert!(result.is_none());
    }

    #[test]
    fn test_cheapest_run_within_finds_min_avg_run() {
        // Two contiguous spans of 30 minutes each.
        //   Run A: [0.20, 0.40] -> weighted avg 0.30
        //   Run B: [0.10, 0.10] -> weighted avg 0.10   <-- cheapest
        //   Isolated singleton at 0.05 (only 15min available -> doesn't qualify)
        let state = PriceState::new("SE3".to_string());
        let mut today = Vec::new();
        today.extend(make_run(30, &[0.20, 0.40])); // 30..45, 45..60
        today.extend(make_run(120, &[0.10, 0.10])); // 120..135, 135..150
        today.extend(make_run(240, &[0.05])); // single 15-min slot -> insufficient
        state.update_prices(today, vec![]);

        let pick = state
            .cheapest_run_within(Duration::from_hours(8), Duration::from_mins(30))
            .unwrap();
        assert_float_eq(pick.spot_sek, 0.10, "run-B start should win");
    }

    #[test]
    fn test_cheapest_run_within_skips_non_contiguous_runs() {
        // Two cheap slots with a 30-min gap — they cannot form a contiguous
        // 30-min run.
        let state = PriceState::new("SE3".to_string());
        let today = vec![
            slot(30, 0.10), // 30..45
            slot(75, 0.10), // 75..90 — 30-min gap
        ];
        state.update_prices(today, vec![]);
        let result = state.cheapest_run_within(Duration::from_hours(8), Duration::from_mins(30));
        assert!(
            result.is_none(),
            "non-contiguous cheap slots must not combine"
        );
    }

    #[test]
    fn test_cheapest_run_within_rejects_two_cheap_slots_split_by_expensive() {
        // Three contiguous slots: [cheap, expensive, cheap]. The two cheap
        // slots are NOT adjacent, so they cannot form a 30-min cheap run.
        // Both qualifying 30-min runs (slots [0..1] and [1..2]) include the
        // expensive middle slot and have the same weighted avg 0.55. The
        // helper's strict `<` tie-break keeps the first run found, so the
        // returned run-start is slot[0] with spot_sek 0.10.
        let state = PriceState::new("SE3".to_string());
        let today = make_run(30, &[0.10, 1.00, 0.10]);
        state.update_prices(today, vec![]);
        let pick = state
            .cheapest_run_within(Duration::from_hours(8), Duration::from_mins(30))
            .unwrap();
        assert_float_eq(pick.spot_sek, 0.10, "first contiguous run wins on tie");
    }

    #[test]
    fn test_cheapest_run_within_returns_none_when_no_run_fits_window() {
        // Only one 15-min slot available — a requested 30-min run cannot fit.
        let state = PriceState::new("SE3".to_string());
        let today = make_run(30, &[0.20]);
        state.update_prices(today, vec![]);
        let result = state.cheapest_run_within(Duration::from_hours(8), Duration::from_mins(30));
        assert!(
            result.is_none(),
            "single 15-min slot cannot host 30-min run"
        );
    }

    #[test]
    fn test_cheapest_run_within_handles_wider_slot() {
        let state = PriceState::new("SE3".to_string());
        let start = chrono::Utc::now() + chrono::Duration::minutes(30);
        let end = start + chrono::Duration::minutes(60);
        let today = vec![PricePoint::from_spot(
            start.to_rfc3339(),
            end.to_rfc3339(),
            0.15,
            0.0,
            0.0,
        )];
        state.update_prices(today, vec![]);
        let pick = state
            .cheapest_run_within(Duration::from_hours(8), Duration::from_mins(30))
            .unwrap();
        assert_float_eq(pick.spot_sek, 0.15, "wider slot satisfies run on its own");
    }

    #[test]
    fn test_cheapest_run_within_excludes_past_slot_starts() {
        // Even with a contiguous future run available, a past slot must not
        // be returned as the run start.
        let state = PriceState::new("SE3".to_string());
        let mut today = vec![slot(-60, 0.05)]; // very cheap, but in the past
        today.extend(make_run(30, &[0.20, 0.30])); // contiguous future run
        state.update_prices(today, vec![]);
        let pick = state
            .cheapest_run_within(Duration::from_hours(8), Duration::from_mins(30))
            .unwrap();
        assert_float_eq(
            pick.spot_sek,
            0.20,
            "run must start at first future slot, not the past one",
        );
    }

    #[test]
    fn cheapest_run_ending_by_picks_run_finishing_at_deadline() {
        // Eight contiguous 15-min slots from now+60 to now+180; the last two
        // are the cheapest. The cheapest 30-min run ends exactly at now+180.
        let state = PriceState::new("SE3".to_string());
        state.update_prices(
            make_run(60, &[0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.1, 0.1]),
            vec![],
        );
        // Deadline captured after the slots, so it sits at/just past the run
        // end — the boundary is inclusive.
        let deadline = SystemTime::now() + Duration::from_hours(3);
        let pick = state
            .cheapest_run_ending_by(deadline, Duration::from_mins(130), Duration::from_mins(30))
            .expect("a run finishing by the deadline exists");
        assert_float_eq(pick.spot_sek, 0.1, "cheapest run ending at deadline wins");
    }

    #[test]
    fn cheapest_run_ending_by_excludes_run_finishing_after_deadline() {
        // Same slots, but the deadline lands before the cheap run finishes, so
        // it must fall back to a costlier run that does finish in time.
        let state = PriceState::new("SE3".to_string());
        state.update_prices(
            make_run(60, &[0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.1, 0.1]),
            vec![],
        );
        let deadline = SystemTime::now() + Duration::from_mins(170);
        let pick = state
            .cheapest_run_ending_by(deadline, Duration::from_mins(130), Duration::from_mins(30))
            .expect("an earlier run still fits");
        assert_float_eq(
            pick.spot_sek,
            0.5,
            "cheap run ends after deadline, so it is excluded",
        );
    }

    #[test]
    fn cheapest_run_ending_by_excludes_run_starting_before_lead() {
        // Cheap run is available but starts before deadline - max_lead, so no
        // run qualifies inside the lead window.
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(0, &[0.1, 0.1, 0.1, 0.1]), vec![]);
        let deadline = SystemTime::now() + Duration::from_hours(3);
        let result = state.cheapest_run_ending_by(
            deadline,
            Duration::from_mins(30),
            Duration::from_mins(30),
        );
        assert!(result.is_none(), "run starts before the lead window");
    }

    #[test]
    fn cheapest_run_ending_by_none_when_truncated_by_data_edge() {
        // Only two slots (30 min) but a 60-min run is required — the run can't
        // accumulate its full length, so nothing qualifies.
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.1, 0.1]), vec![]);
        let deadline = SystemTime::now() + Duration::from_mins(200);
        let result = state.cheapest_run_ending_by(
            deadline,
            Duration::from_hours(3),
            Duration::from_hours(1),
        );
        assert!(result.is_none(), "run truncated by end of price data");
    }

    #[test]
    fn cheapest_run_ending_by_none_when_deadline_in_past() {
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.1, 0.1]), vec![]);
        let deadline = SystemTime::now() - Duration::from_hours(1);
        let result = state.cheapest_run_ending_by(
            deadline,
            Duration::from_hours(2),
            Duration::from_mins(30),
        );
        assert!(result.is_none(), "deadline already passed");
    }

    #[test]
    fn cheapest_run_ending_by_none_with_no_price_data() {
        let state = PriceState::new("SE3".to_string());
        let deadline = SystemTime::now() + Duration::from_hours(2);
        let result = state.cheapest_run_ending_by(
            deadline,
            Duration::from_hours(2),
            Duration::from_mins(30),
        );
        assert!(result.is_none(), "no price data → no run");
    }

    #[test]
    fn cheapest_runs_within_returns_non_overlapping_ranked() {
        let state = PriceState::new("SE3".to_string());
        let mut today = make_run(60, &[0.30, 0.30]); // run A avg 0.30
        today.extend(make_run(180, &[0.10, 0.10])); // run B avg 0.10 (cheapest)
        today.extend(make_run(300, &[0.20, 0.20])); // run C avg 0.20
        state.update_prices(today, vec![]);

        let runs = state.cheapest_runs_within(Duration::from_hours(12), Duration::from_mins(30), 6);
        assert_eq!(runs.len(), 3);
        assert_float_eq(runs[0].avg_spot_sek, 0.10, "cheapest first");
        assert_float_eq(runs[1].avg_spot_sek, 0.20, "second cheapest");
        assert_float_eq(runs[2].avg_spot_sek, 0.30, "third");
        assert!(runs.iter().all(|r| !r.starts_at.is_empty()));
    }

    #[test]
    fn cheapest_runs_within_collapses_overlaps_and_caps_k() {
        let state = PriceState::new("SE3".to_string());
        let today = make_run(30, &[0.10, 0.10, 0.10, 0.10, 0.10, 0.10, 0.10, 0.10]);
        state.update_prices(today, vec![]);

        let runs = state.cheapest_runs_within(Duration::from_hours(12), Duration::from_mins(30), 6);
        assert_eq!(runs.len(), 4); // 120 min / 30 min = 4 disjoint runs
        for w in runs.windows(2) {
            assert!(w[0].ends_at <= w[1].starts_at, "runs must be disjoint");
        }
    }

    #[test]
    fn cheapest_runs_within_empty_when_no_prices() {
        let state = PriceState::new("SE3".to_string());
        let runs = state.cheapest_runs_within(Duration::from_hours(12), Duration::from_mins(30), 6);
        assert!(runs.is_empty());
    }

    #[test]
    fn cheapest_runs_within_caps_at_k() {
        let state = PriceState::new("SE3".to_string());
        // 7 disjoint 30-min runs (14 contiguous 15-min slots); request k=3.
        let today = make_run(30, &[0.10; 14]);
        state.update_prices(today, vec![]);
        let runs = state.cheapest_runs_within(Duration::from_hours(12), Duration::from_mins(30), 3);
        assert_eq!(runs.len(), 3, "k cap must be respected");
    }

    #[test]
    fn cheapest_runs_within_skips_cheaper_overlapping_run() {
        let state = PriceState::new("SE3".to_string());
        // Contiguous block where the two cheapest 30-min runs overlap each
        // other: [60-90]=avg 0.055 and [75-105]=avg 0.065.
        let mut today = make_run(60, &[0.05, 0.06, 0.07]);
        // Plus a disjoint, more expensive run: [180-210]=avg 0.20.
        today.extend(make_run(180, &[0.20, 0.20]));
        state.update_prices(today, vec![]);
        let runs = state.cheapest_runs_within(Duration::from_hours(12), Duration::from_mins(30), 6);
        // Greedy picks [60-90]=0.055, skips the overlapping [75-105]=0.065,
        // then keeps the disjoint 0.20 run.
        assert_eq!(runs.len(), 2);
        assert_float_eq(runs[0].avg_spot_sek, 0.055, "cheapest non-overlap first");
        assert_float_eq(
            runs[1].avg_spot_sek,
            0.20,
            "overlapping 0.065 skipped for disjoint 0.20",
        );
    }

    #[test]
    fn test_cheap_window_end_no_data() {
        let state = PriceState::new("SE3".to_string());
        assert!(state.cheap_window_end(Duration::from_hours(8)).is_none());
    }

    #[test]
    fn test_cheap_window_end_all_cheap_returns_window_end() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![
            slot_with_level(15, 0.10, PriceLevel::VeryCheap),
            slot_with_level(45, 0.12, PriceLevel::Cheap),
            slot_with_level(120, 0.11, PriceLevel::VeryCheap),
        ];
        state.update_prices(today, vec![]);
        let window = Duration::from_hours(4);
        let result = state.cheap_window_end(window).unwrap();
        // Should be ~now + 4h. Allow small slack for test execution time.
        let now = SystemTime::now();
        let expected_max = now + window + Duration::from_secs(5);
        let expected_min = now + window - Duration::from_secs(5);
        assert!(result >= expected_min && result <= expected_max);
    }

    #[test]
    fn test_cheap_window_end_transition_returns_first_normal() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![
            slot_with_level(15, 0.10, PriceLevel::VeryCheap),
            slot_with_level(45, 0.12, PriceLevel::Cheap),
            slot_with_level(75, 0.50, PriceLevel::Normal),
            slot_with_level(105, 0.80, PriceLevel::Expensive),
        ];
        state.update_prices(today, vec![]);
        let result = state.cheap_window_end(Duration::from_hours(4)).unwrap();
        // Should be the start of the +75min slot.
        let target = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(75));
        let diff = result
            .duration_since(target)
            .or_else(|e| Ok::<_, ()>(e.duration()))
            .unwrap();
        assert!(diff < Duration::from_secs(5), "diff was {diff:?}");
    }

    #[test]
    fn test_cheap_window_end_immediate_normal_returns_first_slot() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![
            slot_with_level(15, 0.50, PriceLevel::Normal),
            slot_with_level(45, 0.60, PriceLevel::Expensive),
        ];
        state.update_prices(today, vec![]);
        let result = state.cheap_window_end(Duration::from_hours(4)).unwrap();
        let target = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(15));
        let diff = result
            .duration_since(target)
            .or_else(|e| Ok::<_, ()>(e.duration()))
            .unwrap();
        assert!(diff < Duration::from_secs(5), "diff was {diff:?}");
    }

    #[test]
    fn test_cheap_window_end_unknown_level_treated_as_not_cheap() {
        // A slot with no level is treated as "not cheap": we cannot prove it's
        // cheap, and the safer interpretation is to end the buffer mode early.
        let state = PriceState::new("SE3".to_string());
        let today = vec![
            slot_with_level(15, 0.10, PriceLevel::VeryCheap),
            slot(45, 0.30), // level = None
        ];
        state.update_prices(today, vec![]);
        let result = state.cheap_window_end(Duration::from_hours(4)).unwrap();
        let target = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(45));
        let diff = result
            .duration_since(target)
            .or_else(|e| Ok::<_, ()>(e.duration()))
            .unwrap();
        assert!(diff < Duration::from_secs(5), "diff was {diff:?}");
    }

    #[test]
    fn resolve_serves_today_when_today_covers_now() {
        let state = PriceState::new("SE3".to_string());
        // Bucket spans now-30m .. now+30m -> covers now.
        let today = make_run(-30, &[0.5, 0.5, 0.5, 0.5]);
        let tomorrow = make_run(30, &[0.9, 0.9]);
        state.update_prices(today, tomorrow);
        let (t, tm) = state.resolve_served_prices().expect("today covers now");
        assert_eq!(t.len(), 4, "today served as-is");
        assert_eq!(tm.len(), 2, "tomorrow served as-is");
        assert_float_eq(t[0].spot_sek, 0.5, "today first slot");
    }

    #[test]
    fn resolve_promotes_tomorrow_when_today_is_stale() {
        // today bucket entirely in the past (loop hasn't rotated at midnight);
        // tomorrow bucket is the real today and spans now.
        let state = PriceState::new("SE3".to_string());
        let today = make_run(-300, &[1.0, 1.0]); // spans -300..-270, all past
        let tomorrow = make_run(-15, &[0.2, 0.2, 0.2]); // spans -15..+30, covers now
        state.update_prices(today, tomorrow);
        let (t, tm) = state.resolve_served_prices().expect("tomorrow promoted");
        assert_float_eq(t[0].spot_sek, 0.2, "promoted today == old tomorrow");
        assert!(tm.is_empty(), "no genuine next-day bucket after promotion");
    }

    #[test]
    fn resolve_returns_none_when_no_bucket_covers_now() {
        let state = PriceState::new("SE3".to_string());
        let today = make_run(-300, &[1.0, 1.0]); // past
        let tomorrow = make_run(180, &[0.3, 0.3]); // future, does not cover now
        state.update_prices(today, tomorrow);
        assert!(state.resolve_served_prices().is_none());
    }

    #[test]
    fn resolve_returns_none_for_empty_state() {
        let state = PriceState::new("SE3".to_string());
        assert!(state.resolve_served_prices().is_none());
    }
}
