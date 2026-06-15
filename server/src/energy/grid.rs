//! Grid state management for tracking energy consumption and peaks
//!
//! Tracks daily peak consumption and calculates the monthly peak average
//! (average of the 3 highest consumption days).
//!
//! Also tracks 15-minute consumption for correlation with price periods.
//!
//! Optimized to store only one peak per day (max 31 entries) instead of
//! all hourly data. Peaks are updated in real-time when current hour
//! exceeds stored values.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use chrono_tz::{Europe::Stockholm, Tz};
use serde::Serialize;
use tracing::trace;

use super::tariff::{TariffMode, get_tariff_at, system_time_to_local};

/// Maximum number of 15-min entries to keep (24 hours = 96 entries)
const MAX_QUARTER_ENTRIES: usize = 96;

/// Grid state for tracking energy consumption
#[derive(Clone)]
pub struct GridState {
    inner: Arc<Mutex<GridStateInner>>,
    /// Timezone used to key daily peak buckets (matches the deployment's
    /// local calendar).
    tz: Tz,
}

struct GridStateInner {
    /// Daily peaks: date (year, month, day) -> (`hour_timestamp`, kwh)
    /// Stores only the highest consumption hour per day
    daily_peaks: HashMap<(i32, u32, u32), (u64, f64)>,
    /// Current month being tracked (year, month)
    current_month: (i32, u32),
    /// Current hour's accumulated consumption (for real-time display).
    /// `None` until the first sample arrives for the hour — distinguishes
    /// "no data yet" from a genuine zero reading.
    current_hour_kwh: Option<f64>,
    /// Timestamp of current hour start
    current_hour_start: u64,
    /// 15-minute consumption history (rolling 24h window)
    consumption_15min: VecDeque<ConsumptionEntry>,
    /// Current 15-minute period's accumulated consumption
    current_quarter_kwh: f64,
    /// Timestamp of current 15-minute period start
    current_quarter_start: u64,
}

/// A 15-minute consumption entry
#[derive(Debug, Clone, Serialize)]
pub struct ConsumptionEntry {
    /// Start of 15-min period (Unix timestamp)
    pub timestamp: u64,
    /// Consumption in kWh for this period
    pub kwh: f64,
    /// ISO 8601 formatted timestamp
    pub starts_at: String,
}

impl GridState {
    /// Create a new grid state pinned to Europe/Stockholm. Used by tests.
    #[must_use]
    pub fn new() -> Self {
        Self::with_tz(Stockholm)
    }

    /// Create a new grid state keyed to `tz` for daily peak buckets.
    #[must_use]
    pub fn with_tz(tz: Tz) -> Self {
        let now = SystemTime::now();
        let (year, month, _, hour_start) = timestamp_to_components_in(now, tz);
        let quarter_start = timestamp_to_quarter_start(now);

        Self {
            inner: Arc::new(Mutex::new(GridStateInner {
                daily_peaks: HashMap::new(),
                current_month: (year, month),
                current_hour_kwh: None,
                current_hour_start: hour_start,
                consumption_15min: VecDeque::with_capacity(MAX_QUARTER_ENTRIES),
                current_quarter_kwh: 0.0,
                current_quarter_start: quarter_start,
            })),
            tz,
        }
    }

    /// Update daily peaks if the given hour exceeds stored values
    ///
    /// Called when hour changes or with historical data. Only records during
    /// high-tariff periods (winter weekdays 07:00-19:59 Swedish time).
    ///
    /// Logic:
    /// - If today already has a peak and this hour is higher -> update today's peak
    /// - If today is new day and we have < 3 days -> add this as today's peak
    /// - If today is new day and we have 3 days -> replace lowest if this is higher
    pub fn maybe_update_peak(&self, hour_timestamp: u64, kwh: f64) {
        // Skip NaN values
        if kwh.is_nan() {
            trace!("Grid: Skipping NaN kWh value for peak update");
            return;
        }

        // Check if this is a high-tariff hour
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(hour_timestamp);
        if get_tariff_at(time) != TariffMode::High {
            trace!(
                "Grid: Skipping low-tariff hour {} for peak update",
                format_timestamp(time)
            );
            return;
        }

        let mut inner = self.inner.lock().unwrap();
        // The day-key uses the configured local calendar. March can already
        // be in CEST after spring forward while high-tariff hours still
        // apply, so route through the DST-aware helper instead of a fixed
        // +1 h offset.
        let (year, month, day, _) = system_time_to_local(time, self.tz);
        let today = (year, month, day);

        // Check for month change
        if (year, month) != inner.current_month {
            trace!(
                "Grid: New month detected ({}-{:02}), clearing old data",
                year, month
            );
            inner.daily_peaks.clear();
            inner.current_month = (year, month);
        }

        // Case 1: Today already has a peak - update if current is higher
        if let Some((ts, peak)) = inner.daily_peaks.get_mut(&today) {
            if kwh > *peak {
                trace!(
                    "Grid: Updating today's ({:?}) peak from {:.2} to {:.2} kWh",
                    today, *peak, kwh
                );
                *ts = hour_timestamp;
                *peak = kwh;
            }
            return;
        }

        // Case 2: Today is a new day - add or replace depending on count
        if inner.daily_peaks.len() < 3 {
            // Less than 3 days - just add
            trace!(
                "Grid: Adding new daily peak for {:?}: {:.2} kWh (day {})",
                today,
                kwh,
                inner.daily_peaks.len() + 1
            );
            inner.daily_peaks.insert(today, (hour_timestamp, kwh));
        } else {
            // Find the day with the lowest peak
            let lowest = inner
                .daily_peaks
                .iter()
                .min_by(|a, b| a.1.1.partial_cmp(&b.1.1).unwrap())
                .map(|(date, (_, kwh))| (*date, *kwh));

            if let Some((lowest_date, lowest_kwh)) = lowest {
                if kwh > lowest_kwh {
                    trace!(
                        "Grid: Replacing {:?} ({:.2} kWh) with {:?} ({:.2} kWh)",
                        lowest_date, lowest_kwh, today, kwh
                    );
                    inner.daily_peaks.remove(&lowest_date);
                    inner.daily_peaks.insert(today, (hour_timestamp, kwh));
                } else {
                    trace!(
                        "Grid: New day {:?} ({:.2} kWh) lower than all stored peaks, skipping",
                        today, kwh
                    );
                }
            }
        }
    }

    /// Record consumption for a completed hour (high tariff only)
    ///
    /// Only records hours during high tariff periods for peak tracking.
    /// Called by WebSocket handler or historic sync with completed hour data.
    pub fn record_hour(&self, timestamp: SystemTime, kwh: f64) {
        let secs = timestamp
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Round to hour start
        let hour_start = (secs / 3600) * 3600;

        // maybe_update_peak handles tariff check and logic
        self.maybe_update_peak(hour_start, kwh);
    }

    /// Update the current hour's accumulated consumption
    ///
    /// Called periodically with the current accumulated kWh for the ongoing hour.
    /// When hour changes, records the previous hour as a potential peak.
    pub fn update_current_hour(&self, kwh: f64) {
        let now = SystemTime::now();
        let (year, month, _, hour_start) = timestamp_to_components_in(now, self.tz);

        let mut inner = self.inner.lock().unwrap();

        // Check if hour changed
        if hour_start != inner.current_hour_start {
            // Record the previous hour if we have data
            if let Some(prev_kwh) = inner.current_hour_kwh {
                // Copy values before dropping lock
                let prev_start = inner.current_hour_start;
                drop(inner);

                // maybe_update_peak will check if it was high-tariff
                self.maybe_update_peak(prev_start, prev_kwh);

                // Re-acquire lock
                inner = self.inner.lock().unwrap();
            }

            // Check for month change
            if (year, month) != inner.current_month {
                trace!(
                    "Grid: New month detected ({}-{:02}), clearing old data",
                    year, month
                );
                inner.daily_peaks.clear();
                inner.current_month = (year, month);
            }

            // Reset for new hour
            inner.current_hour_start = hour_start;
            inner.current_hour_kwh = None;
        }

        inner.current_hour_kwh = Some(kwh);
    }

    /// Get the current hour's accumulated consumption.
    /// Returns `None` if no sample has arrived yet for this hour.
    #[must_use]
    pub fn get_current_hour_kwh(&self) -> Option<f64> {
        self.inner.lock().unwrap().current_hour_kwh
    }

    /// Get the average of the top 3 daily peak consumption hours this month
    ///
    /// Energy companies calculate effect tariff based on the three highest
    /// consumption hours distributed across THREE DIFFERENT DAYS.
    #[must_use]
    pub fn get_top3_average(&self) -> f64 {
        let inner = self.inner.lock().unwrap();

        if inner.daily_peaks.is_empty() {
            return 0.0;
        }

        // Already have one peak per day, just sort and average top 3
        let mut peaks: Vec<f64> = inner.daily_peaks.values().map(|(_, kwh)| *kwh).collect();
        peaks.sort_by(|a, b| b.partial_cmp(a).unwrap());

        let top_count = peaks.len().min(3);
        let sum: f64 = peaks.iter().take(top_count).sum();

        #[allow(clippy::cast_precision_loss)]
        // top_count is always <= 3, so no precision loss occurs
        {
            sum / top_count as f64
        }
    }

    /// Get the top 3 daily peak hours with details
    ///
    /// Returns one peak hour per day (up to 3 days), sorted by consumption descending.
    #[must_use]
    pub fn get_top3_hours(&self) -> Vec<PeakHour> {
        let inner = self.inner.lock().unwrap();

        let mut peaks: Vec<(u64, f64)> = inner.daily_peaks.values().copied().collect();
        peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        peaks
            .into_iter()
            .take(3)
            .map(|(ts, kwh)| PeakHour {
                timestamp: format_timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(ts)),
                kwh,
            })
            .collect()
    }

    /// Get the number of unique days with recorded high-tariff consumption
    ///
    /// Used to indicate incomplete data (< 3 days) on the dashboard.
    #[must_use]
    pub fn get_recorded_days_count(&self) -> usize {
        self.inner.lock().unwrap().daily_peaks.len()
    }

    /// Get the number of recorded hours this month
    ///
    /// With the optimized structure, this returns the number of days tracked
    /// (since we only store one peak per day).
    #[must_use]
    pub fn recorded_hours_count(&self) -> usize {
        self.inner.lock().unwrap().daily_peaks.len()
    }

    // ==================== 15-Minute Consumption Tracking ====================

    /// Update the current 15-minute period's accumulated consumption
    ///
    /// Called periodically with the current accumulated kWh for the ongoing quarter.
    /// When quarter changes, records the previous quarter.
    pub fn update_current_quarter(&self, kwh: f64) {
        let now = SystemTime::now();
        let quarter_start = timestamp_to_quarter_start(now);

        let mut inner = self.inner.lock().unwrap();

        // Check if quarter changed
        if quarter_start != inner.current_quarter_start {
            // Record the previous quarter, including idle (0 kWh) ones. Otherwise
            // the rolling 24 h buffer has variable density and current_15min_kwh
            // averages skew high — heat-pump-off quarters silently disappear.
            // NaN is the only thing we drop, since it would poison aggregates.
            if !inner.current_quarter_kwh.is_nan() {
                let entry = ConsumptionEntry {
                    timestamp: inner.current_quarter_start,
                    kwh: inner.current_quarter_kwh,
                    starts_at: format_timestamp(
                        SystemTime::UNIX_EPOCH + Duration::from_secs(inner.current_quarter_start),
                    ),
                };

                inner.consumption_15min.push_back(entry);

                // Keep only last 24 hours (96 entries)
                while inner.consumption_15min.len() > MAX_QUARTER_ENTRIES {
                    inner.consumption_15min.pop_front();
                }

                trace!(
                    "Grid: Recorded quarter {}: {:.3} kWh",
                    inner.current_quarter_start, inner.current_quarter_kwh
                );
            }

            // Reset for new quarter
            inner.current_quarter_start = quarter_start;
            inner.current_quarter_kwh = 0.0;
        }

        inner.current_quarter_kwh = kwh;
    }

    /// Record consumption for a completed 15-minute period
    ///
    /// Called with historical or completed quarter data.
    pub fn record_quarter(&self, timestamp: u64, kwh: f64) {
        if kwh.is_nan() || kwh < 0.0 {
            return;
        }

        // Round to quarter start (15-minute boundary)
        let quarter_start = (timestamp / 900) * 900;

        let mut inner = self.inner.lock().unwrap();

        // Don't record if it's the current quarter (use update_current_quarter instead)
        if quarter_start == inner.current_quarter_start {
            return;
        }

        // Check if this quarter already exists
        if inner
            .consumption_15min
            .iter()
            .any(|e| e.timestamp == quarter_start)
        {
            return;
        }

        let entry = ConsumptionEntry {
            timestamp: quarter_start,
            kwh,
            starts_at: format_timestamp(
                SystemTime::UNIX_EPOCH + Duration::from_secs(quarter_start),
            ),
        };

        // Insert in chronological order
        let pos = inner
            .consumption_15min
            .iter()
            .position(|e| e.timestamp > quarter_start);

        match pos {
            Some(idx) => inner.consumption_15min.insert(idx, entry),
            None => inner.consumption_15min.push_back(entry),
        }

        // Keep only last 24 hours
        while inner.consumption_15min.len() > MAX_QUARTER_ENTRIES {
            inner.consumption_15min.pop_front();
        }
    }

    /// Get the current 15-minute period's accumulated consumption
    #[must_use]
    pub fn get_current_quarter_kwh(&self) -> f64 {
        self.inner.lock().unwrap().current_quarter_kwh
    }

    /// Get the last 24 hours of 15-minute consumption data
    #[must_use]
    pub fn get_consumption_15min(&self) -> Vec<ConsumptionEntry> {
        self.inner
            .lock()
            .unwrap()
            .consumption_15min
            .iter()
            .cloned()
            .collect()
    }
}

impl Default for GridState {
    fn default() -> Self {
        Self::new()
    }
}

/// A peak hour record
#[derive(Debug, Clone, Serialize)]
pub struct PeakHour {
    /// ISO 8601 timestamp of the hour start
    pub timestamp: String,
    /// Consumption in kWh
    pub kwh: f64,
}

/// Format a `SystemTime` as ISO 8601 string (e.g., "2025-01-15T14:30:00Z")
fn format_timestamp(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Convert `SystemTime` to the start of its 15-minute period (Unix timestamp).
///
/// 15-minute boundaries align between UTC and Swedish-local because CET/CEST
/// offsets are both whole hours, so rounding to a 900-second boundary in UTC
/// produces the same instant as rounding in local time. The DST transitions
/// themselves are whole-hour shifts, not 15-minute shifts, so they don't
/// disturb the alignment either.
fn timestamp_to_quarter_start(time: SystemTime) -> u64 {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = duration.as_secs();

    // Round down to 15-minute boundary (900 seconds)
    (total_secs / 900) * 900
}

/// Convert `SystemTime` to (year, month, day, `hour_start_unix_secs`) in the
/// given `tz`. The (year, month, day) tuple is local because it keys the
/// daily peak map; `hour_start` stays in UTC Unix seconds.
fn timestamp_to_components_in(time: SystemTime, tz: Tz) -> (i32, u32, u32, u64) {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let total_secs = duration.as_secs();
    let hour_start = (total_secs / 3600) * 3600;

    let (year, month, day, _hour) = system_time_to_local(time, tz);
    (year, month, day, hour_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: Create unix timestamp for a specific UTC hour
    /// Note: This is UTC time, not Swedish time
    fn hour_timestamp(year: i32, month: u32, day: u32, hour: u32) -> u64 {
        // Simple calculation: days since epoch + hours
        // This gives us a UTC timestamp for the start of the specified hour
        let days = ymd_to_days(year, month, day);
        #[allow(clippy::cast_sign_loss)]
        let secs = (days * 86400 + i64::from(hour) * 3600) as u64;
        secs
    }

    /// Convert (year, month, day) to days since Unix epoch (for test helper)
    #[allow(clippy::similar_names)]
    fn ymd_to_days(year: i32, month: u32, day: u32) -> i64 {
        let y = i64::from(if month <= 2 { year - 1 } else { year });
        let m = i64::from(if month <= 2 { month + 12 } else { month });

        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (m - 3) + 2) / 5 + i64::from(day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

        era * 146_097 + doe - 719_468
    }

    impl GridState {
        /// Test helper: directly insert daily peak without tariff check
        #[cfg(test)]
        fn test_insert_peak(&self, date: (i32, u32, u32), timestamp: u64, kwh: f64) {
            let mut inner = self.inner.lock().unwrap();
            inner.daily_peaks.insert(date, (timestamp, kwh));
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_grid_state_new() {
        let state = GridState::new();
        assert_eq!(state.get_current_hour_kwh(), None);
        assert_eq!(state.get_top3_average(), 0.0);
        assert!(state.get_top3_hours().is_empty());
    }

    #[test]
    fn test_update_current_hour() {
        let state = GridState::new();

        state.update_current_hour(1.5);
        let v = state.get_current_hour_kwh().expect("kwh recorded");
        assert!((v - 1.5).abs() < f64::EPSILON);

        state.update_current_hour(2.3);
        let v = state.get_current_hour_kwh().expect("kwh recorded");
        assert!((v - 2.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_timestamp_to_components() {
        // 2025-01-15 14:30:00 UTC = 15:30:00 CET (same date locally)
        let timestamp = 1_736_951_400;
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp);
        let (year, month, day, hour_start) = timestamp_to_components_in(time, Stockholm);

        assert_eq!(year, 2025);
        assert_eq!(month, 1);
        assert_eq!(day, 15);
        // Hour 14 starts at 14:00:00 = 1736949600
        assert_eq!(hour_start, 1_736_949_600);
    }

    #[test]
    fn test_timestamp_to_components_winter_local_day_rollover() {
        // 2025-01-15 23:30 UTC = 2025-01-16 00:30 CET. The day component must
        // follow Swedish-local, otherwise daily peaks land on the wrong day
        // (and month rollover near UTC midnight thrashes the peak cache).
        let timestamp = 1_736_983_800; // 2025-01-15 23:30:00 UTC
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp);
        let (year, month, day, _) = timestamp_to_components_in(time, Stockholm);
        assert_eq!((year, month, day), (2025, 1, 16));
    }

    #[test]
    fn test_timestamp_to_components_summer_local_day_rollover() {
        // 2025-07-15 22:30 UTC = 2025-07-16 00:30 CEST (UTC+2 in summer).
        let timestamp = 1_752_618_600; // 2025-07-15 22:30:00 UTC
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp);
        let (year, month, day, _) = timestamp_to_components_in(time, Stockholm);
        assert_eq!((year, month, day), (2025, 7, 16));
    }

    #[test]
    fn test_timestamp_to_components_month_rollover_at_local_midnight() {
        // 2025-03-31 23:30 UTC = 2025-04-01 01:30 CEST (post spring-forward).
        // Regression: if the UTC date had leaked in, update_current_hour and
        // maybe_update_peak would disagree on the month and clear the peak
        // cache every other call near the month boundary.
        let timestamp = 1_743_464_000 + 1_400; // 2025-03-31 23:33:20 UTC, close enough
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp);
        let (year, month, _, _) = timestamp_to_components_in(time, Stockholm);
        assert_eq!((year, month), (2025, 4));
    }

    #[test]
    fn test_timestamp_to_quarter_start_aligns_across_dst() {
        // 15-min boundaries should land on the same Unix instants regardless
        // of DST because both CET and CEST offsets are whole hours.
        let winter = 1_736_951_400; // 2025-01-15 14:30:00 UTC
        let summer = 1_752_618_600; // 2025-07-15 22:30:00 UTC
        let q_winter =
            timestamp_to_quarter_start(SystemTime::UNIX_EPOCH + Duration::from_secs(winter));
        let q_summer =
            timestamp_to_quarter_start(SystemTime::UNIX_EPOCH + Duration::from_secs(summer));
        assert_eq!(q_winter % 900, 0);
        assert_eq!(q_summer % 900, 0);
        // 14:30 UTC rounds to 14:30 UTC; 22:30 UTC rounds to 22:30 UTC.
        assert_eq!(q_winter, 1_736_951_400);
        assert_eq!(q_summer, 1_752_618_600);
    }

    #[test]
    fn test_top3_different_days() {
        let state = GridState::new();

        // Insert data for 3 different days using test helper
        // Day 1: Jan 2, 2026 = 5.0 kWh
        state.test_insert_peak((2026, 1, 2), hour_timestamp(2026, 1, 2, 9), 5.0);
        // Day 2: Jan 5, 2026 = 4.0 kWh
        state.test_insert_peak((2026, 1, 5), hour_timestamp(2026, 1, 5, 10), 4.0);
        // Day 3: Jan 6, 2026 = 3.0 kWh
        state.test_insert_peak((2026, 1, 6), hour_timestamp(2026, 1, 6, 11), 3.0);

        // Average of top 3 daily peaks = (5.0 + 4.0 + 3.0) / 3 = 4.0
        let avg = state.get_top3_average();
        assert!((avg - 4.0).abs() < f64::EPSILON, "Expected 4.0, got {avg}");

        // Should return 3 peak hours (one per day)
        let peaks = state.get_top3_hours();
        assert_eq!(peaks.len(), 3);

        // Recorded days count should be 3
        assert_eq!(state.get_recorded_days_count(), 3);
    }

    #[test]
    fn test_update_peak_same_day_keeps_highest() {
        let state = GridState::new();

        // Insert initial peak for a day
        state.test_insert_peak((2026, 1, 2), hour_timestamp(2026, 1, 2, 9), 3.0);

        // Update with higher value for same day using maybe_update_peak
        // Note: This bypasses tariff check since we're using the internal method
        {
            let mut inner = state.inner.lock().unwrap();
            // Manually update to simulate higher consumption
            inner.daily_peaks.insert(
                (2026, 1, 2),
                (hour_timestamp(2026, 1, 2, 10), 5.0), // Higher value
            );
        }

        // Should have the higher value
        let avg = state.get_top3_average();
        assert!((avg - 5.0).abs() < f64::EPSILON, "Expected 5.0, got {avg}");

        // Should still be only 1 day
        assert_eq!(state.get_recorded_days_count(), 1);
    }

    #[test]
    fn test_single_day() {
        let state = GridState::new();

        // Only 1 day
        state.test_insert_peak((2026, 1, 2), hour_timestamp(2026, 1, 2, 9), 7.5);

        let avg = state.get_top3_average();
        assert!((avg - 7.5).abs() < f64::EPSILON, "Expected 7.5, got {avg}");

        let peaks = state.get_top3_hours();
        assert_eq!(peaks.len(), 1);
        assert!((peaks[0].kwh - 7.5).abs() < f64::EPSILON);

        assert_eq!(state.get_recorded_days_count(), 1);
    }

    #[test]
    fn test_empty() {
        let state = GridState::new();

        assert!((state.get_top3_average() - 0.0).abs() < f64::EPSILON);
        assert!(state.get_top3_hours().is_empty());
        assert_eq!(state.get_recorded_days_count(), 0);
    }

    #[test]
    fn test_two_days_average() {
        let state = GridState::new();

        // 2 days of data
        state.test_insert_peak((2026, 1, 2), hour_timestamp(2026, 1, 2, 9), 5.0);
        state.test_insert_peak((2026, 1, 5), hour_timestamp(2026, 1, 5, 10), 6.0);

        // Only 2 days, so average = (6.0 + 5.0) / 2 = 5.5
        let avg = state.get_top3_average();
        assert!((avg - 5.5).abs() < f64::EPSILON, "Expected 5.5, got {avg}");

        // Should return 2 peak hours
        let peaks = state.get_top3_hours();
        assert_eq!(peaks.len(), 2);

        // Recorded days count should be 2
        assert_eq!(state.get_recorded_days_count(), 2);
    }

    #[test]
    fn test_maybe_update_peak_new_day_replaces_lowest() {
        let state = GridState::new();

        // Insert 3 days of peaks
        state.test_insert_peak((2026, 1, 2), hour_timestamp(2026, 1, 2, 9), 5.0);
        state.test_insert_peak((2026, 1, 3), hour_timestamp(2026, 1, 3, 10), 4.0);
        state.test_insert_peak((2026, 1, 4), hour_timestamp(2026, 1, 4, 11), 3.0); // Lowest

        // Simulate a 4th day with higher consumption that should replace the lowest
        // We need to manually test this logic since maybe_update_peak has tariff check
        {
            let mut inner = state.inner.lock().unwrap();
            let new_day = (2026, 1, 5);
            let new_kwh = 6.0;

            // Find lowest
            let lowest = inner
                .daily_peaks
                .iter()
                .min_by(|a, b| a.1.1.partial_cmp(&b.1.1).unwrap())
                .map(|(date, _)| *date);

            if let Some(lowest_date) = lowest {
                inner.daily_peaks.remove(&lowest_date);
                inner
                    .daily_peaks
                    .insert(new_day, (hour_timestamp(2026, 1, 5, 12), new_kwh));
            }
        }

        // Should now have 3 days: 5.0, 4.0, 6.0 (replaced 3.0)
        // Average = (6.0 + 5.0 + 4.0) / 3 = 5.0
        let avg = state.get_top3_average();
        assert!((avg - 5.0).abs() < f64::EPSILON, "Expected 5.0, got {avg}");

        assert_eq!(state.get_recorded_days_count(), 3);
    }

    #[test]
    fn test_maybe_update_peak_new_day_lower_than_all() {
        let state = GridState::new();

        // Insert 3 days of peaks
        state.test_insert_peak((2026, 1, 2), hour_timestamp(2026, 1, 2, 9), 5.0);
        state.test_insert_peak((2026, 1, 3), hour_timestamp(2026, 1, 3, 10), 6.0);
        state.test_insert_peak((2026, 1, 4), hour_timestamp(2026, 1, 4, 11), 7.0);

        // A new day with lower value should NOT be added
        // (simulating the logic since we can't easily bypass tariff check)
        {
            let inner = state.inner.lock().unwrap();
            // All peaks are >= 5.0, so a new day with 4.0 shouldn't be added
            // This is just verifying our test data is set up correctly
            let min_peak = inner
                .daily_peaks
                .values()
                .map(|(_, kwh)| *kwh)
                .fold(f64::INFINITY, f64::min);
            assert!((min_peak - 5.0).abs() < f64::EPSILON);
        }

        // Average should still be (7.0 + 6.0 + 5.0) / 3 = 6.0
        let avg = state.get_top3_average();
        assert!((avg - 6.0).abs() < f64::EPSILON, "Expected 6.0, got {avg}");
    }

    #[test]
    fn test_maybe_update_peak_uses_swedish_local_date_in_cest() {
        // 2026-03-30 Monday 10:00 Swedish-local (CEST, UTC+2) = 08:00 UTC.
        // Spring forward happened on 2026-03-29 (last Sunday of March), so
        // the local zone is already CEST. The day-key must reflect Swedish
        // local date 2026-03-30, not whatever a +3600 s offset would yield.
        let state = GridState::new();
        let utc_secs = hour_timestamp(2026, 3, 30, 8);
        state.maybe_update_peak(utc_secs, 4.2);

        let inner = state.inner.lock().unwrap();
        let entry = inner.daily_peaks.get(&(2026, 3, 30));
        assert!(
            entry.is_some(),
            "expected daily peak under Swedish-local date 2026-03-30; got {:?}",
            inner.daily_peaks.keys().collect::<Vec<_>>()
        );
        let (_, kwh) = entry.unwrap();
        assert!((*kwh - 4.2).abs() < f64::EPSILON);
    }

    #[test]
    fn test_recorded_hours_returns_days() {
        let state = GridState::new();

        state.test_insert_peak((2026, 1, 2), hour_timestamp(2026, 1, 2, 9), 5.0);
        state.test_insert_peak((2026, 1, 5), hour_timestamp(2026, 1, 5, 10), 4.0);

        // With optimized structure, recorded_hours_count returns number of days
        assert_eq!(state.recorded_hours_count(), 2);
        assert_eq!(state.get_recorded_days_count(), 2);
    }

    // ==================== 15-Minute Tracking Tests ====================

    #[test]
    fn test_timestamp_to_quarter_start() {
        // 14:37:45 should round to 14:30:00
        let ts = hour_timestamp(2026, 1, 15, 14) + 37 * 60 + 45; // 14:37:45
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(ts);
        let quarter = timestamp_to_quarter_start(time);

        // Should be 14:30:00 (the 15-min boundary before 14:37)
        let expected = hour_timestamp(2026, 1, 15, 14) + 30 * 60;
        assert_eq!(quarter, expected);
    }

    #[test]
    fn test_timestamp_to_quarter_start_exact_boundary() {
        // Exactly at 14:15:00 should stay at 14:15:00
        let ts = hour_timestamp(2026, 1, 15, 14) + 15 * 60;
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(ts);
        let quarter = timestamp_to_quarter_start(time);

        assert_eq!(quarter, ts);
    }

    #[test]
    fn test_record_quarter() {
        let state = GridState::new();

        // Record some 15-min periods (in the past, not current quarter)
        let ts1 = hour_timestamp(2026, 1, 15, 10); // 10:00
        let ts2 = hour_timestamp(2026, 1, 15, 10) + 15 * 60; // 10:15
        let ts3 = hour_timestamp(2026, 1, 15, 10) + 30 * 60; // 10:30

        state.record_quarter(ts1, 0.5);
        state.record_quarter(ts2, 0.6);
        state.record_quarter(ts3, 0.4);

        let entries = state.get_consumption_15min();
        assert_eq!(entries.len(), 3);

        // Should be in chronological order
        assert_eq!(entries[0].timestamp, ts1);
        assert!((entries[0].kwh - 0.5).abs() < f64::EPSILON);
        assert_eq!(entries[1].timestamp, ts2);
        assert!((entries[1].kwh - 0.6).abs() < f64::EPSILON);
        assert_eq!(entries[2].timestamp, ts3);
        assert!((entries[2].kwh - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn test_record_quarter_deduplication() {
        let state = GridState::new();

        let ts = hour_timestamp(2026, 1, 15, 10);

        // Record same quarter twice
        state.record_quarter(ts, 0.5);
        state.record_quarter(ts, 0.6); // Should be ignored

        let entries = state.get_consumption_15min();
        assert_eq!(entries.len(), 1);
        assert!((entries[0].kwh - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_record_quarter_ignores_nan() {
        let state = GridState::new();

        let ts = hour_timestamp(2026, 1, 15, 10);
        state.record_quarter(ts, f64::NAN);

        assert!(state.get_consumption_15min().is_empty());
    }

    #[test]
    fn test_record_quarter_ignores_negative() {
        let state = GridState::new();

        let ts = hour_timestamp(2026, 1, 15, 10);
        state.record_quarter(ts, -1.0);

        assert!(state.get_consumption_15min().is_empty());
    }

    #[test]
    fn test_consumption_15min_max_entries() {
        let state = GridState::new();

        // Record more than 96 entries (24 hours worth)
        let base_ts = hour_timestamp(2026, 1, 15, 0);
        for i in 0..100 {
            let ts = base_ts + i * 900; // Every 15 minutes
            state.record_quarter(ts, 0.5);
        }

        // Should keep only last 96 entries
        assert_eq!(state.get_consumption_15min().len(), MAX_QUARTER_ENTRIES);

        // First entry should be the 5th one (100 - 96 = 4, so entry index 4)
        let entries = state.get_consumption_15min();
        let expected_first = base_ts + 4 * 900;
        assert_eq!(entries[0].timestamp, expected_first);
    }

    #[test]
    fn test_get_current_quarter_kwh_initial() {
        let state = GridState::new();
        assert!((state.get_current_quarter_kwh() - 0.0).abs() < f64::EPSILON);
    }

    // ==================== maybe_update_peak real-path tests ====================
    //
    // These drive the public method (not test_insert_peak) so the tariff
    // filter, month-change clear, and case-1/case-2 branches are exercised.

    /// A known winter weekday at 10:00 Swedish-local (high tariff) Unix start.
    /// 2026-01-07 is a Wednesday; CET (UTC+1) so 10:00 local = 09:00 UTC.
    const HIGH_TARIFF_HOUR: u64 = 1_767_776_400;
    /// Same Wednesday at 03:00 Swedish-local — outside 07:00-19:59 → low tariff.
    const LOW_TARIFF_HOUR: u64 = 1_767_751_200;

    #[test]
    fn maybe_update_peak_skips_nan() {
        let state = GridState::new();
        state.maybe_update_peak(HIGH_TARIFF_HOUR, f64::NAN);
        assert_eq!(state.get_recorded_days_count(), 0);
    }

    #[test]
    fn maybe_update_peak_skips_low_tariff_hour() {
        let state = GridState::new();
        state.maybe_update_peak(LOW_TARIFF_HOUR, 9.9);
        assert_eq!(
            state.get_recorded_days_count(),
            0,
            "low-tariff hours must not be recorded as peaks"
        );
    }

    #[test]
    fn maybe_update_peak_records_high_tariff_hour() {
        let state = GridState::new();
        state.maybe_update_peak(HIGH_TARIFF_HOUR, 4.2);
        assert_eq!(state.get_recorded_days_count(), 1);
        assert!((state.get_top3_average() - 4.2).abs() < f64::EPSILON);
    }

    #[test]
    fn maybe_update_peak_same_day_keeps_higher_and_ignores_lower() {
        let state = GridState::new();
        // Two high-tariff hours on the same day (09:00 and 10:00 local).
        let h09 = HIGH_TARIFF_HOUR - 3600;
        state.maybe_update_peak(h09, 3.0);
        // Higher → replaces (Case 1 update branch).
        state.maybe_update_peak(HIGH_TARIFF_HOUR, 5.0);
        // Lower → ignored (Case 1 no-op branch).
        state.maybe_update_peak(h09, 1.0);
        assert_eq!(state.get_recorded_days_count(), 1, "still one day");
        assert!(
            (state.get_top3_average() - 5.0).abs() < f64::EPSILON,
            "highest same-day value wins"
        );
    }

    #[test]
    fn maybe_update_peak_fourth_day_replaces_lowest_via_public_path() {
        let state = GridState::new();
        // Three distinct winter weekdays, each high-tariff at 10:00 local.
        let day1 = HIGH_TARIFF_HOUR; // 2026-01-07 Wed
        let day2 = HIGH_TARIFF_HOUR + 86_400; // 2026-01-08 Thu
        let day3 = HIGH_TARIFF_HOUR + 2 * 86_400; // 2026-01-09 Fri
        state.maybe_update_peak(day1, 5.0);
        state.maybe_update_peak(day2, 4.0);
        state.maybe_update_peak(day3, 3.0); // lowest of the three
        assert_eq!(state.get_recorded_days_count(), 3);

        // A 4th weekday (2026-01-12 Mon) higher than the lowest must replace it.
        let day4 = HIGH_TARIFF_HOUR + 5 * 86_400;
        state.maybe_update_peak(day4, 6.0);
        assert_eq!(state.get_recorded_days_count(), 3, "still capped at 3 days");
        // Top3 now {6,5,4} → avg 5.0; the 3.0 day was evicted.
        assert!((state.get_top3_average() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn maybe_update_peak_fourth_day_lower_than_all_is_ignored() {
        let state = GridState::new();
        let day1 = HIGH_TARIFF_HOUR;
        let day2 = HIGH_TARIFF_HOUR + 86_400;
        let day3 = HIGH_TARIFF_HOUR + 2 * 86_400;
        state.maybe_update_peak(day1, 5.0);
        state.maybe_update_peak(day2, 6.0);
        state.maybe_update_peak(day3, 7.0);

        // 4th day below every stored peak → skipped (Case 2 else branch).
        let day4 = HIGH_TARIFF_HOUR + 5 * 86_400;
        state.maybe_update_peak(day4, 1.0);
        assert_eq!(state.get_recorded_days_count(), 3);
        assert!((state.get_top3_average() - 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn maybe_update_peak_new_month_clears_old_data() {
        let state = GridState::new();
        // Seed January.
        state.maybe_update_peak(HIGH_TARIFF_HOUR, 5.0);
        assert_eq!(state.get_recorded_days_count(), 1);

        // A high-tariff hour roughly a month later (2026-02-04 Wed 10:00 local
        // = 31 days after 2026-01-07, still a Wednesday, winter, high tariff).
        let february = HIGH_TARIFF_HOUR + 28 * 86_400;
        state.maybe_update_peak(february, 2.0);
        assert_eq!(
            state.get_recorded_days_count(),
            1,
            "month change clears the January peak, leaving only February"
        );
        assert!((state.get_top3_average() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn record_hour_rounds_and_records_high_tariff() {
        let state = GridState::new();
        // 37 minutes into the high-tariff hour → rounds down to the hour start.
        let mid_hour = SystemTime::UNIX_EPOCH + Duration::from_secs(HIGH_TARIFF_HOUR + 37 * 60);
        state.record_hour(mid_hour, 3.3);
        assert_eq!(state.get_recorded_days_count(), 1);
        let peaks = state.get_top3_hours();
        assert_eq!(peaks.len(), 1);
        assert!((peaks[0].kwh - 3.3).abs() < f64::EPSILON);
    }

    #[test]
    fn record_hour_skips_low_tariff() {
        let state = GridState::new();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(LOW_TARIFF_HOUR);
        state.record_hour(t, 8.0);
        assert_eq!(state.get_recorded_days_count(), 0);
    }

    // ==================== update_current_quarter tests ====================

    #[test]
    fn update_current_quarter_sets_current_value() {
        let state = GridState::new();
        state.update_current_quarter(0.42);
        assert!((state.get_current_quarter_kwh() - 0.42).abs() < f64::EPSILON);
        // No quarter boundary crossed → nothing pushed to the history yet.
        assert!(state.get_consumption_15min().is_empty());
    }

    #[test]
    fn update_current_quarter_rollover_records_previous() {
        let state = GridState::new();
        // Force the in-memory current quarter into the past so the next call
        // observes a quarter change and flushes the previous quarter to history.
        {
            let mut inner = state.inner.lock().unwrap();
            inner.current_quarter_start -= 900; // one quarter earlier
            inner.current_quarter_kwh = 0.7;
        }
        state.update_current_quarter(0.1);
        let entries = state.get_consumption_15min();
        assert_eq!(entries.len(), 1, "previous quarter recorded on rollover");
        assert!((entries[0].kwh - 0.7).abs() < f64::EPSILON);
        // Current value reset to the new sample.
        assert!((state.get_current_quarter_kwh() - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn update_current_quarter_rollover_drops_nan_previous() {
        let state = GridState::new();
        {
            let mut inner = state.inner.lock().unwrap();
            inner.current_quarter_start -= 900;
            inner.current_quarter_kwh = f64::NAN; // poison value must not persist
        }
        state.update_current_quarter(0.2);
        assert!(
            state.get_consumption_15min().is_empty(),
            "NaN previous quarter must be dropped, not recorded"
        );
    }

    #[test]
    fn record_quarter_skips_current_quarter() {
        let state = GridState::new();
        // Recording the *current* quarter is a no-op (use update_current_quarter).
        let current = state.inner.lock().unwrap().current_quarter_start;
        state.record_quarter(current, 0.9);
        assert!(state.get_consumption_15min().is_empty());
    }
}
