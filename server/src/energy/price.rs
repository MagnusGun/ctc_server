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

/// Statistics for a set of prices
#[derive(Clone, Debug, Serialize)]
pub struct PriceStatistics {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
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
    /// Used for `SmartGrid` auto-resume: when the user blocks the heater, this picks
    /// the lowest-price 15-minute slot inside the configured horizon (e.g. 8h).
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
mod tests {
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
            1.0, 0.0, 0.0,
        )];
        let snapshot_b = vec![PricePoint::from_spot(
            "2026-01-04T00:00:00Z".to_string(),
            "2026-01-04T00:15:00Z".to_string(),
            2.0, 0.0, 0.0,
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

    fn slot(offset_mins: i64, spot_sek: f64) -> PricePoint {
        let start = chrono::Utc::now() + chrono::Duration::minutes(offset_mins);
        let end = start + chrono::Duration::minutes(15);
        PricePoint::from_spot(start.to_rfc3339(), end.to_rfc3339(), spot_sek, 0.0, 0.0)
    }

    #[test]
    fn test_cheapest_within_empty_state() {
        let state = PriceState::new("SE3".to_string());
        assert!(
            state
                .cheapest_within(Duration::from_secs(8 * 3600))
                .is_none()
        );
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
        let pick = state
            .cheapest_within(Duration::from_secs(8 * 3600))
            .unwrap();
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
        let pick = state
            .cheapest_within(Duration::from_secs(8 * 3600))
            .unwrap();
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
        let pick = state
            .cheapest_within(Duration::from_secs(8 * 3600))
            .unwrap();
        assert_float_eq(pick.spot_sek, 0.80, "stays inside window");
    }

    #[test]
    fn test_cheapest_within_spans_today_and_tomorrow() {
        let state = PriceState::new("SE3".to_string());
        let today = vec![slot(30, 1.20), slot(60, 0.90)];
        let tomorrow = vec![slot(180, 0.25), slot(240, 0.75)];
        state.update_prices(today, tomorrow);
        let pick = state
            .cheapest_within(Duration::from_secs(8 * 3600))
            .unwrap();
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
    fn test_cheap_window_end_no_data() {
        let state = PriceState::new("SE3".to_string());
        assert!(
            state
                .cheap_window_end(Duration::from_secs(8 * 3600))
                .is_none()
        );
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
        let window = Duration::from_secs(4 * 3600);
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
        let result = state
            .cheap_window_end(Duration::from_secs(4 * 3600))
            .unwrap();
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
        let result = state
            .cheap_window_end(Duration::from_secs(4 * 3600))
            .unwrap();
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
        let result = state
            .cheap_window_end(Duration::from_secs(4 * 3600))
            .unwrap();
        let target = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(45));
        let diff = result
            .duration_since(target)
            .or_else(|e| Ok::<_, ()>(e.duration()))
            .unwrap();
        assert!(diff < Duration::from_secs(5), "diff was {diff:?}");
    }
}
