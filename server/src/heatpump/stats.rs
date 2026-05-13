//! Heat pump statistics tracking
//!
//! Tracks compressor cycle statistics including:
//! - Cycle times (min/max/avg)
//! - Compressor starts per time window (hour/day/week/month/year)
//! - Operating hours per time window
//! - Outdoor temperature correlation for each cycle

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use chrono_tz::{Europe::Stockholm, Tz};
use serde::{Deserialize, Serialize};
use tracing::{trace, warn};

use crate::storage::{Accumulators, CycleBlob, DailyBlob, Store};

/// Maximum number of cycles to keep in the in-memory tail (for `cycle_stats`).
/// The store keeps the full history on disk.
const MAX_CYCLE_HISTORY: usize = 1000;

/// Maximum number of daily records `get_history` may return (1 year).
const MAX_DAILY_HISTORY: usize = 365;

/// Heat pump statistics state (thread-safe wrapper)
#[derive(Clone)]
pub struct HeatPumpStats {
    inner: Arc<Mutex<HeatPumpStatsInner>>,
    /// Optional store. `None` disables persistence (used by tests and
    /// in-memory-only runs).
    store: Option<Store>,
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

    /// Completed cycle history kept in memory for `cycle_stats` computation.
    /// Hydrated from `Store::recent_cycles` on boot; mirrored to the store on
    /// every cycle close.
    cycle_history: VecDeque<CycleRecord>,

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

    /// Seconds from the current ON cycle that have already been credited
    /// to previous days' archives because the cycle spans midnight(s).
    /// Subtracted from the total cycle duration when crediting today's
    /// counters at cycle completion, so the total isn't double-counted.
    /// Reset whenever a cycle ends (recorded or discarded).
    current_cycle_credited_secs: u64,

    /// Statistics tracking start time
    tracking_started: SystemTime,

    /// Total compressor starts since tracking began
    total_starts: u64,

    /// Total operating time since tracking began (seconds)
    total_operating_secs: u64,

    /// Timezone used for daily/local-date keying. Defaults to Europe/Stockholm
    /// for back-compat in test constructors; production uses the configured tz.
    tz: Tz,
}

/// A single completed compressor cycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleRecord {
    /// ISO 8601 timestamp when cycle started
    pub timestamp: String,
    /// Cycle duration in seconds
    pub duration_secs: u64,
    /// Outdoor temperature at cycle start (Celsius)
    pub outdoor_temp_c: Option<f32>,
}

/// Daily aggregated statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Create a new in-memory-only heat pump statistics tracker (Stockholm-pinned).
    /// Data is lost on restart. Used by tests and for runs with no store.
    #[must_use]
    pub fn new() -> Self {
        Self::with_tz(Stockholm)
    }

    /// Create a new in-memory-only tracker keyed to `tz`.
    #[must_use]
    pub fn with_tz(tz: Tz) -> Self {
        Self {
            inner: Arc::new(Mutex::new(fresh_inner(SystemTime::now(), tz))),
            store: None,
        }
    }

    /// Create a tracker backed by `store` and keyed to `tz`. Accumulators and
    /// the recent-cycle tail are hydrated from the store; future cycle
    /// completions and day rollovers write back to it (RAM-only — the store's
    /// hourly `flush()` owns durability).
    #[must_use]
    pub fn new_with_store_and_tz(store: Store, tz: Tz) -> Self {
        let mut inner = fresh_inner(SystemTime::now(), tz);

        let acc = store.accumulators();
        // `tracking_started_unix_secs == 0` is the sentinel for "store has
        // never recorded a session" — keep the fresh `now` in that case so
        // tracking_hours starts at 0 instead of 1970.
        if acc.tracking_started_unix_secs > 0 {
            inner.tracking_started =
                UNIX_EPOCH + Duration::from_secs(acc.tracking_started_unix_secs);
        }
        inner.total_starts = acc.total_starts;
        inner.total_operating_secs = acc.total_operating_secs;

        // Hydrate the cycle tail used for cycle_stats. `recent_cycles`
        // returns newest first; reverse for VecDeque chronological order.
        match store.recent_cycles(0, MAX_CYCLE_HISTORY) {
            Ok(cycles) => {
                let mut cycles: Vec<CycleRecord> = cycles
                    .into_iter()
                    .map(|c| CycleRecord {
                        timestamp: c.timestamp,
                        duration_secs: c.duration_secs,
                        outdoor_temp_c: c.outdoor_temp_c,
                    })
                    .collect();
                cycles.reverse();
                inner.cycle_history = cycles.into();
            }
            Err(e) => warn!("Failed to hydrate cycle history from store: {e}"),
        }

        Self {
            inner: Arc::new(Mutex::new(inner)),
            store: Some(store),
        }
    }

    /// Mark the heat-pump status as unknown after a failed Modbus poll.
    ///
    /// Without this, the tracker would carry the last observed `compressor_on`
    /// state across the outage and, on the next ON→OFF transition, credit the
    /// entire gap (including the outage) as operating time. Instead we discard
    /// the in-progress cycle: any pre-midnight credit accrued during the
    /// current cycle is rolled back, and the next successful poll re-syncs
    /// from a clean slate (treated like the first poll after server start).
    pub fn mark_poll_failed(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);

        if inner.compressor_on && inner.observed_cycle_start {
            let credited = inner.current_cycle_credited_secs;
            inner.operating_secs_this_day = inner.operating_secs_this_day.saturating_sub(credited);
            inner.current_day_operating_secs =
                inner.current_day_operating_secs.saturating_sub(credited);
        }

        inner.observed_cycle_start = false;
        inner.current_cycle_credited_secs = 0;
        // Force the next successful poll to be treated as the first poll —
        // matching the "server starts with heater ON" semantics.
        inner.initialized = false;
    }

    /// Update compressor state based on polled status code
    ///
    /// # Arguments
    /// * `status_code` - Heat pump status code (3, 4, 5 = ON; others = OFF)
    /// * `outdoor_temp` - Current outdoor temperature (Celsius)
    #[allow(clippy::too_many_lines)]
    pub fn update_state(&self, status_code: u16, outdoor_temp: Option<f32>) {
        let is_on = matches!(status_code, 3..=5);
        let now = SystemTime::now();
        let now_secs = system_time_to_secs(now);

        // Collected under the lock, acted on after release.
        let mut closed_cycle: Option<(SystemTime, CycleBlob)> = None;
        let mut archived_daily: Option<(u32, DailyBlob)> = None;
        let mut accumulators_snapshot: Option<Accumulators> = None;

        {
            let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);

            // Handle first update - just sync state without counting anything
            // This ensures we don't count partial cycles if server starts with heater ON
            if !inner.initialized {
                inner.initialized = true;
                inner.compressor_on = is_on;
                inner.state_started_at = now;
                // observed_cycle_start stays false - we didn't observe the actual start
                return;
            }

            let mut dirty = false;

            // Window rollovers. A day rollover may yield an archived
            // DailyBlob to flush to the store.
            if let Some(daily) = inner.check_window_rollovers(now_secs) {
                archived_daily = Some(daily);
                dirty = true;
            }

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
                    let started_at = inner.state_started_at;

                    inner.cycle_history.push_back(cycle.clone());
                    if inner.cycle_history.len() > MAX_CYCLE_HISTORY {
                        inner.cycle_history.pop_front();
                    }

                    // If this cycle spans midnight, an earlier portion has
                    // already been credited to previous days' archives at
                    // day-rollover time. Subtract that portion from the
                    // current-day counters so the total stays accurate.
                    let credited = inner.current_cycle_credited_secs;
                    let today_secs = duration_secs.saturating_sub(credited);

                    // Update operating time counters
                    inner.operating_secs_this_hour += duration_secs;
                    inner.operating_secs_this_day += today_secs;
                    inner.operating_secs_this_week += duration_secs;
                    inner.operating_secs_this_month += duration_secs;
                    inner.operating_secs_this_year += duration_secs;
                    inner.total_operating_secs += duration_secs;
                    inner.current_day_operating_secs += today_secs;

                    closed_cycle = Some((
                        started_at,
                        CycleBlob {
                            timestamp: cycle.timestamp,
                            duration_secs: cycle.duration_secs,
                            outdoor_temp_c: cycle.outdoor_temp_c,
                        },
                    ));
                    dirty = true;
                }

                // Reset for next cycle
                inner.observed_cycle_start = false;
                inner.current_cycle_credited_secs = 0;
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
                dirty = true;
            }

            // Update state if changed
            if inner.compressor_on != is_on {
                inner.compressor_on = is_on;
                inner.state_started_at = now;
            }

            if dirty {
                accumulators_snapshot = Some(Accumulators {
                    tracking_started_unix_secs: system_time_to_secs(inner.tracking_started),
                    total_starts: inner.total_starts,
                    total_operating_secs: inner.total_operating_secs,
                });
            }
        } // drop inner lock before touching store

        if let Some(store) = self.store.as_ref() {
            if let Some((started_at, blob)) = closed_cycle
                && let Err(e) = store.record_cycle(started_at, blob)
            {
                warn!("Failed to record cycle to store: {e}");
            }
            if let Some((date, blob)) = archived_daily {
                store.upsert_daily(date, blob);
            }
            if let Some(acc) = accumulators_snapshot {
                store.set_accumulators(acc);
            }
        }
    }

    /// Get the summary statistics
    #[must_use]
    pub fn get_summary(&self) -> HeatPumpStatsResponse {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
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

    /// Get history data for charts.
    ///
    /// Reads from the store when one is attached; otherwise serves the
    /// in-memory cycle tail (cycles only — daily history requires a store).
    ///
    /// # Arguments
    /// * `days` - Number of days of daily history to return (max 365)
    #[must_use]
    pub fn get_history(&self, days: usize) -> HeatPumpHistoryResponse {
        let days = days.min(MAX_DAILY_HISTORY);

        let Some(store) = self.store.as_ref() else {
            // No store: fall back to the in-memory cycle tail. Daily history
            // is empty since archive targets the store.
            let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
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
            return HeatPumpHistoryResponse {
                cycles,
                daily: Vec::new(),
            };
        };

        // Recent cycles: newest first from store, take up to 100, reverse to
        // chronological order for the chart.
        let mut cycles: Vec<CycleRecord> = store
            .recent_cycles(0, 100)
            .unwrap_or_default()
            .into_iter()
            .map(|c| CycleRecord {
                timestamp: c.timestamp,
                duration_secs: c.duration_secs,
                outdoor_temp_c: c.outdoor_temp_c,
            })
            .collect();
        cycles.reverse();

        // Daily history: ascending by date from store; take the tail `days`.
        let all_daily = store.all_daily().unwrap_or_default();
        let daily: Vec<DailyRecord> = all_daily
            .into_iter()
            .rev()
            .take(days)
            .map(|d| DailyRecord {
                date: d.date,
                starts: d.starts,
                operating_hours: d.operating_hours,
                avg_outdoor_temp_c: d.avg_outdoor_temp_c,
            })
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

fn fresh_inner(now: SystemTime, tz: Tz) -> HeatPumpStatsInner {
    let now_secs = system_time_to_secs(now);
    let (year, month, day) = secs_to_ymd_in(now_secs, tz);
    HeatPumpStatsInner {
        initialized: false,
        observed_cycle_start: false,
        compressor_on: false,
        state_started_at: now,
        cycle_start_temp: None,
        cycle_history: VecDeque::with_capacity(MAX_CYCLE_HISTORY),

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
        current_day_start: day_start_in(now_secs, tz),
        current_week_start: week_start(now_secs),
        current_month_start: month_start(year, month),
        current_year_start: year_start(year),

        current_day_date: (year, month, day),
        current_day_starts: 0,
        current_day_operating_secs: 0,
        current_day_temp_sum: 0.0,
        current_day_temp_count: 0,
        current_cycle_credited_secs: 0,

        tracking_started: now,
        total_starts: 0,
        total_operating_secs: 0,

        tz,
    }
}

impl HeatPumpStatsInner {
    /// Check and handle window rollovers (hour, day, week, month, year).
    /// Returns `Some((yyyymmdd, DailyBlob))` when a day rollover archived a
    /// record — caller pushes it to the store after releasing the lock.
    fn check_window_rollovers(&mut self, now_secs: u64) -> Option<(u32, DailyBlob)> {
        let (year, month, _day) = secs_to_ymd_in(now_secs, self.tz);
        let mut archived = None;

        // Hour rollover
        let new_hour_start = hour_start(now_secs);
        if new_hour_start != self.current_hour_start {
            trace!("Hour rollover detected");
            self.starts_this_hour = 0;
            self.operating_secs_this_hour = 0;
            self.current_hour_start = new_hour_start;
        }

        // Day rollover
        let new_day_start = day_start_in(now_secs, self.tz);
        if new_day_start != self.current_day_start {
            trace!("Day rollover detected");

            // If a compressor cycle is currently in progress, credit the
            // portion of it that falls inside the day being archived. The
            // credited amount is remembered so it can be subtracted from
            // the cycle's total when it eventually completes (avoids
            // double-counting). Gate on `observed_cycle_start` to match
            // the cycle-completion logic.
            if self.compressor_on && self.observed_cycle_start {
                let state_started_secs = system_time_to_secs(self.state_started_at);
                let cycle_floor = state_started_secs.max(self.current_day_start);
                if new_day_start > cycle_floor {
                    let pre_midnight = new_day_start - cycle_floor;
                    self.operating_secs_this_day += pre_midnight;
                    self.current_day_operating_secs += pre_midnight;
                    self.current_cycle_credited_secs += pre_midnight;
                }
            }

            // Snapshot the previous day for the store.
            archived = self.snapshot_current_day();

            self.starts_this_day = 0;
            self.operating_secs_this_day = 0;
            self.current_day_start = new_day_start;

            // Reset current day tracking. Derive the date from the new day
            // boundary, not `now_secs` — a rapid double-tick across midnight
            // could otherwise drift if those two helpers disagree.
            self.current_day_date = secs_to_ymd_in(new_day_start, self.tz);
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

        archived
    }

    /// Build a `DailyBlob` from the current day's accumulators. Returns
    /// `None` if the day has no recorded activity.
    fn snapshot_current_day(&self) -> Option<(u32, DailyBlob)> {
        if self.current_day_starts == 0 && self.current_day_operating_secs == 0 {
            return None;
        }

        let (year, month, day) = self.current_day_date;
        let date = format!("{year:04}-{month:02}-{day:02}");
        // Negative years mean the clock is set before year 0 — practically
        // unreachable from a u64 Unix timestamp, but if it ever happens we
        // route the row to a sentinel 1970-01-01 key (19700101) so successive
        // corruptions don't silently collide at month/day-only keys.
        #[allow(clippy::cast_sign_loss)]
        let yyyymmdd = if year < 0 {
            warn!("snapshot_current_day: negative year {year}; routing to sentinel 1970-01-01");
            19_700_101
        } else {
            (year as u32) * 10_000 + month * 100 + day
        };

        #[allow(clippy::cast_precision_loss)]
        let operating_hours = self.current_day_operating_secs as f64 / 3600.0;

        let avg_temp = if self.current_day_temp_count > 0 {
            Some(self.current_day_temp_sum / f64::from(self.current_day_temp_count))
        } else {
            None
        };

        Some((
            yyyymmdd,
            DailyBlob {
                date,
                starts: self.current_day_starts,
                operating_hours,
                avg_outdoor_temp_c: avg_temp,
            },
        ))
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

/// Get the start of the day (local midnight in `tz`) containing the given
/// timestamp, expressed as a Unix timestamp in UTC seconds.
fn day_start_in(secs: u64, tz: Tz) -> u64 {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    let (year, month, day, _) = crate::energy::tariff::system_time_to_local(time, tz);
    crate::energy::tariff::local_midnight_utc_secs(year, month, day, tz)
}

/// Get the start of the week (Monday UTC midnight) containing the given timestamp.
fn week_start(secs: u64) -> u64 {
    let datetime = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
    let days_since_monday = datetime.weekday().num_days_from_monday();
    let monday = datetime - chrono::Duration::days(i64::from(days_since_monday));
    // `(0, 0, 0)` is always a valid time-of-day; `and_hms_opt` only returns
    // None for out-of-range hours/minutes/seconds.
    let monday_midnight = monday
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is a valid time-of-day")
        .and_utc();
    #[allow(clippy::cast_sign_loss)]
    {
        monday_midnight.timestamp() as u64
    }
}

/// Get the start of the month containing the given year/month.
///
/// Callers (`year_start`, `month_buckets`) only invoke this with months
/// produced by `secs_to_ymd_in`, which routes through `system_time_to_local`
/// and is guaranteed to return month in 1..=12.
fn month_start(year: i32, month: u32) -> u64 {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, 1)
        .expect("month must be 1..=12 by caller contract");
    let datetime = date
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is a valid time-of-day")
        .and_utc();
    #[allow(clippy::cast_sign_loss)]
    {
        datetime.timestamp() as u64
    }
}

/// Get the start of the year
fn year_start(year: i32) -> u64 {
    month_start(year, 1)
}

/// Convert Unix timestamp to (year, month, day) in `tz` local time.
fn secs_to_ymd_in(secs: u64, tz: Tz) -> (i32, u32, u32) {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    let (year, month, day, _hour) = crate::energy::tariff::system_time_to_local(time, tz);
    (year, month, day)
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
        // 2026-01-15 14:37:45 UTC == 2026-01-15 15:37:45 Swedish (UTC+1).
        let ts: u64 = 1_768_487_865;
        let day = day_start_in(ts, Stockholm);

        // Swedish-local midnight for 2026-01-15 is 2026-01-14 23:00:00 UTC.
        // 2026-01-15 00:00:00 UTC == 1_768_435_200, so Swedish midnight is
        // one hour earlier: 1_768_431_600.
        assert_eq!(day, 1_768_431_600);
    }

    #[test]
    fn test_day_start_swedish_boundary() {
        // 2026-01-15 23:30:00 UTC == 2026-01-16 00:30:00 Swedish — the
        // Swedish-local day has already rolled over.
        let ts: u64 = 1_768_519_800;
        let day = day_start_in(ts, Stockholm);
        // Swedish midnight of 2026-01-16 == 2026-01-15 23:00:00 UTC == 1_768_518_000.
        assert_eq!(day, 1_768_518_000);
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
        // 2026-01-15 14:37:45 UTC == 2026-01-15 15:37:45 Swedish.
        let ts: u64 = 1_768_487_865;
        let (year, month, day) = secs_to_ymd_in(ts, Stockholm);

        assert_eq!(year, 2026);
        assert_eq!(month, 1);
        assert_eq!(day, 15);
    }

    #[test]
    fn test_secs_to_ymd_crosses_swedish_midnight() {
        // 2026-01-15 23:30:00 UTC is already 2026-01-16 00:30:00 Swedish.
        let ts: u64 = 1_768_519_800;
        let (year, month, day) = secs_to_ymd_in(ts, Stockholm);
        assert_eq!((year, month, day), (2026, 1, 16));
    }

    #[test]
    fn test_secs_to_ymd_crosses_swedish_midnight_cest() {
        // 2026-07-15 22:30:00 UTC is 2026-07-16 00:30:00 Swedish during CEST
        // (UTC+2). The Swedish day boundary lies at 22:00 UTC in summer, not
        // 23:00 UTC as it does in winter.
        let ts: u64 = 1_784_154_600;
        let (year, month, day) = secs_to_ymd_in(ts, Stockholm);
        assert_eq!((year, month, day), (2026, 7, 16));
    }

    #[test]
    fn test_day_start_swedish_boundary_cest() {
        // Same instant as above: 2026-07-15 22:30:00 UTC == 2026-07-16
        // 00:30:00 Swedish (CEST). The start of the Swedish day 2026-07-16
        // is 2026-07-15 22:00:00 UTC == 1_784_152_800.
        let ts: u64 = 1_784_154_600;
        let day = day_start_in(ts, Stockholm);
        assert_eq!(day, 1_784_152_800);
    }

    #[test]
    fn test_check_window_rollovers_spring_forward_dst() {
        use chrono::TimeZone;

        // 2026 Stockholm spring forward: Sunday 2026-03-29 02:00 CET → 03:00
        // CEST. The Sunday→Monday rollover is the DST-affected one: Sunday's
        // local day is only 23 hours long because the clock jumps forward.
        let sunday_noon = Stockholm.with_ymd_and_hms(2026, 3, 29, 12, 0, 0).unwrap();
        let monday_noon = Stockholm.with_ymd_and_hms(2026, 3, 30, 12, 0, 0).unwrap();
        let monday_midnight = Stockholm.with_ymd_and_hms(2026, 3, 30, 0, 0, 0).unwrap();

        let sunday_noon_secs = u64::try_from(sunday_noon.timestamp()).unwrap();
        let monday_noon_secs = u64::try_from(monday_noon.timestamp()).unwrap();
        let monday_midnight_secs = u64::try_from(monday_midnight.timestamp()).unwrap();

        let mut inner = fresh_inner(
            SystemTime::UNIX_EPOCH + Duration::from_secs(sunday_noon_secs),
            Stockholm,
        );
        // Force the day to look active so snapshot_current_day archives.
        inner.current_day_starts = 3;
        inner.current_day_operating_secs = 7200;

        let archived = inner
            .check_window_rollovers(monday_noon_secs)
            .expect("Sunday should be archived on Monday rollover");
        assert_eq!(archived.0, 20_260_329, "yyyymmdd for archived Sunday");
        assert_eq!(archived.1.date, "2026-03-29");
        assert_eq!(archived.1.starts, 3);

        assert_eq!(
            inner.current_day_start, monday_midnight_secs,
            "new current_day_start should be Monday Stockholm midnight"
        );
        assert_eq!(inner.current_day_date, (2026, 3, 30));
        assert_eq!(inner.current_day_starts, 0);
    }

    #[test]
    fn test_active_cycle_credit_across_spring_forward_dst() {
        use chrono::TimeZone;

        // Stockholm 2026-03-30 (Monday) midnight is the day boundary that
        // immediately follows DST spring-forward (Sunday 02:00 → 03:00 CEST).
        // An active cycle that started 30 min before Monday-midnight Stockholm
        // should credit Sunday with that 30 min when the day rolls over.
        let sunday_anchor = Stockholm.with_ymd_and_hms(2026, 3, 29, 12, 0, 0).unwrap();
        let monday_midnight = Stockholm.with_ymd_and_hms(2026, 3, 30, 0, 0, 0).unwrap();
        let cycle_start = monday_midnight - chrono::Duration::minutes(30);
        let after_midnight = monday_midnight + chrono::Duration::minutes(10);

        let sunday_anchor_secs = u64::try_from(sunday_anchor.timestamp()).unwrap();
        let monday_midnight_secs = u64::try_from(monday_midnight.timestamp()).unwrap();
        let cycle_start_secs = u64::try_from(cycle_start.timestamp()).unwrap();
        let after_midnight_secs = u64::try_from(after_midnight.timestamp()).unwrap();

        let mut inner = fresh_inner(
            SystemTime::UNIX_EPOCH + Duration::from_secs(sunday_anchor_secs),
            Stockholm,
        );
        // Simulate an in-progress cycle that started 30 min before Monday-midnight.
        inner.compressor_on = true;
        inner.observed_cycle_start = true;
        inner.state_started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(cycle_start_secs);

        let archived = inner
            .check_window_rollovers(after_midnight_secs)
            .expect("Sunday should be archived on Monday rollover");
        assert_eq!(archived.0, 20_260_329, "yyyymmdd for archived Sunday");

        // Sunday should be credited with the 30 min pre-midnight portion (0.5 h).
        assert_float_eq(
            archived.1.operating_hours,
            1800.0 / 3600.0,
            "Sunday operating_hours from pre-midnight credit",
        );

        // current_cycle_credited_secs records the credit so the eventual
        // cycle close subtracts it instead of double-counting.
        assert_eq!(inner.current_cycle_credited_secs, 1800);

        // Day boundary advanced cleanly to Monday Stockholm midnight.
        assert_eq!(inner.current_day_start, monday_midnight_secs);
        assert_eq!(inner.current_day_date, (2026, 3, 30));
    }

    #[test]
    fn test_active_cycle_credit_across_fall_back_dst() {
        use chrono::TimeZone;

        // Stockholm 2026-10-26 (Monday) midnight is the boundary right after
        // DST fall-back (Sunday 03:00 CEST → 02:00 CET). Sunday's local day
        // is 25 hours long; the credit math must still produce exactly the
        // pre-midnight chunk regardless of day length.
        let sunday_anchor = Stockholm.with_ymd_and_hms(2026, 10, 25, 12, 0, 0).unwrap();
        let monday_midnight = Stockholm.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap();
        let cycle_start = monday_midnight - chrono::Duration::minutes(45);
        let after_midnight = monday_midnight + chrono::Duration::minutes(15);

        let sunday_anchor_secs = u64::try_from(sunday_anchor.timestamp()).unwrap();
        let monday_midnight_secs = u64::try_from(monday_midnight.timestamp()).unwrap();
        let cycle_start_secs = u64::try_from(cycle_start.timestamp()).unwrap();
        let after_midnight_secs = u64::try_from(after_midnight.timestamp()).unwrap();

        let mut inner = fresh_inner(
            SystemTime::UNIX_EPOCH + Duration::from_secs(sunday_anchor_secs),
            Stockholm,
        );
        inner.compressor_on = true;
        inner.observed_cycle_start = true;
        inner.state_started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(cycle_start_secs);

        let archived = inner
            .check_window_rollovers(after_midnight_secs)
            .expect("Sunday should be archived on Monday rollover");
        assert_eq!(archived.0, 20_261_025, "yyyymmdd for archived Sunday");

        // Sunday should be credited with the 45 min pre-midnight portion (0.75 h).
        assert_float_eq(
            archived.1.operating_hours,
            2700.0 / 3600.0,
            "Sunday operating_hours (45 min pre-midnight)",
        );
        assert_eq!(inner.current_cycle_credited_secs, 2700);
        assert_eq!(inner.current_day_start, monday_midnight_secs);
        assert_eq!(inner.current_day_date, (2026, 10, 26));
    }

    #[test]
    fn test_check_window_rollovers_fall_back_dst() {
        use chrono::TimeZone;

        // 2026 Stockholm fall back: Sunday 2026-10-25 03:00 CEST → 02:00 CET.
        // The Sunday→Monday rollover is the DST-affected one: Sunday's local
        // day is 25 hours long because the clock jumps back.
        let sunday_noon = Stockholm.with_ymd_and_hms(2026, 10, 25, 12, 0, 0).unwrap();
        let monday_noon = Stockholm.with_ymd_and_hms(2026, 10, 26, 12, 0, 0).unwrap();
        let monday_midnight = Stockholm.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap();

        let sunday_noon_secs = u64::try_from(sunday_noon.timestamp()).unwrap();
        let monday_noon_secs = u64::try_from(monday_noon.timestamp()).unwrap();
        let monday_midnight_secs = u64::try_from(monday_midnight.timestamp()).unwrap();

        let mut inner = fresh_inner(
            SystemTime::UNIX_EPOCH + Duration::from_secs(sunday_noon_secs),
            Stockholm,
        );
        inner.current_day_starts = 5;
        inner.current_day_operating_secs = 10_800;

        let archived = inner
            .check_window_rollovers(monday_noon_secs)
            .expect("Sunday should be archived on Monday rollover");
        assert_eq!(archived.0, 20_261_025, "yyyymmdd for archived Sunday");
        assert_eq!(archived.1.date, "2026-10-25");
        assert_eq!(archived.1.starts, 5);

        assert_eq!(
            inner.current_day_start, monday_midnight_secs,
            "new current_day_start should be Monday Stockholm midnight"
        );
        assert_eq!(inner.current_day_date, (2026, 10, 26));
        assert_eq!(inner.current_day_starts, 0);
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
    fn test_mark_poll_failed_discards_in_progress_cycle() {
        let stats = HeatPumpStats::new();

        // Establish an observed ON cycle.
        stats.update_state(0, Some(0.0));
        stats.update_state(3, Some(-5.0));
        assert_eq!(stats.get_summary().starts.this_hour, 1);

        // Modbus poll fails — state becomes unknown, cycle is discarded.
        stats.mark_poll_failed();

        // Compressor turns off during the outage; we only see it after recovery.
        stats.update_state(0, Some(-5.0));

        // No cycle should be recorded (we couldn't verify the cycle's duration).
        let summary = stats.get_summary();
        assert!(
            summary.cycle_stats.is_none(),
            "outage cycle must not be recorded"
        );
        // No operating time should be credited across the outage.
        assert!(summary.operating_hours.this_hour < 0.001);

        assert_eq!(summary.starts.this_hour, 1);
    }

    #[test]
    fn test_mark_poll_failed_rolls_back_pre_midnight_credit() {
        let stats = HeatPumpStats::new();

        stats.update_state(0, Some(0.0));
        stats.update_state(3, Some(-5.0));

        let now_secs = system_time_to_secs(SystemTime::now());
        let today_midnight = day_start_in(now_secs, Stockholm);
        let yesterday_midnight = day_start_in(today_midnight - 1, Stockholm);

        {
            let mut inner = stats.inner.lock().unwrap();
            inner.state_started_at =
                SystemTime::UNIX_EPOCH + Duration::from_secs(today_midnight - 1800);
            inner.current_day_start = yesterday_midnight;
            inner.current_day_date = secs_to_ymd_in(yesterday_midnight, Stockholm);
            inner.current_day_starts = 1;
        }
        stats.update_state(3, Some(-5.0));

        let before_today = stats.get_summary().operating_hours.this_day;
        stats.mark_poll_failed();
        let after_today = stats.get_summary().operating_hours.this_day;
        assert!(
            before_today >= after_today,
            "mark_poll_failed should not inflate today's operating hours \
             (before={before_today}, after={after_today})"
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn test_cycle_spanning_midnight_splits_between_days() {
        let (_dir, store) = tmp_store();
        let stats = HeatPumpStats::new_with_store_and_tz(store, Stockholm);

        // Bring the tracker into a clean OFF state, then ON state.
        stats.update_state(0, Some(0.0));
        stats.update_state(3, Some(-5.0));

        // Now rewrite internal state so the current ON cycle appears to
        // have started 30 minutes before Swedish-local midnight, and the
        // "current day" is the day that just ended. We do NOT touch `now`
        // itself — `update_state` reads `SystemTime::now()` which is
        // strictly after the rewritten state_started_at, so the day
        // rollover at the top of `update_state` will fire.
        let now = SystemTime::now();
        let now_secs = system_time_to_secs(now);
        // Find the Swedish-local midnight that has most recently passed.
        let today_midnight = day_start_in(now_secs, Stockholm);
        // Skip the test if we happen to be running within ±5 s of Swedish-local
        // midnight. The test arithmetic relies on the day boundary not moving
        // between the two `SystemTime::now()` reads, which can fail at the
        // exact rollover. Re-running the suite a few seconds later will work.
        if now_secs.saturating_sub(today_midnight) < 5 {
            eprintln!(
                "test_cycle_spanning_midnight_splits_between_days: skipping — too close to Swedish-local midnight (now-midnight = {} s)",
                now_secs - today_midnight
            );
            return;
        }
        // Cycle started 30 min before that midnight.
        let cycle_start_secs = today_midnight - 1800;
        let yesterday_midnight = day_start_in(today_midnight - 1, Stockholm);

        {
            let mut inner = stats.inner.lock().unwrap();
            inner.state_started_at = SystemTime::UNIX_EPOCH + Duration::from_secs(cycle_start_secs);
            inner.current_day_start = yesterday_midnight;
            // Match current_day_date to yesterday so archive_current_day
            // writes a record labelled with yesterday's date.
            inner.current_day_date = secs_to_ymd_in(yesterday_midnight, Stockholm);
            // Pretend yesterday already had a non-trivial accumulator so
            // archive_current_day doesn't early-return on the "no data" guard.
            inner.current_day_starts = 1;
        }

        // Cycle ends "now" — duration is roughly (now - cycle_start).
        stats.update_state(0, Some(-5.0));

        let now_secs_after = system_time_to_secs(SystemTime::now());
        let total_cycle_secs = now_secs_after - cycle_start_secs;
        // Today gets only the portion after Swedish-local midnight.
        let expected_today_secs = now_secs_after - today_midnight;
        // Yesterday gets the 30 min before midnight.
        let expected_yesterday_secs: u64 = 1800;

        let summary = stats.get_summary();
        // Allow a small tolerance because SystemTime::now() advances
        // between our reads.
        let this_day_secs = (summary.operating_hours.this_day * 3600.0).round() as u64;
        assert!(
            this_day_secs.abs_diff(expected_today_secs) <= 1,
            "today's operating secs: got {this_day_secs}, expected ~{expected_today_secs}"
        );

        // Yesterday should have been archived with ~30 min of operating time.
        let history = stats.get_history(7);
        let yesterday = history
            .daily
            .iter()
            .find(|d| {
                let (y, m, d_) = secs_to_ymd_in(yesterday_midnight, Stockholm);
                d.date == format!("{y:04}-{m:02}-{d_:02}")
            })
            .expect("yesterday's archive should be present");
        let yesterday_secs = (yesterday.operating_hours * 3600.0).round() as u64;
        assert_eq!(
            yesterday_secs, expected_yesterday_secs,
            "yesterday's operating secs"
        );

        // Total across both days must equal the full cycle (no double counting).
        assert!(
            (yesterday_secs + this_day_secs).abs_diff(total_cycle_secs) <= 1,
            "split totals should equal cycle duration: \
             yesterday={yesterday_secs} + today={this_day_secs} vs cycle={total_cycle_secs}"
        );

        // The cycle history still records the full duration.
        let last_cycle = history.cycles.last().expect("cycle recorded");
        assert!(
            last_cycle.duration_secs.abs_diff(total_cycle_secs) <= 1,
            "cycle history duration should be the full cycle"
        );
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

    fn tmp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();
        (dir, store)
    }

    #[test]
    fn test_store_round_trip() {
        let (_dir, store) = tmp_store();
        let stats = HeatPumpStats::new_with_store_and_tz(store.clone(), Stockholm);

        // Drive a complete cycle so accumulators advance.
        stats.update_state(0, Some(10.0));
        stats.update_state(3, Some(11.0));
        std::thread::sleep(std::time::Duration::from_millis(20));
        stats.update_state(0, Some(12.0));

        let before = stats.get_summary();
        assert_eq!(before.tracking.total_starts, 1);

        // A second HeatPumpStats sharing the same store must see the same
        // accumulators — no flush is needed because Store keeps everything
        // in RAM until `flush()` is called.
        let reloaded = HeatPumpStats::new_with_store_and_tz(store, Stockholm);
        let after = reloaded.get_summary();
        assert_eq!(after.tracking.total_starts, 1);
        assert_eq!(after.tracking.started_at, before.tracking.started_at);

        // Cycle history must round-trip via the store.
        let history = reloaded.get_history(7);
        assert_eq!(history.cycles.len(), 1);
    }

    #[test]
    fn test_store_round_trip_after_flush() {
        // Open a store, write a cycle, flush, then re-open the store and
        // verify accumulators + cycle survive across the simulated restart.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ctc.redb");

        {
            let store = Store::open(&path).unwrap();
            let stats = HeatPumpStats::new_with_store_and_tz(store.clone(), Stockholm);
            stats.update_state(0, Some(5.0));
            stats.update_state(3, Some(6.0));
            std::thread::sleep(std::time::Duration::from_millis(10));
            stats.update_state(0, Some(7.0));
            store.flush().unwrap();
        }

        let store = Store::open(&path).unwrap();
        let stats = HeatPumpStats::new_with_store_and_tz(store, Stockholm);
        let summary = stats.get_summary();
        assert_eq!(summary.tracking.total_starts, 1);
        assert_eq!(stats.get_history(7).cycles.len(), 1);
    }

    #[test]
    fn test_fresh_store_starts_with_zero_accumulators() {
        let (_dir, store) = tmp_store();
        let stats = HeatPumpStats::new_with_store_and_tz(store, Stockholm);
        assert_eq!(stats.get_summary().tracking.total_starts, 0);
    }
}
