//! Heat pump statistics tracking
//!
//! Tracks compressor cycle statistics including:
//! - Cycle times (min/max/avg)
//! - Compressor starts per time window (hour/day/week/month/year)
//! - Operating hours per time window
//! - Outdoor temperature correlation for each cycle

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use serde::Serialize;
use tracing::trace;

/// Maximum number of cycles to keep in history
const MAX_CYCLE_HISTORY: usize = 1000;

/// Maximum number of daily records to keep (1 year)
const MAX_DAILY_HISTORY: usize = 365;

/// Heat pump statistics state (thread-safe wrapper)
#[derive(Clone)]
pub struct HeatPumpStats {
    inner: Arc<Mutex<HeatPumpStatsInner>>,
}

/// Internal state for heat pump statistics
struct HeatPumpStatsInner {
    /// Whether we've received at least one status update
    initialized: bool,

    /// Whether we observed the start of the current ON cycle
    /// (false if server started with heater already ON)
    observed_cycle_start: bool,

    /// Current compressor state (true = ON, false = OFF)
    compressor_on: bool,

    /// Timestamp when current state began
    state_started_at: SystemTime,

    /// Outdoor temperature when current cycle started (if ON)
    cycle_start_temp: Option<f32>,

    /// Completed cycle history (for cycle time stats)
    cycle_history: VecDeque<CycleRecord>,

    /// Daily aggregated history (for charts)
    daily_history: VecDeque<DailyRecord>,

    /// Rolling counters for compressor starts
    starts_this_hour: u32,
    starts_this_day: u32,
    starts_this_week: u32,
    starts_this_month: u32,
    starts_this_year: u32,

    /// Rolling counters for operating time (seconds)
    operating_secs_this_hour: u64,
    operating_secs_this_day: u64,
    operating_secs_this_week: u64,
    operating_secs_this_month: u64,
    operating_secs_this_year: u64,

    /// Window boundary timestamps (Unix seconds)
    current_hour_start: u64,
    current_day_start: u64,
    current_week_start: u64,
    current_month_start: u64,
    current_year_start: u64,

    /// Current day's accumulating stats (for daily history)
    current_day_date: (i32, u32, u32), // (year, month, day)
    current_day_starts: u32,
    current_day_operating_secs: u64,
    current_day_temp_sum: f64,
    current_day_temp_count: u32,

    /// Statistics tracking start time
    tracking_started: SystemTime,

    /// Total compressor starts since tracking began
    total_starts: u64,

    /// Total operating time since tracking began (seconds)
    total_operating_secs: u64,
}

/// A single completed compressor cycle
#[derive(Debug, Clone, Serialize)]
pub struct CycleRecord {
    /// ISO 8601 timestamp when cycle started
    pub timestamp: String,
    /// Cycle duration in seconds
    pub duration_secs: u64,
    /// Outdoor temperature at cycle start (Celsius)
    pub outdoor_temp_c: Option<f32>,
}

/// Daily aggregated statistics
#[derive(Debug, Clone, Serialize)]
pub struct DailyRecord {
    /// Date string (YYYY-MM-DD)
    pub date: String,
    /// Number of compressor starts this day
    pub starts: u32,
    /// Operating hours this day
    pub operating_hours: f64,
    /// Average outdoor temperature this day (Celsius)
    pub avg_outdoor_temp_c: Option<f64>,
}

/// Cycle statistics summary
#[derive(Debug, Clone, Serialize)]
pub struct CycleStats {
    /// Minimum cycle duration in seconds
    pub min_secs: u64,
    /// Maximum cycle duration in seconds
    pub max_secs: u64,
    /// Average cycle duration in seconds
    pub avg_secs: f64,
    /// Number of completed cycles in history
    pub cycle_count: usize,
}

/// Starts per time window
#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_field_names)] // "this_" prefix is semantic and matches JSON API
pub struct StartsPerWindow {
    pub this_hour: u32,
    pub this_day: u32,
    pub this_week: u32,
    pub this_month: u32,
    pub this_year: u32,
}

/// Operating hours per time window
#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_field_names)] // "this_" prefix is semantic and matches JSON API
pub struct OperatingHoursPerWindow {
    pub this_hour: f64,
    pub this_day: f64,
    pub this_week: f64,
    pub this_month: f64,
    pub this_year: f64,
}

/// Tracking metadata
#[derive(Debug, Clone, Serialize)]
pub struct TrackingInfo {
    /// ISO 8601 timestamp when tracking started
    pub started_at: String,
    /// Total tracking duration in hours
    pub tracking_hours: f64,
    /// Total compressor starts
    pub total_starts: u64,
    /// Total operating hours
    pub total_operating_hours: f64,
}

/// Complete statistics response
#[derive(Debug, Clone, Serialize)]
pub struct HeatPumpStatsResponse {
    /// Current compressor state
    pub compressor_on: bool,
    /// Duration of current state in seconds
    pub current_state_duration_secs: u64,
    /// Cycle time statistics (None if no completed cycles)
    pub cycle_stats: Option<CycleStats>,
    /// Starts per time window
    pub starts: StartsPerWindow,
    /// Operating hours per time window
    pub operating_hours: OperatingHoursPerWindow,
    /// Tracking metadata
    pub tracking: TrackingInfo,
}

/// History response for charts
#[derive(Debug, Clone, Serialize)]
pub struct HeatPumpHistoryResponse {
    /// Recent completed cycles
    pub cycles: Vec<CycleRecord>,
    /// Daily aggregated statistics
    pub daily: Vec<DailyRecord>,
}

impl HeatPumpStats {
    /// Create a new heat pump statistics tracker
    #[must_use]
    pub fn new() -> Self {
        let now = SystemTime::now();
        let now_secs = system_time_to_secs(now);
        let (year, month, day) = secs_to_ymd(now_secs);

        Self {
            inner: Arc::new(Mutex::new(HeatPumpStatsInner {
                initialized: false,
                observed_cycle_start: false,
                compressor_on: false,
                state_started_at: now,
                cycle_start_temp: None,
                cycle_history: VecDeque::with_capacity(MAX_CYCLE_HISTORY),
                daily_history: VecDeque::with_capacity(MAX_DAILY_HISTORY),

                starts_this_hour: 0,
                starts_this_day: 0,
                starts_this_week: 0,
                starts_this_month: 0,
                starts_this_year: 0,

                operating_secs_this_hour: 0,
                operating_secs_this_day: 0,
                operating_secs_this_week: 0,
                operating_secs_this_month: 0,
                operating_secs_this_year: 0,

                current_hour_start: hour_start(now_secs),
                current_day_start: day_start(now_secs),
                current_week_start: week_start(now_secs),
                current_month_start: month_start(year, month),
                current_year_start: year_start(year),

                current_day_date: (year, month, day),
                current_day_starts: 0,
                current_day_operating_secs: 0,
                current_day_temp_sum: 0.0,
                current_day_temp_count: 0,

                tracking_started: now,
                total_starts: 0,
                total_operating_secs: 0,
            })),
        }
    }

    /// Update compressor state based on polled status code
    ///
    /// # Arguments
    /// * `status_code` - Heat pump status code (3, 4, 5 = ON; others = OFF)
    /// * `outdoor_temp` - Current outdoor temperature (Celsius)
    pub fn update_state(&self, status_code: u16, outdoor_temp: Option<f32>) {
        let is_on = matches!(status_code, 3..=5);
        let now = SystemTime::now();
        let now_secs = system_time_to_secs(now);

        let mut inner = self.inner.lock().unwrap();

        // Handle first update - just sync state without counting anything
        // This ensures we don't count partial cycles if server starts with heater ON
        if !inner.initialized {
            inner.initialized = true;
            inner.compressor_on = is_on;
            inner.state_started_at = now;
            // observed_cycle_start stays false - we didn't observe the actual start
            return;
        }

        // Check for window rollovers first
        inner.check_window_rollovers(now_secs);

        // Record outdoor temp for daily average (if available)
        if let Some(temp) = outdoor_temp {
            inner.current_day_temp_sum += f64::from(temp);
            inner.current_day_temp_count += 1;
        }

        // State transition: ON -> OFF (cycle complete)
        if inner.compressor_on && !is_on {
            // Only record cycle if we observed the actual start (not a partial cycle)
            if inner.observed_cycle_start {
                let duration = now
                    .duration_since(inner.state_started_at)
                    .unwrap_or_default();
                let duration_secs = duration.as_secs();

                trace!(
                    "Compressor cycle complete: {} seconds, temp: {:?}",
                    duration_secs, inner.cycle_start_temp
                );

                // Record the completed cycle
                let cycle = CycleRecord {
                    timestamp: format_timestamp(inner.state_started_at),
                    duration_secs,
                    outdoor_temp_c: inner.cycle_start_temp,
                };

                inner.cycle_history.push_back(cycle);
                if inner.cycle_history.len() > MAX_CYCLE_HISTORY {
                    inner.cycle_history.pop_front();
                }

                // Update operating time counters
                inner.operating_secs_this_hour += duration_secs;
                inner.operating_secs_this_day += duration_secs;
                inner.operating_secs_this_week += duration_secs;
                inner.operating_secs_this_month += duration_secs;
                inner.operating_secs_this_year += duration_secs;
                inner.total_operating_secs += duration_secs;
                inner.current_day_operating_secs += duration_secs;
            }

            // Reset for next cycle
            inner.observed_cycle_start = false;
        }

        // State transition: OFF -> ON (cycle start)
        if !inner.compressor_on && is_on {
            trace!("Compressor cycle started, temp: {:?}", outdoor_temp);

            // Mark that we observed this start (for valid cycle tracking)
            inner.observed_cycle_start = true;

            inner.starts_this_hour += 1;
            inner.starts_this_day += 1;
            inner.starts_this_week += 1;
            inner.starts_this_month += 1;
            inner.starts_this_year += 1;
            inner.total_starts += 1;
            inner.current_day_starts += 1;

            inner.cycle_start_temp = outdoor_temp;
        }

        // Update state if changed
        if inner.compressor_on != is_on {
            inner.compressor_on = is_on;
            inner.state_started_at = now;
        }
    }

    /// Get the summary statistics
    #[must_use]
    pub fn get_summary(&self) -> HeatPumpStatsResponse {
        let inner = self.inner.lock().unwrap();
        let now = SystemTime::now();

        let current_state_duration = now
            .duration_since(inner.state_started_at)
            .unwrap_or_default()
            .as_secs();

        let tracking_duration = now
            .duration_since(inner.tracking_started)
            .unwrap_or_default();

        // Calculate cycle stats if we have any cycles
        let cycle_stats = if inner.cycle_history.is_empty() {
            None
        } else {
            let mut min_secs = u64::MAX;
            let mut max_secs = 0u64;
            let mut sum_secs = 0u64;

            for cycle in &inner.cycle_history {
                min_secs = min_secs.min(cycle.duration_secs);
                max_secs = max_secs.max(cycle.duration_secs);
                sum_secs += cycle.duration_secs;
            }

            #[allow(clippy::cast_precision_loss)]
            let avg_secs = sum_secs as f64 / inner.cycle_history.len() as f64;

            Some(CycleStats {
                min_secs,
                max_secs,
                avg_secs,
                cycle_count: inner.cycle_history.len(),
            })
        };

        #[allow(clippy::cast_precision_loss)]
        HeatPumpStatsResponse {
            compressor_on: inner.compressor_on,
            current_state_duration_secs: current_state_duration,
            cycle_stats,
            starts: StartsPerWindow {
                this_hour: inner.starts_this_hour,
                this_day: inner.starts_this_day,
                this_week: inner.starts_this_week,
                this_month: inner.starts_this_month,
                this_year: inner.starts_this_year,
            },
            operating_hours: OperatingHoursPerWindow {
                this_hour: inner.operating_secs_this_hour as f64 / 3600.0,
                this_day: inner.operating_secs_this_day as f64 / 3600.0,
                this_week: inner.operating_secs_this_week as f64 / 3600.0,
                this_month: inner.operating_secs_this_month as f64 / 3600.0,
                this_year: inner.operating_secs_this_year as f64 / 3600.0,
            },
            tracking: TrackingInfo {
                started_at: format_timestamp(inner.tracking_started),
                tracking_hours: tracking_duration.as_secs_f64() / 3600.0,
                total_starts: inner.total_starts,
                total_operating_hours: inner.total_operating_secs as f64 / 3600.0,
            },
        }
    }

    /// Get history data for charts
    ///
    /// # Arguments
    /// * `days` - Number of days of daily history to return (max 365)
    #[must_use]
    pub fn get_history(&self, days: usize) -> HeatPumpHistoryResponse {
        let inner = self.inner.lock().unwrap();

        let days = days.min(MAX_DAILY_HISTORY);

        // Get recent cycles (last 100 or so for the chart)
        let cycles: Vec<CycleRecord> = inner
            .cycle_history
            .iter()
            .rev()
            .take(100)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        // Get daily history
        let daily: Vec<DailyRecord> = inner
            .daily_history
            .iter()
            .rev()
            .take(days)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        HeatPumpHistoryResponse { cycles, daily }
    }
}

impl Default for HeatPumpStats {
    fn default() -> Self {
        Self::new()
    }
}

impl HeatPumpStatsInner {
    /// Check and handle window rollovers (hour, day, week, month, year)
    fn check_window_rollovers(&mut self, now_secs: u64) {
        let (year, month, day) = secs_to_ymd(now_secs);

        // Hour rollover
        let new_hour_start = hour_start(now_secs);
        if new_hour_start != self.current_hour_start {
            trace!("Hour rollover detected");
            self.starts_this_hour = 0;
            self.operating_secs_this_hour = 0;
            self.current_hour_start = new_hour_start;
        }

        // Day rollover
        let new_day_start = day_start(now_secs);
        if new_day_start != self.current_day_start {
            trace!("Day rollover detected");

            // Archive the previous day's stats
            self.archive_current_day();

            self.starts_this_day = 0;
            self.operating_secs_this_day = 0;
            self.current_day_start = new_day_start;

            // Reset current day tracking
            self.current_day_date = (year, month, day);
            self.current_day_starts = 0;
            self.current_day_operating_secs = 0;
            self.current_day_temp_sum = 0.0;
            self.current_day_temp_count = 0;
        }

        // Week rollover (Monday midnight)
        let new_week_start = week_start(now_secs);
        if new_week_start != self.current_week_start {
            trace!("Week rollover detected");
            self.starts_this_week = 0;
            self.operating_secs_this_week = 0;
            self.current_week_start = new_week_start;
        }

        // Month rollover
        let new_month_start = month_start(year, month);
        if new_month_start != self.current_month_start {
            trace!("Month rollover detected");
            self.starts_this_month = 0;
            self.operating_secs_this_month = 0;
            self.current_month_start = new_month_start;
        }

        // Year rollover
        let new_year_start = year_start(year);
        if new_year_start != self.current_year_start {
            trace!("Year rollover detected");
            self.starts_this_year = 0;
            self.operating_secs_this_year = 0;
            self.current_year_start = new_year_start;
        }
    }

    /// Archive the current day's stats to daily history
    fn archive_current_day(&mut self) {
        // Only archive if we have some data
        if self.current_day_starts == 0 && self.current_day_operating_secs == 0 {
            return;
        }

        let (year, month, day) = self.current_day_date;
        let date = format!("{year:04}-{month:02}-{day:02}");

        #[allow(clippy::cast_precision_loss)]
        let operating_hours = self.current_day_operating_secs as f64 / 3600.0;

        let avg_temp = if self.current_day_temp_count > 0 {
            Some(self.current_day_temp_sum / f64::from(self.current_day_temp_count))
        } else {
            None
        };

        let record = DailyRecord {
            date,
            starts: self.current_day_starts,
            operating_hours,
            avg_outdoor_temp_c: avg_temp,
        };

        trace!("Archiving daily record: {:?}", record);

        self.daily_history.push_back(record);
        if self.daily_history.len() > MAX_DAILY_HISTORY {
            self.daily_history.pop_front();
        }
    }
}

// ==================== Time Utility Functions ====================

/// Convert `SystemTime` to Unix timestamp (seconds)
fn system_time_to_secs(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Format a `SystemTime` as ISO 8601 string
fn format_timestamp(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Get the start of the hour containing the given timestamp
fn hour_start(secs: u64) -> u64 {
    (secs / 3600) * 3600
}

/// Get the start of the day (UTC midnight) containing the given timestamp
fn day_start(secs: u64) -> u64 {
    (secs / 86400) * 86400
}

/// Get the start of the week (Monday UTC midnight) containing the given timestamp
fn week_start(secs: u64) -> u64 {
    let datetime = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
    let days_since_monday = datetime.weekday().num_days_from_monday();
    let monday = datetime - chrono::Duration::days(i64::from(days_since_monday));
    let monday_midnight = monday.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    #[allow(clippy::cast_sign_loss)]
    {
        monday_midnight.timestamp() as u64
    }
}

/// Get the start of the month containing the given year/month
fn month_start(year: i32, month: u32) -> u64 {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let datetime = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
    #[allow(clippy::cast_sign_loss)]
    {
        datetime.timestamp() as u64
    }
}

/// Get the start of the year
fn year_start(year: i32) -> u64 {
    month_start(year, 1)
}

/// Convert Unix timestamp to (year, month, day)
fn secs_to_ymd(secs: u64) -> (i32, u32, u32) {
    let datetime = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
    (datetime.year(), datetime.month(), datetime.day())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_float_eq(a: f64, b: f64, msg: &str) {
        assert!((a - b).abs() < 1e-6, "{msg}: expected {b}, got {a}");
    }

    #[test]
    fn test_new_stats() {
        let stats = HeatPumpStats::new();
        let summary = stats.get_summary();

        assert!(!summary.compressor_on);
        assert_eq!(summary.starts.this_hour, 0);
        assert_eq!(summary.starts.this_day, 0);
        assert!(summary.cycle_stats.is_none());
    }

    #[test]
    fn test_compressor_start() {
        let stats = HeatPumpStats::new();

        // Initialize with OFF state first
        stats.update_state(0, Some(0.0));

        // Turn on (Heating = 3)
        stats.update_state(3, Some(-5.0));

        let summary = stats.get_summary();
        assert!(summary.compressor_on);
        assert_eq!(summary.starts.this_hour, 1);
        assert_eq!(summary.starts.this_day, 1);
        assert_eq!(summary.tracking.total_starts, 1);
    }

    #[test]
    fn test_compressor_off_no_change() {
        let stats = HeatPumpStats::new();

        // Status 0 = Start Delay (OFF)
        stats.update_state(0, Some(5.0));
        stats.update_state(1, Some(5.0)); // Ready (OFF)
        stats.update_state(2, Some(5.0)); // Wait Flow (OFF)

        let summary = stats.get_summary();
        assert!(!summary.compressor_on);
        assert_eq!(summary.starts.this_hour, 0);
    }

    #[test]
    fn test_compressor_on_states() {
        let stats = HeatPumpStats::new();

        // Initialize with OFF state first
        stats.update_state(0, None);

        // Test all ON states
        stats.update_state(3, None); // Heating
        let s1 = stats.get_summary();
        assert!(s1.compressor_on);
        assert_eq!(s1.starts.this_hour, 1);

        // Reset by going OFF then testing Defrost
        stats.update_state(0, None);
        stats.update_state(4, None); // Defrost
        let s2 = stats.get_summary();
        assert!(s2.compressor_on);
        assert_eq!(s2.starts.this_hour, 2);

        // Reset by going OFF then testing Cooling
        stats.update_state(0, None);
        stats.update_state(5, None); // Cooling
        let s3 = stats.get_summary();
        assert!(s3.compressor_on);
        assert_eq!(s3.starts.this_hour, 3);
    }

    #[test]
    fn test_cycle_recording() {
        let stats = HeatPumpStats::new();

        // Initialize with OFF state first
        stats.update_state(0, Some(0.0));

        // Start compressor
        stats.update_state(3, Some(-10.0));

        // Stop compressor (cycle complete)
        std::thread::sleep(std::time::Duration::from_millis(10));
        stats.update_state(0, Some(-10.0));

        let summary = stats.get_summary();
        assert!(!summary.compressor_on);
        assert!(summary.cycle_stats.is_some());

        let cycle_stats = summary.cycle_stats.unwrap();
        assert_eq!(cycle_stats.cycle_count, 1);
        // min_secs is always >= 0 since it's u64, just verify it's valid
        assert!(cycle_stats.min_secs <= cycle_stats.max_secs);

        // Check history
        let history = stats.get_history(30);
        assert_eq!(history.cycles.len(), 1);
        assert_eq!(history.cycles[0].outdoor_temp_c, Some(-10.0));
    }

    #[test]
    fn test_multiple_cycles() {
        let stats = HeatPumpStats::new();

        // Initialize with OFF state first
        stats.update_state(0, Some(0.0));

        // Cycle 1
        stats.update_state(3, Some(0.0));
        stats.update_state(0, Some(0.0));

        // Cycle 2
        stats.update_state(3, Some(-5.0));
        stats.update_state(0, Some(-5.0));

        // Cycle 3
        stats.update_state(4, Some(-10.0)); // Defrost mode
        stats.update_state(0, Some(-10.0));

        let summary = stats.get_summary();
        assert_eq!(summary.starts.this_hour, 3);
        assert_eq!(summary.tracking.total_starts, 3);

        let cycle_stats = summary.cycle_stats.unwrap();
        assert_eq!(cycle_stats.cycle_count, 3);
    }

    #[test]
    fn test_no_start_increment_when_staying_on() {
        let stats = HeatPumpStats::new();

        // Initialize with OFF state first
        stats.update_state(0, None);

        // Turn on
        stats.update_state(3, None);
        assert_eq!(stats.get_summary().starts.this_hour, 1);

        // Stay on (multiple updates while ON)
        stats.update_state(3, None);
        stats.update_state(3, None);
        stats.update_state(4, None); // Change from Heating to Defrost (still ON)
        stats.update_state(5, None); // Change to Cooling (still ON)

        // Should still be 1 start
        assert_eq!(stats.get_summary().starts.this_hour, 1);
    }

    #[test]
    fn test_hour_start_calculation() {
        // 2026-01-15 14:37:45 UTC
        let ts: u64 = 1_768_487_865;
        let hour = hour_start(ts);

        // Should be 14:00:00 = 1768485600
        assert_eq!(hour, 1_768_485_600);
    }

    #[test]
    fn test_day_start_calculation() {
        // 2026-01-15 14:37:45 UTC
        let ts: u64 = 1_768_487_865;
        let day = day_start(ts);

        // Should be 2026-01-15 00:00:00 UTC = 1768435200
        assert_eq!(day, 1_768_435_200);
    }

    #[test]
    fn test_week_start_calculation() {
        // 2026-01-15 is a Thursday
        // Monday 2026-01-12 00:00:00 UTC = 1768176000
        let ts: u64 = 1_768_487_865; // Thursday 14:37:45
        let week = week_start(ts);

        assert_eq!(week, 1_768_176_000);
    }

    #[test]
    fn test_secs_to_ymd() {
        // 2026-01-15 14:37:45 UTC
        let ts: u64 = 1_768_487_865;
        let (year, month, day) = secs_to_ymd(ts);

        assert_eq!(year, 2026);
        assert_eq!(month, 1);
        assert_eq!(day, 15);
    }

    #[test]
    fn test_month_start() {
        let start = month_start(2026, 1);
        // 2026-01-01 00:00:00 UTC = 1767225600
        assert_eq!(start, 1_767_225_600);
    }

    #[test]
    fn test_year_start() {
        let start = year_start(2026);
        // 2026-01-01 00:00:00 UTC = 1767225600
        assert_eq!(start, 1_767_225_600);
    }

    #[test]
    fn test_operating_hours_conversion() {
        let stats = HeatPumpStats::new();

        // Manually set some operating seconds via internal access
        {
            let mut inner = stats.inner.lock().unwrap();
            inner.operating_secs_this_day = 7200; // 2 hours
        }

        let summary = stats.get_summary();
        assert_float_eq(summary.operating_hours.this_day, 2.0, "Operating hours");
    }

    #[test]
    fn test_history_limits() {
        let stats = HeatPumpStats::new();

        // Create many cycles
        for _ in 0..150 {
            stats.update_state(3, Some(0.0));
            stats.update_state(0, Some(0.0));
        }

        // History should be limited to 100 recent cycles
        let history = stats.get_history(30);
        assert_eq!(history.cycles.len(), 100);
    }

    #[test]
    fn test_default_impl() {
        let stats = HeatPumpStats::default();
        let summary = stats.get_summary();
        assert!(!summary.compressor_on);
    }

    #[test]
    fn test_server_starts_with_heater_on_no_cycle_recorded() {
        let stats = HeatPumpStats::new();

        // First poll: heater already ON (status 3 = Heating)
        stats.update_state(3, Some(-5.0));

        // Should NOT count as a start (we didn't observe it starting)
        let summary = stats.get_summary();
        assert!(summary.compressor_on);
        assert_eq!(summary.starts.this_hour, 0);
        assert_eq!(summary.tracking.total_starts, 0);

        // Heater turns OFF
        stats.update_state(0, Some(-5.0));

        // Should NOT record a cycle (partial cycle)
        let summary = stats.get_summary();
        assert!(!summary.compressor_on);
        assert!(summary.cycle_stats.is_none());
        assert_float_eq(
            summary.operating_hours.this_hour,
            0.0,
            "No operating hours for partial cycle",
        );
    }

    #[test]
    fn test_server_starts_with_heater_on_second_cycle_valid() {
        let stats = HeatPumpStats::new();

        // First poll: heater already ON (partial cycle - should be ignored)
        stats.update_state(3, Some(-5.0));
        stats.update_state(0, Some(-5.0)); // OFF - no cycle recorded

        assert!(stats.get_summary().cycle_stats.is_none());
        assert_eq!(stats.get_summary().starts.this_hour, 0);

        // Now a complete cycle that we fully observe
        stats.update_state(3, Some(-5.0)); // ON - observed start
        stats.update_state(0, Some(-5.0)); // OFF - cycle complete

        let summary = stats.get_summary();
        assert_eq!(summary.starts.this_hour, 1);
        assert_eq!(summary.tracking.total_starts, 1);
        assert!(summary.cycle_stats.is_some());
        assert_eq!(summary.cycle_stats.unwrap().cycle_count, 1);
    }

    #[test]
    fn test_server_starts_with_heater_off_normal_tracking() {
        let stats = HeatPumpStats::new();

        // First poll: heater OFF (normal startup)
        stats.update_state(0, Some(5.0));

        // Complete cycle
        stats.update_state(3, Some(5.0)); // ON - observed start
        stats.update_state(0, Some(5.0)); // OFF - cycle complete

        let summary = stats.get_summary();
        assert_eq!(summary.starts.this_hour, 1);
        assert!(summary.cycle_stats.is_some());
        assert_eq!(summary.cycle_stats.unwrap().cycle_count, 1);
    }

    #[test]
    fn test_partial_cycle_no_operating_time() {
        let stats = HeatPumpStats::new();

        // Server starts with heater ON
        stats.update_state(3, Some(-5.0));

        // Wait a bit (simulated by immediate call)
        stats.update_state(3, Some(-5.0)); // Still ON

        // Turn OFF
        stats.update_state(0, Some(-5.0));

        // No operating time should be recorded for the partial cycle
        let summary = stats.get_summary();
        assert_float_eq(summary.operating_hours.this_hour, 0.0, "No operating hours");
        assert_eq!(summary.tracking.total_starts, 0);
    }
}
