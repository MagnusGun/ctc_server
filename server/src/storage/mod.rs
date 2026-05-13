//! Persistent storage for the dashboard.
//!
//! Backed by a single embedded `redb` database. Hot paths only touch an
//! in-memory mirror — the database is fsync'd at most once an hour by
//! `flush()` (and on graceful shutdown), so we never wear flash on per-poll
//! commits.
//!
//! Three logical stores:
//!
//! * `CYCLES`   — `unix_secs -> JSON(CycleBlob)` completed compressor cycles
//! * `DAILY`    — `yyyymmdd -> JSON(DailyBlob)` per-day aggregates
//! * `TRACKING` — `"accumulators" -> JSON(Accumulators)` lifetime counters
//!
//! JSON costs more bytes than a binary codec, but the volumes here are tiny
//! (one accumulator row, a handful of cycles per hour, ≤365 daily rows) and
//! `serde_json` is already a workspace dep with no extra audit warnings.
//!
//! Raw sensor samples live only in a per-sensor 24h in-memory ring; the
//! trend modal serves from that ring and a restart drops it. Persisting
//! samples to disk would generate ~329k row inserts per flush for no
//! product benefit.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};
use tracing::warn;

// v1 cycle table — keyed by Unix-seconds only. Read-only after open() migrates
// any rows out; left declared so the migration path can iterate it.
const CYCLES_V1: TableDefinition<i64, &[u8]> = TableDefinition::new("cycles");
// v2 cycle table — keyed by (Unix-seconds, per-second sequence) so two cycles
// in the same second don't collide on insert.
const CYCLES: TableDefinition<(i64, u32), &[u8]> = TableDefinition::new("cycles_v2");
const DAILY: TableDefinition<u32, &[u8]> = TableDefinition::new("daily");
const TRACKING: TableDefinition<&str, &[u8]> = TableDefinition::new("tracking");
const META: TableDefinition<&str, u32> = TableDefinition::new("meta");
const STEP_EVENTS: TableDefinition<i64, &[u8]> = TableDefinition::new("step_events");

/// Cap on disk + in-memory step-response events. The chart renders the last
/// 6 by default, so 50 leaves headroom and bounds the table size.
const MAX_STEP_EVENTS: usize = 50;

const SCHEMA_VERSION: u32 = 2;
const TRACKING_KEY: &str = "accumulators";
const SCHEMA_KEY: &str = "schema_version";

/// Default in-memory retention for sensor samples (seconds).
pub const SERIES_RETENTION_SECS: i64 = 24 * 3600;

/// Stable sensor identifiers. New entries append; never renumber.
#[repr(u16)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Sensor {
    Room = 1,
    Outdoor = 2,
    Flow = 3,
    Return = 4,
    FlowSp = 5,
    HpIn = 6,
    HpOut = 7,
    Discharge = 8,
    Suction = 9,
    HighP = 10,
    LowP = 11,
    BrineIn = 12,
    BrineOut = 13,
    ChargePump = 14,
    BrinePump = 15,
    DhwUpper = 16,
    Lower = 17,
    SystemStatus = 18,
    HpStatus = 19,
}

impl Sensor {
    /// Map dashboard query strings to sensor IDs.
    #[must_use]
    pub fn from_slug(s: &str) -> Option<Self> {
        Some(match s {
            "room" => Self::Room,
            "outdoor" => Self::Outdoor,
            "flow" => Self::Flow,
            "return" => Self::Return,
            "flow_sp" | "flow-sp" => Self::FlowSp,
            "hp_in" | "hp-in" => Self::HpIn,
            "hp_out" | "hp-out" => Self::HpOut,
            "discharge" => Self::Discharge,
            "suction" => Self::Suction,
            "high_p" | "high-p" => Self::HighP,
            "low_p" | "low-p" => Self::LowP,
            "brine_in" | "brine-in" => Self::BrineIn,
            "brine_out" | "brine-out" => Self::BrineOut,
            "charge_pump" | "charge-pump" => Self::ChargePump,
            "brine_pump" | "brine-pump" => Self::BrinePump,
            "dhw_upper" | "dhw-upper" => Self::DhwUpper,
            "lower" => Self::Lower,
            "system_status" | "system-status" => Self::SystemStatus,
            "hp_status" | "hp-status" => Self::HpStatus,
            _ => return None,
        })
    }

    #[must_use]
    pub fn as_u16(self) -> u16 {
        self as u16
    }
}

/// Lifetime + windowed accumulators persisted across restarts.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Accumulators {
    pub tracking_started_unix_secs: u64,
    pub total_starts: u64,
    pub total_operating_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CycleBlob {
    pub timestamp: String,
    pub duration_secs: u64,
    pub outdoor_temp_c: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DailyBlob {
    pub date: String,
    pub starts: u32,
    pub operating_hours: f64,
    pub avg_outdoor_temp_c: Option<f64>,
}

/// One step-response capture: a detected change in flow temperature plus the
/// observed return-temperature response curve. Persisted via the redb store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEventBlob {
    /// Unix seconds when the step was detected.
    pub started_at: u64,
    /// Flow temperature before the step (last stable reading).
    pub flow_before: f32,
    /// Flow temperature immediately after the step.
    pub flow_after: f32,
    /// Return temperature at the moment of the step.
    pub return_before: f32,
    /// Samples taken over the observation window:
    /// `(seconds_since_start, flow_c, return_c)`.
    pub samples: Vec<(u32, f32, f32)>,
}

/// In-memory mirror. Hot paths only touch this; `flush` drains the pending
/// fields to disk in one transaction.
#[derive(Default)]
struct MemState {
    /// Per-sensor 24h ring. Kept fully in RAM so the trend modal never hits disk.
    series: HashMap<u16, VecDeque<(i64, f32)>>,
    /// Latest accumulator snapshot (overwrites on flush).
    accumulators: Accumulators,
    /// Cycles closed since boot. Append-only; flushed list cleared on commit.
    /// Key is `(unix_secs, seq)` so two cycles in the same second remain
    /// distinct.
    pending_cycles: Vec<((i64, u32), CycleBlob)>,
    /// Next unused per-second sequence for cycle keys. Hydrated on open so the
    /// counter survives a restart; the entry for a given second is bumped on
    /// every record + every legacy-import call.
    cycle_seq_next: HashMap<i64, u32>,
    /// Daily records dirtied since last flush.
    pending_daily: HashMap<u32, DailyBlob>,
    /// Step-response events captured since the last flush.
    pending_step_events: Vec<(i64, StepEventBlob)>,
    /// Step-response events hydrated from disk on `open` for the API to serve.
    /// Newest-first; capped at `MAX_STEP_EVENTS`.
    step_events_cache: Vec<StepEventBlob>,
    /// Anything to write?
    dirty: bool,
    /// Monotonic counter bumped by every record_*/set_*/upsert_*. `flush`
    /// snapshots this before releasing the lock and only clears `dirty` if
    /// it hasn't changed by the time the commit lands — otherwise a
    /// concurrent write that ran during the commit would have its
    /// `dirty=true` clobbered.
    dirty_gen: u64,
}

impl MemState {
    /// Mark state dirty and bump the generation counter. The counter lets
    /// `flush` detect whether a concurrent writer landed during its commit.
    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.dirty_gen = self.dirty_gen.wrapping_add(1);
    }
}

#[derive(Clone)]
pub struct Store {
    db: Arc<Database>,
    state: Arc<Mutex<MemState>>,
    series_retention_secs: i64,
}

/// Open errors that may surface during startup. redb's error variants are
/// each ~160 bytes; box them so `Result<_, StorageError>` stays small enough
/// to satisfy clippy's `result_large_err` and avoid pessimising hot paths.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error(transparent)]
    Db(#[from] Box<redb::Error>),
    #[error(transparent)]
    Tx(#[from] Box<redb::TransactionError>),
    #[error(transparent)]
    Table(#[from] Box<redb::TableError>),
    #[error(transparent)]
    Commit(#[from] Box<redb::CommitError>),
    #[error(transparent)]
    Storage(#[from] Box<redb::StorageError>),
    #[error(transparent)]
    DbCreate(#[from] Box<redb::DatabaseError>),
    #[error("json codec: {0}")]
    Codec(#[from] serde_json::Error),
    #[error("time: clock moved before unix epoch")]
    Clock,
}

// Shim From impls so callers can still do `redb_call()?` without sprinkling
// `Box::new(...)` everywhere — `?` invokes these instead of the now-Box-only
// `#[from]` variants directly.
impl From<redb::Error> for StorageError {
    fn from(e: redb::Error) -> Self {
        Self::Db(Box::new(e))
    }
}
impl From<redb::TransactionError> for StorageError {
    fn from(e: redb::TransactionError) -> Self {
        Self::Tx(Box::new(e))
    }
}
impl From<redb::TableError> for StorageError {
    fn from(e: redb::TableError) -> Self {
        Self::Table(Box::new(e))
    }
}
impl From<redb::CommitError> for StorageError {
    fn from(e: redb::CommitError) -> Self {
        Self::Commit(Box::new(e))
    }
}
impl From<redb::StorageError> for StorageError {
    fn from(e: redb::StorageError) -> Self {
        Self::Storage(Box::new(e))
    }
}
impl From<redb::DatabaseError> for StorageError {
    fn from(e: redb::DatabaseError) -> Self {
        Self::DbCreate(Box::new(e))
    }
}

type Result<T> = std::result::Result<T, StorageError>;

fn unix_secs(t: SystemTime) -> Result<i64> {
    // i64 is fine well past the year 2200.
    let s = t
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StorageError::Clock)?;
    i64::try_from(s.as_secs()).map_err(|_| StorageError::Clock)
}

impl Store {
    /// Open (or create) the database at `path` and hydrate the in-memory mirror
    /// from disk. The schema version is written into the `meta` table on every
    /// open; a future bump can read this to drive migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Database::create(path)?;

        // Read the on-disk schema version BEFORE writing our own — a v1 store
        // needs migration to v2 before the rest of open() touches CYCLES.
        let stored_version: Option<u32> = {
            let r = db.begin_read()?;
            match r.open_table(META) {
                Ok(t) => t.get(SCHEMA_KEY)?.map(|v| v.value()),
                Err(_) => None,
            }
        };

        if stored_version == Some(1) {
            migrate_cycles_v1_to_v2(&db)?;
        }

        // Write the current schema version (idempotent).
        {
            let w = db.begin_write()?;
            {
                let mut m = w.open_table(META)?;
                m.insert(SCHEMA_KEY, SCHEMA_VERSION)?;
            }
            w.commit()?;
        }

        let mut state = MemState::default();

        // Hydrate accumulators.
        let r = db.begin_read()?;
        if let Ok(t) = r.open_table(TRACKING)
            && let Some(v) = t.get(TRACKING_KEY)?
        {
            let acc: Accumulators = serde_json::from_slice(v.value())?;
            state.accumulators = acc;
        }

        // Hydrate the step-events cache (newest first, capped).
        if let Ok(t) = r.open_table(STEP_EVENTS) {
            for entry in t.iter()?.rev().take(MAX_STEP_EVENTS) {
                let (_, v) = entry?;
                let blob: StepEventBlob = serde_json::from_slice(v.value())?;
                state.step_events_cache.push(blob);
            }
        }

        // Hydrate cycle_seq_next so new same-second cycles after restart pick
        // up where the previous run left off instead of colliding with rows
        // already on disk.
        if let Ok(t) = r.open_table(CYCLES) {
            for entry in t.iter()? {
                let (k, _) = entry?;
                let (sec, seq) = k.value();
                let cur = state.cycle_seq_next.entry(sec).or_insert(0);
                *cur = (*cur).max(seq + 1);
            }
        }

        Ok(Self {
            db: Arc::new(db),
            state: Arc::new(Mutex::new(state)),
            series_retention_secs: SERIES_RETENTION_SECS,
        })
    }

    /// Record a single sensor sample. RAM-only; flushed hourly.
    ///
    /// Non-finite values (`NaN`, `±Inf`) are dropped silently — they corrupt
    /// downstream serialization (invalid JSON) and chart math (`Math.min(NaN)`
    /// poisons the y-axis).
    ///
    /// # Errors
    /// Returns an error if the system clock is set before the unix epoch.
    pub fn record_sample(&self, sensor: Sensor, t: SystemTime, value: f32) -> Result<()> {
        if !value.is_finite() {
            return Ok(());
        }
        let s = unix_secs(t)?;
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let buf = st.series.entry(sensor.as_u16()).or_default();
        buf.push_back((s, value));
        let cutoff = s - self.series_retention_secs;
        while buf.front().is_some_and(|(t, _)| *t < cutoff) {
            buf.pop_front();
        }
        // Series are RAM-only — no disk-bound state changes, so don't mark
        // dirty. Marking here would force the hourly flush to rewrite
        // accumulators/cycles/daily on every poll even when nothing else
        // changed.
        Ok(())
    }

    /// Read the in-memory series for `sensor` within `[from, to)`.
    ///
    /// Older data lives on disk and is not served here — the dashboard's 24h
    /// trend modal does not need it, and adding a disk fall-through would
    /// trade one cheap hit for a transaction per request.
    #[must_use]
    pub fn series_range(&self, sensor: Sensor, from: i64, to: i64) -> Vec<(i64, f32)> {
        let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.series
            .get(&sensor.as_u16())
            .map(|buf| {
                buf.iter()
                    .filter(|(t, _)| *t >= from && *t < to)
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Most recent sample for `sensor`, if any. O(1).
    #[must_use]
    pub fn latest_sample(&self, sensor: Sensor) -> Option<(i64, f32)> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .series
            .get(&sensor.as_u16())
            .and_then(|buf| buf.back().copied())
    }

    /// Replace the accumulator snapshot in memory. Caller owns increment logic.
    pub fn set_accumulators(&self, acc: Accumulators) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.accumulators = acc;
        st.mark_dirty();
    }

    #[must_use]
    pub fn accumulators(&self) -> Accumulators {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .accumulators
            .clone()
    }

    /// Queue a completed cycle. Flushed to disk on next `flush()`. Cycles
    /// that share a started-at second are disambiguated by a per-second
    /// sequence counter so neither overwrites the other.
    pub fn record_cycle(&self, started_at: SystemTime, blob: CycleBlob) -> Result<()> {
        let s = unix_secs(started_at)?;
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let seq = *st.cycle_seq_next.entry(s).or_insert(0);
        st.cycle_seq_next.insert(s, seq.saturating_add(1));
        st.pending_cycles.push(((s, seq), blob));
        st.mark_dirty();
        Ok(())
    }

    /// Queue a step-response event. Also pushed to the in-memory cache so the
    /// API can serve it immediately without waiting for the hourly flush.
    pub fn record_step_event(&self, blob: StepEventBlob) {
        let key = if let Ok(k) = i64::try_from(blob.started_at) {
            k
        } else {
            // started_at > i64::MAX implies a corrupt clock (year > 292 billion).
            // Clamp so we don't drop the event, but warn loudly — silent
            // saturation produces colliding STEP_EVENTS keys.
            tracing::warn!(
                "Step event timestamp {} overflows i64; clamping to i64::MAX (key collision possible)",
                blob.started_at
            );
            i64::MAX
        };
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.step_events_cache.insert(0, blob.clone());
        if st.step_events_cache.len() > MAX_STEP_EVENTS {
            st.step_events_cache.truncate(MAX_STEP_EVENTS);
        }
        st.pending_step_events.push((key, blob));
        st.mark_dirty();
    }

    /// Most recent step-response events (newest first). `limit` is capped at
    /// `MAX_STEP_EVENTS`.
    #[must_use]
    pub fn recent_step_events(&self, limit: usize) -> Vec<StepEventBlob> {
        let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.step_events_cache
            .iter()
            .take(limit.min(MAX_STEP_EVENTS))
            .cloned()
            .collect()
    }

    /// Upsert a per-day aggregate. Multiple updates per day collapse to one
    /// row on flush — the latest value wins.
    pub fn upsert_daily(&self, date_yyyymmdd: u32, blob: DailyBlob) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.pending_daily.insert(date_yyyymmdd, blob);
        st.mark_dirty();
    }

    /// Most recent `limit` cycles known to the in-memory mirror plus any cycles
    /// on disk newer than `since`.
    ///
    /// `since` is `unix_secs`. `limit` is capped server-side by the caller.
    pub fn recent_cycles(&self, since: i64, limit: usize) -> Result<Vec<CycleBlob>> {
        // Pending (RAM) first — newest at the back. Don't `break` on the
        // first older entry: pending_cycles ordering isn't a hard invariant
        // (migration appends in legacy-file order, which may not be sorted),
        // so a single out-of-order entry shouldn't truncate the rest.
        let mut out: Vec<CycleBlob> = Vec::new();
        {
            let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for ((t, _), c) in st.pending_cycles.iter().rev() {
                if *t < since {
                    continue;
                }
                out.push(c.clone());
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
        // Then on-disk, newest first. `since` is the seconds floor; the seq
        // component of the key isn't a filter, so use (since, 0) as the lower
        // bound to include every cycle from that second onward.
        let r = self.db.begin_read()?;
        if let Ok(t) = r.open_table(CYCLES) {
            for entry in t.range((since, 0u32)..)?.rev() {
                let (_, v) = entry?;
                let blob: CycleBlob = serde_json::from_slice(v.value())?;
                out.push(blob);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// All daily rows present on disk plus any pending updates (latest values).
    pub fn all_daily(&self) -> Result<Vec<DailyBlob>> {
        let mut by_date: HashMap<u32, DailyBlob> = HashMap::new();
        let r = self.db.begin_read()?;
        if let Ok(t) = r.open_table(DAILY) {
            for entry in t.iter()? {
                let (k, v) = entry?;
                let blob: DailyBlob = serde_json::from_slice(v.value())?;
                by_date.insert(k.value(), blob);
            }
        }
        {
            let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for (k, v) in &st.pending_daily {
                by_date.insert(*k, v.clone());
            }
        }
        let mut out: Vec<_> = by_date.into_values().collect();
        out.sort_by(|a, b| a.date.cmp(&b.date));
        Ok(out)
    }

    /// Commit all pending state to disk in a single transaction. Idempotent
    /// when nothing is dirty — costs only a mutex acquire.
    #[allow(clippy::many_single_char_names)]
    pub fn flush(&self) -> Result<()> {
        let (accumulators, cycles, daily, step_events, gen_at_snapshot) = {
            let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if !st.dirty {
                return Ok(());
            }
            let cycles = std::mem::take(&mut st.pending_cycles);
            let daily = std::mem::take(&mut st.pending_daily);
            let step_events = std::mem::take(&mut st.pending_step_events);
            let acc = st.accumulators.clone();
            let current_gen = st.dirty_gen;
            // Keep memory dirty=false only after the commit succeeds.
            (acc, cycles, daily, step_events, current_gen)
        };

        let w = self.db.begin_write()?;
        {
            let mut c = w.open_table(CYCLES)?;
            for (key, blob) in &cycles {
                let buf = serde_json::to_vec(blob)?;
                c.insert(*key, buf.as_slice())?;
            }
            let mut d = w.open_table(DAILY)?;
            for (date, blob) in &daily {
                let buf = serde_json::to_vec(blob)?;
                d.insert(*date, buf.as_slice())?;
            }
            let mut s = w.open_table(STEP_EVENTS)?;
            for (t, blob) in &step_events {
                let buf = serde_json::to_vec(blob)?;
                s.insert(*t, buf.as_slice())?;
            }
            // Bound table size: drop oldest entries beyond MAX_STEP_EVENTS.
            let total = s.len()?;
            let limit = u64::try_from(MAX_STEP_EVENTS).unwrap_or(u64::MAX);
            if total > limit {
                let to_drop = total - limit;
                let victims: Vec<i64> = s
                    .iter()?
                    .take(usize::try_from(to_drop).unwrap_or(usize::MAX))
                    .filter_map(|entry| entry.ok().map(|(k, _)| k.value()))
                    .collect();
                for k in victims {
                    s.remove(k)?;
                }
            }
            let mut t = w.open_table(TRACKING)?;
            let buf = serde_json::to_vec(&accumulators)?;
            t.insert(TRACKING_KEY, buf.as_slice())?;
        }
        w.commit()?;

        // Mark clean only after the commit landed — and only if no other
        // writer bumped the generation while we were committing. Otherwise
        // their `dirty = true` would be silently clobbered and the next
        // flush would skip their data.
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if st.dirty_gen == gen_at_snapshot {
            st.dirty = false;
        }
        Ok(())
    }

    /// One-shot migration: if a legacy JSON heatpump-stats file exists and the
    /// store has no `accumulators` row, load it and rename the file
    /// `<path>.migrated`. Returns `true` when a migration ran.
    ///
    /// # Errors
    /// Surfaces IO errors from rename. Parse failures are logged + treated as
    /// "no migration" so a corrupt file never blocks startup.
    pub fn migrate_from_legacy_json(&self, json_path: &Path) -> std::io::Result<bool> {
        // Bail if we already have data. Treat any non-default field — including
        // tracking_started_unix_secs alone — as "already migrated", otherwise a
        // store that's been initialized but hasn't yet logged a cycle would
        // re-run migration and clobber tracking_started_unix_secs.
        if self.accumulators() != Accumulators::default() {
            return Ok(false);
        }
        let bytes = match std::fs::read(json_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let Some(parsed) = parse_legacy_json(&bytes) else {
            warn!(
                "Legacy heatpump-stats JSON at {} could not be parsed — leaving file in place",
                json_path.display()
            );
            return Ok(false);
        };

        self.set_accumulators(Accumulators {
            tracking_started_unix_secs: parsed.tracking_started_unix_secs,
            total_starts: parsed.total_starts,
            total_operating_secs: parsed.total_operating_secs,
        });
        for c in parsed.cycle_history {
            let secs = parse_iso8601_secs(&c.timestamp).unwrap_or(0);
            // record_cycle's only error path is clock-skew, which doesn't
            // apply to a pre-parsed timestamp — swallow it.
            let _ = self.record_cycle(
                UNIX_EPOCH + std::time::Duration::from_secs(u64::try_from(secs).unwrap_or(0)),
                CycleBlob {
                    timestamp: c.timestamp,
                    duration_secs: c.duration_secs,
                    outdoor_temp_c: c.outdoor_temp_c,
                },
            );
        }
        for d in parsed.daily_history {
            let date_num = parse_date_yyyymmdd(&d.date).unwrap_or(0);
            self.upsert_daily(
                date_num,
                DailyBlob {
                    date: d.date,
                    starts: d.starts,
                    operating_hours: d.operating_hours,
                    avg_outdoor_temp_c: d.avg_outdoor_temp_c,
                },
            );
        }
        self.flush().map_err(std::io::Error::other)?;
        let renamed = json_path.with_extension("json.migrated");
        std::fs::rename(json_path, &renamed)?;
        Ok(true)
    }
}

/// One-shot schema migration: copy v1 cycle rows (keyed by `i64` seconds) into
/// the v2 table (keyed by `(i64, u32)` with seq starting at 0), then drop the
/// v1 table so future opens don't reapply.
///
/// v1 silently lost any same-second collisions on insert, so the rows we see
/// here are already de-duplicated — seq=0 is sufficient.
fn migrate_cycles_v1_to_v2(db: &Database) -> Result<()> {
    let pairs: Vec<(i64, Vec<u8>)> = {
        let r = db.begin_read()?;
        match r.open_table(CYCLES_V1) {
            Ok(t) => {
                let mut out = Vec::new();
                for entry in t.iter()? {
                    let (k, v) = entry?;
                    out.push((k.value(), v.value().to_vec()));
                }
                out
            }
            // No v1 table on disk (brand-new DB whose META was written but
            // CYCLES never was). Nothing to migrate.
            Err(_) => return Ok(()),
        }
    };

    let w = db.begin_write()?;
    {
        let mut new_table = w.open_table(CYCLES)?;
        for (sec, value) in &pairs {
            new_table.insert((*sec, 0u32), value.as_slice())?;
        }
    }
    w.delete_table(CYCLES_V1)?;
    w.commit()?;

    Ok(())
}

/// Shape of the legacy JSON file (`PersistedStats` in the old code).
/// Mirrors the on-disk format only — no behaviour attached.
#[derive(Debug, serde::Deserialize)]
struct LegacyPersistedStats {
    #[allow(dead_code)]
    schema_version: u32,
    tracking_started_unix_secs: u64,
    total_starts: u64,
    total_operating_secs: u64,
    cycle_history: std::collections::VecDeque<LegacyCycle>,
    daily_history: std::collections::VecDeque<LegacyDaily>,
}

#[derive(Debug, serde::Deserialize)]
struct LegacyCycle {
    timestamp: String,
    duration_secs: u64,
    outdoor_temp_c: Option<f32>,
}

#[derive(Debug, serde::Deserialize)]
struct LegacyDaily {
    date: String,
    starts: u32,
    operating_hours: f64,
    avg_outdoor_temp_c: Option<f64>,
}

fn parse_legacy_json(bytes: &[u8]) -> Option<LegacyPersistedStats> {
    serde_json::from_slice(bytes).ok()
}

/// Parse an ISO 8601 timestamp produced by chrono's `to_rfc3339_opts` to
/// unix seconds. Only the year-second precision is needed for ordering.
fn parse_iso8601_secs(s: &str) -> Option<i64> {
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(dt.timestamp())
}

fn parse_date_yyyymmdd(s: &str) -> Option<u32> {
    // Accept "YYYY-MM-DD" — produce 20260510 etc. Calendar-validated via
    // chrono so legacy-migration garbage (e.g. "1970-99-99", "2025-02-31")
    // doesn't get composed into a DAILY key that survives forever.
    use chrono::Datelike;
    let date = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    let y = u32::try_from(date.year()).ok()?;
    Some(y * 10_000 + date.month() * 100 + date.day())
}

pub mod poller;

#[cfg(test)]
mod tests;
