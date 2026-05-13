//! Modbus actor for CTC heating system
//!
//! This module provides an actor-based interface to the Modbus RTU protocol
//! for communicating with CTC heating systems. The actor ensures exclusive
//! access to the serial port and processes operations sequentially.

use crate::error::ModbusError;
use crate::modbus::bms_parameters::{ALARM_REF_MIN, INFO_REF_MAX};
use crate::modbus::{Access, CTCModbusParameter, SupervisorStats};
use hdrhistogram::Histogram;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout};
use tokio_modbus::client::Writer;
use tokio_modbus::prelude::{Reader, Slave, rtu};
use tokio_serial::SerialPortBuilderExt;
use tokio_serial::{DataBits, FlowControl, Parity, StopBits};
use tracing::{debug, error, info, trace, warn};

/// Response types for Modbus operations
#[derive(Debug, Clone)]
pub enum ModbusResponse {
    /// Scaled parameter value (for Read/Write operations)
    Value(f32),
    /// Raw register data (for `ReadRawRegisters`)
    RawRegisters { start: u16, values: Vec<u16> },
    /// Snapshot of the actor's telemetry counters. Boxed because the
    /// payload (`Vec`s of recent retries + per-register entries, plus
    /// percentile snapshots) would otherwise inflate every other variant's
    /// enum size.
    Stats(Box<ModbusStats>),
}

pub type ModbusResult = Result<ModbusResponse, ModbusError>;
pub type ResponseChannel = oneshot::Sender<ModbusResult>;
pub type ModbusRequest = (ParameterOperation, ResponseChannel);
pub type ModbusSender = mpsc::Sender<ModbusRequest>;

/// First visibility register address (inclusive)
const VISIBILITY_REG_START: u16 = 62500;
/// Last visibility register address (inclusive)
const VISIBILITY_REG_END: u16 = 62548;
/// Number of visibility registers to read
const VISIBILITY_REG_COUNT: usize = (VISIBILITY_REG_END - VISIBILITY_REG_START + 1) as usize; // 49

// Several call sites do `u16::try_from(idx)` on indices in 0..VISIBILITY_REG_COUNT.
// That works as long as the count fits in u16; assert it at compile time so a
// later range expansion doesn't silently start truncating.
const _: () = assert!(VISIBILITY_REG_COUNT < u16::MAX as usize);

#[derive(Debug)]
pub enum ParameterOperation {
    Read(CTCModbusParameter),
    // ReadVector(&Vec<'static CTCModbusParameter>),
    Write(CTCModbusParameter, f32),
    /// Read a specific visibility register (62500-62548)
    /// Returns the raw bitmask value as f32
    ReadVisibility(u16),
    /// Read all visibility registers (62500-62548 by default)
    /// Returns `ModbusResponse::RawRegisters` with all cached visibility values
    ReadAllVisibility,
    /// Read raw registers without scaling (Modbus function 0x03)
    /// Returns `ModbusResponse::RawRegisters`
    ReadRawRegisters {
        start: u16,
        count: u16,
    },
    /// Write a single raw register without scaling (Modbus function 0x06)
    /// Returns `ModbusResponse::Value` with the written value
    WriteRawRegister {
        register: u16,
        value: u16,
    },
    /// Snapshot the actor's telemetry counters.
    /// Returns `ModbusResponse::Stats` with a fully-owned snapshot.
    GetStats,
}

/// Whether a wire operation read from or wrote to the bus.
///
/// Threaded through `with_retry!` so per-register and per-histogram
/// bookkeeping can route to the right counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireOpKind {
    Read,
    Write,
}

/// Client-op variant counters. One field per `ParameterOperation` variant.
///
/// Bumped in the dispatch loop AFTER the disconnected-client early-return
/// so a request that's been hung up on does not skew the per-variant counts.
#[derive(Debug, Default, Clone, Serialize)]
pub(crate) struct OpCounts {
    pub read: u64,
    pub write: u64,
    pub read_visibility: u64,
    pub read_all_visibility: u64,
    pub read_raw_registers: u64,
    pub write_raw_register: u64,
    pub get_stats: u64,
}

impl OpCounts {
    fn bump(&mut self, op: &ParameterOperation) {
        match op {
            ParameterOperation::Read(_) => self.read += 1,
            ParameterOperation::Write(_, _) => self.write += 1,
            ParameterOperation::ReadVisibility(_) => self.read_visibility += 1,
            ParameterOperation::ReadAllVisibility => self.read_all_visibility += 1,
            ParameterOperation::ReadRawRegisters { .. } => self.read_raw_registers += 1,
            ParameterOperation::WriteRawRegister { .. } => self.write_raw_register += 1,
            ParameterOperation::GetStats => self.get_stats += 1,
        }
    }
}

/// Per-register counters tracking traffic patterns by Modbus address.
///
/// Updated inside `with_retry!` keyed on the macro's `$register` argument,
/// so both visibility-scan reads (start register), min/max/step reads
/// (`param.reg_max`), parameter reads/writes (`param.id`), and raw bulk
/// reads (start register) all land in the right entry.
#[derive(Debug, Default, Clone, Copy, Serialize)]
struct RegisterCounters {
    pub reads: u64,
    pub writes: u64,
    pub retry_attempts: u64,
    pub final_failures: u64,
}

/// Outcome of a single wire attempt.
///
/// `Exception(String)` carries the rendered error so the response shape
/// can show callers which Modbus exception fired without leaking the full
/// `ModbusError` enum.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "detail")]
pub(crate) enum AttemptOutcome {
    Success,
    Timeout,
    Exception(String),
}

/// Final outcome of a retry-emitting request.
///
/// Pure first-shot successes do not emit retry events at all, so this enum
/// only describes requests that hit at least one retry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FinalOutcome {
    Succeeded { attempts_used: u32 },
    FinalFailure { reason: String },
}

/// One attempt within a retry-emitting request.
///
/// `ms_since_prev_wire_op` is captured BEFORE this attempt's wire call —
/// for the first attempt of a request, that's "how long was the bus quiet
/// before this request started"; for retries it's "gap since the previous
/// attempt of this same request finished." For the first attempt ever the
/// previous-op timestamp is `None`, mapped to `0` per the response schema.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AttemptDetail {
    pub ms_since_prev_wire_op: u32,
    pub ms_since_request_first_attempt: u32,
    pub elapsed_ms: u32,
    pub outcome: AttemptOutcome,
}

/// A retry-emitting request, captured for the recent-retries ring.
///
/// Pushed exactly once per request that took at least one retry — whether
/// it eventually succeeded or final-failed.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RetryEvent {
    pub when_epoch_secs: u64,
    pub register: u16,
    pub op_name: &'static str,
    pub attempts: Vec<AttemptDetail>,
    pub final_outcome: FinalOutcome,
    pub total_struggle_ms: u32,
}

/// FIFO ring of the most recent retry-emitting requests. Cap-100 by design.
#[derive(Debug, Default)]
struct RetryRing {
    inner: VecDeque<RetryEvent>,
}

impl RetryRing {
    const CAP: usize = 100;

    pub fn push(&mut self, event: RetryEvent) {
        if self.inner.len() == Self::CAP {
            self.inner.pop_front();
        }
        self.inner.push_back(event);
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &RetryEvent> {
        self.inner.iter()
    }
}

/// Sliding window of recent wire-op timestamps, used to compute rates over
/// 10 s / 60 s / 5 min windows.
///
/// `push` evicts entries older than 5 min (the longest configured window)
/// so the deque stays roughly proportional to actual bus traffic, not to
/// process uptime.
#[derive(Debug, Default)]
struct RateWindow {
    inner: VecDeque<Instant>,
}

impl RateWindow {
    const MAX_AGE: Duration = Duration::from_secs(300);

    pub fn push(&mut self, now: Instant) {
        // Evict anything outside the longest window. Use checked_duration_since
        // to avoid panicking if the monotonic clock had a shenanigan.
        while let Some(front) = self.inner.front() {
            if now.checked_duration_since(*front).unwrap_or(Duration::ZERO) > Self::MAX_AGE {
                self.inner.pop_front();
            } else {
                break;
            }
        }
        self.inner.push_back(now);
    }

    pub fn count_within(&self, window: Duration) -> usize {
        let now = Instant::now();
        self.inner
            .iter()
            .filter(|t| now.checked_duration_since(**t).unwrap_or(Duration::ZERO) <= window)
            .count()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Snapshot of an HDR histogram's percentiles + mean + max + count.
///
/// All durations in milliseconds. Computed inside the actor on demand so
/// the response is fully owned (`Send + 'static`) and the actor's
/// histograms stay private.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HistogramSnapshot {
    pub count: u64,
    pub p50: u64,
    pub p90: u64,
    pub p99: u64,
    pub p99_9: u64,
    pub max: u64,
    pub mean: f64,
}

impl HistogramSnapshot {
    fn from_histo(h: &Histogram<u64>) -> Self {
        Self {
            count: h.len(),
            p50: h.value_at_percentile(50.0),
            p90: h.value_at_percentile(90.0),
            p99: h.value_at_percentile(99.0),
            p99_9: h.value_at_percentile(99.9),
            max: h.max(),
            mean: h.mean(),
        }
    }
}

/// One `registers[]` entry in the stats response.
///
/// Flat shape so the JSON has stable key names rather than nested map
/// objects.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RegisterCountsEntry {
    pub address: u16,
    pub reads: u64,
    pub writes: u64,
    pub retry_attempts: u64,
    pub final_failures: u64,
}

/// Snapshot of supervisor-level counters (across actor respawns).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SupervisorStatsSnapshot {
    pub respawns: u32,
    pub port_open_failures: u32,
    pub last_respawn_epoch_secs: Option<u64>,
    pub actor_uptime_secs: u64,
}

/// Snapshot of client-side aggregate counters.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ClientOpStats {
    pub total: u64,
    pub by_kind: OpCounts,
}

/// Snapshot of wire-side aggregate counters + rate windows.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct WireOpStats {
    pub total: u64,
    pub timeouts: u64,
    pub retry_attempts: u64,
    pub final_failures: u64,
    pub consecutive_failures: u32,
    pub max_consecutive_failures: u32,
    pub ms_since_last_success: Option<u64>,
    pub ms_since_last_wire_op: Option<u64>,
    pub per_sec_last_10s: f64,
    pub per_sec_last_60s: f64,
    pub per_sec_last_5min: f64,
}

/// Full stats payload returned by `ParameterOperation::GetStats`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ModbusStats {
    pub actor_started_at_secs: u64,
    pub client_ops: ClientOpStats,
    pub wire_ops: WireOpStats,
    pub supervisor: SupervisorStatsSnapshot,
    pub read_durations_ms: HistogramSnapshot,
    pub write_durations_ms: HistogramSnapshot,
    pub registers: Vec<RegisterCountsEntry>,
    pub recent_retries: Vec<RetryEvent>,
}

/// Wall-clock seconds since `UNIX_EPOCH`, saturating on pre-epoch clock
/// readings (which would be a system misconfiguration, not a normal case).
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Saturating cast of a `Duration` to a `u32` count of milliseconds.
///
/// Used pervasively by the retry-event bookkeeping in `with_retry!`. A
/// duration longer than ~49 days would saturate at `u32::MAX`; we never see
/// anything close to that on a wire op (`operation_timeout` truncates at 1 s).
pub(crate) fn ms_u32(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

pub struct CtcActor {
    context: tokio_modbus::client::Context,
    // Timeout and retry configuration
    operation_timeout: Duration,
    max_retries: u32,
    initial_retry_delay: Duration,
    backoff_multiplier: f64,
    max_consecutive_failures: u32,
    /// Minimum gap between consecutive Modbus transactions. Enforced inside
    /// `with_retry!` so the CTC firmware has guaranteed settle time between
    /// back-to-back reads — without this, polled bursts from the sensor loop
    /// occasionally overlap with the device's internal processing and drop
    /// the response, triggering an `attempt 1/3 timeout` warning.
    inter_request_gap: Duration,
    /// When the last wire transaction completed. Used to compute the actual
    /// sleep needed before the next one.
    last_wire_op: Option<Instant>,
    // Tracking fields
    consecutive_failures: u32,
    /// Highest `consecutive_failures` ever observed on this actor instance.
    /// Resets on respawn.
    max_consecutive_failures_seen: u32,
    last_success: Option<Instant>,
    total_operations: u64,
    total_failures: u64,
    // Visibility cache: registers 62500-62548, lazy-loaded on first access
    visibility_cache: Option<[u16; VISIBILITY_REG_COUNT]>,

    // Telemetry — all fields below reset on actor respawn. The shared
    // `sup_stats` does not; it tracks the respawns themselves.
    actor_started_at: Instant,
    read_histo: Histogram<u64>,
    write_histo: Histogram<u64>,
    op_counts: OpCounts,
    per_register: HashMap<u16, RegisterCounters>,
    retry_ring: RetryRing,
    rate_window: RateWindow,
    /// Every attempt that hit the bus, retries included.
    total_wire_ops: u64,
    /// Wire attempts that hit the `operation_timeout`.
    total_wire_timeouts: u64,
    /// Wire attempts past the first (i.e. retries that ran).
    total_wire_retry_attempts: u64,
    sup_stats: Arc<SupervisorStats>,
}

/// Cap on the number of distinct register addresses tracked in
/// `per_register`. The CTC parameter set is well under 100 distinct
/// addresses; hitting this cap means we have a bug emitting fresh
/// addresses on the hot path.
const PER_REGISTER_CAP: usize = 256;

#[allow(dead_code)]
#[derive(Clone)]
pub struct CtcActorBuilder {
    tty_path: String,
    baud_rate: u32,
    data_bits: DataBits,
    parity: Parity,
    stop_bits: StopBits,
    flow_control: FlowControl,
    timeout: Duration,
    slave_id: u8,
    // Timeout and retry configuration
    operation_timeout: Duration,
    max_retries: u32,
    initial_retry_delay: Duration,
    backoff_multiplier: f64,
    max_consecutive_failures: u32,
    inter_request_gap: Duration,
    sup_stats: Arc<SupervisorStats>,
}

#[allow(dead_code)]
impl CtcActorBuilder {
    /// Create a new builder with just the TTY path
    /// All other parameters should be set via builder methods
    pub fn new(tty_path: impl Into<String>) -> Self {
        Self {
            tty_path: tty_path.into(),
            baud_rate: 9600,                           // Will be overridden by config
            data_bits: DataBits::Eight,                // Will be overridden by config
            parity: Parity::Even,                      // Will be overridden by config
            stop_bits: StopBits::One,                  // Will be overridden by config
            flow_control: FlowControl::Hardware,       // Will be overridden by config
            timeout: Duration::from_secs(1),           // Will be overridden by config
            slave_id: 1,                               // Will be overridden by config
            operation_timeout: Duration::from_secs(5), // Will be overridden by config
            max_retries: 2,                            // Will be overridden by config
            initial_retry_delay: Duration::from_millis(100), // Will be overridden by config
            backoff_multiplier: 2.0,                   // Will be overridden by config
            max_consecutive_failures: 5,               // Will be overridden by config
            inter_request_gap: Duration::from_millis(10), // Will be overridden by config
            sup_stats: Arc::new(SupervisorStats::default()),
        }
    }

    /// Replace the supervisor stats Arc. Callers should hand the same
    /// `Arc<SupervisorStats>` to both this builder and any reader that
    /// wants to observe the counters — the builder clones it into the
    /// actor on `build()`.
    pub fn sup_stats(mut self, sup_stats: Arc<SupervisorStats>) -> Self {
        self.sup_stats = sup_stats;
        self
    }

    pub fn baud_rate(mut self, baud_rate: u32) -> Self {
        self.baud_rate = baud_rate;
        self
    }

    pub fn data_bits(mut self, data_bits: DataBits) -> Self {
        self.data_bits = data_bits;
        self
    }

    pub fn parity(mut self, parity: Parity) -> Self {
        self.parity = parity;
        self
    }

    pub fn stop_bits(mut self, stop_bits: StopBits) -> Self {
        self.stop_bits = stop_bits;
        self
    }

    pub fn flow_control(mut self, flow_control: FlowControl) -> Self {
        self.flow_control = flow_control;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn slave_id(mut self, slave_id: u8) -> Self {
        self.slave_id = slave_id;
        self
    }

    pub fn operation_timeout(mut self, operation_timeout: Duration) -> Self {
        self.operation_timeout = operation_timeout;
        self
    }

    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn initial_retry_delay(mut self, initial_retry_delay: Duration) -> Self {
        self.initial_retry_delay = initial_retry_delay;
        self
    }

    pub fn backoff_multiplier(mut self, backoff_multiplier: f64) -> Self {
        self.backoff_multiplier = backoff_multiplier;
        self
    }

    pub fn max_consecutive_failures(mut self, max_consecutive_failures: u32) -> Self {
        self.max_consecutive_failures = max_consecutive_failures;
        self
    }

    pub fn inter_request_gap(mut self, inter_request_gap: Duration) -> Self {
        self.inter_request_gap = inter_request_gap;
        self
    }

    pub fn build(&self) -> io::Result<CtcActor> {
        // Set up the serial port
        let port = tokio_serial::new(&self.tty_path, self.baud_rate)
            .baud_rate(self.baud_rate)
            .data_bits(self.data_bits)
            .parity(self.parity)
            .stop_bits(self.stop_bits)
            .flow_control(self.flow_control)
            .timeout(self.timeout)
            .open_native_async()?;
        info!("ctc_actor::build: Serial port opened at {}", self.tty_path);

        // Create the Modbus RTU context
        let ctx = rtu::attach_slave(port, Slave(self.slave_id));

        // HDR bounds: 1 ms .. 60 s with 3 significant figures of precision.
        // The bounds are compile-time constants and `new_with_bounds` only
        // fails on bad inputs (lo<=0, hi<=lo, sig_fig out of 0..=5), so the
        // `expect` is justified.
        let read_histo = Histogram::<u64>::new_with_bounds(1, 60_000, 3)
            .expect("HDR bounds known valid at compile time");
        let write_histo = Histogram::<u64>::new_with_bounds(1, 60_000, 3)
            .expect("HDR bounds known valid at compile time");

        Ok(CtcActor {
            context: ctx,
            operation_timeout: self.operation_timeout,
            max_retries: self.max_retries,
            initial_retry_delay: self.initial_retry_delay,
            backoff_multiplier: self.backoff_multiplier,
            max_consecutive_failures: self.max_consecutive_failures,
            inter_request_gap: self.inter_request_gap,
            last_wire_op: None,
            consecutive_failures: 0,
            max_consecutive_failures_seen: 0,
            last_success: None,
            total_operations: 0,
            total_failures: 0,
            visibility_cache: None,
            actor_started_at: Instant::now(),
            read_histo,
            write_histo,
            op_counts: OpCounts::default(),
            per_register: HashMap::new(),
            retry_ring: RetryRing::default(),
            rate_window: RateWindow::default(),
            total_wire_ops: 0,
            total_wire_timeouts: 0,
            total_wire_retry_attempts: 0,
            sup_stats: Arc::clone(&self.sup_stats),
        })
    }

    /// Spawn the actor under a supervisor task that respawns it on unexpected exit.
    ///
    /// The supervisor owns the request `receiver` across respawns, so the
    /// `mpsc::Sender` held by the rest of the application remains valid even
    /// when the underlying actor task exits or panics. On exit (clean or via
    /// panic) the supervisor sleeps briefly, rebuilds the actor (which reopens
    /// the serial port), and resumes processing requests.
    ///
    /// Records supervisor events into the `Arc<SupervisorStats>` stored on
    /// the builder: a failed `build()` bumps `port_open_failures`; a `run()`
    /// exit (clean or via panic) bumps `respawns` and records the wall-clock
    /// timestamp. The FIRST successful `build()` does NOT count as a respawn
    /// — that's the first start, not a rebuild.
    pub fn spawn_supervised(self, receiver: mpsc::Receiver<ModbusRequest>) {
        use futures_util::FutureExt;
        use std::panic::AssertUnwindSafe;

        tokio::spawn(async move {
            const RESPAWN_DELAY: Duration = Duration::from_secs(1);
            let mut receiver = receiver;
            loop {
                match self.build() {
                    Ok(mut actor) => {
                        info!("ctc_actor::supervisor: Starting actor loop");
                        let result = AssertUnwindSafe(actor.run(&mut receiver))
                            .catch_unwind()
                            .await;
                        // Whether the loop exited cleanly or via panic, the
                        // supervisor is about to rebuild — that's a respawn
                        // regardless of whether the next `build()` succeeds.
                        self.sup_stats.record_respawn();
                        match result {
                            Ok(()) => {
                                error!(
                                    "ctc_actor::supervisor: Actor loop exited; respawning after {:?}",
                                    RESPAWN_DELAY
                                );
                            }
                            Err(_panic) => {
                                error!(
                                    "ctc_actor::supervisor: Actor loop panicked; respawning after {:?}",
                                    RESPAWN_DELAY
                                );
                            }
                        }
                    }
                    Err(e) => {
                        self.sup_stats.record_build_failure();
                        error!(
                            "ctc_actor::supervisor: Failed to build actor: {e}; retrying in {:?}",
                            RESPAWN_DELAY
                        );
                    }
                }
                sleep(RESPAWN_DELAY).await;
            }
        });
    }
}

/// Retry macro for Modbus operations with exponential backoff.
///
/// # Type constraints
/// - `$operation` must be an async expression yielding `Result<T, ModbusError>`
/// - `$kind` must be a `WireOpKind` expression (Read or Write); routes the
///   per-register `reads`/`writes` counter and selects which histogram
///   records the success-path latency.
/// - Returns `Result<T, ModbusError>`
///
/// # Important
/// - The `$operation` expression is evaluated INSIDE the loop, creating a fresh future
///   each iteration. Never poll the same future twice.
/// - On final failure, returns the stored `ModbusError` (not stringified), allowing
///   structured logging before conversion to `ApiError`.
///
/// # Telemetry contract
/// - Emits one `RetryEvent` to the retry ring per request that needed at
///   least one retry. Pure first-shot successes emit nothing.
/// - Per-register `reads` or `writes` counter bumps on EVERY attempt
///   (regardless of outcome). Per-register `retry_attempts` bumps on
///   each failed attempt (including a failed attempt 0). Per-register
///   `final_failures` bumps once per request whose final outcome is a
///   failure — either retry exhaustion OR a non-transient error that
///   short-circuited the retry loop on attempt 0.
/// - Aggregate `total_wire_ops` bumps on every attempt;
///   `total_wire_timeouts` on `Err(_elapsed)` timeouts;
///   `total_wire_retry_attempts` on attempts past the first (i.e. only
///   retries that actually ran — so this is NOT directly comparable to
///   `sum(registers[].retry_attempts)`).
/// - HDR histogram records only on `Ok(Ok(_))` — failed attempts'
///   "elapsed" reflects the `operation_timeout`, not real bus latency.
/// - `rate_window.push(Instant::now())` on every attempt, retries
///   included.
macro_rules! with_retry {
    ($self:expr, $op_name:expr, $register:expr, $kind:expr, $operation:expr) => {{
        use crate::modbus::actor::{
            AttemptDetail, AttemptOutcome, FinalOutcome, PER_REGISTER_CAP, RetryEvent, WireOpKind,
            ms_u32, now_epoch_secs,
        };

        let mut last_error: Option<ModbusError> = None;

        // Sampled once per request so the eventual RetryEvent's
        // `when_epoch_secs` and `total_struggle_ms` describe the same instant.
        let request_when_epoch = now_epoch_secs();
        let request_started_at = Instant::now();
        let mut attempts: Vec<AttemptDetail> = Vec::with_capacity(
            usize::try_from($self.max_retries)
                .unwrap_or(usize::MAX)
                .saturating_add(1),
        );

        // Builds the RetryEvent for either a success-after-retry or final
        // failure. Captures only Copy values — no `$self` borrow — so the
        // caller is free to mutate the actor while constructing the event.
        let make_event = |attempts: Vec<AttemptDetail>, final_outcome: FinalOutcome| RetryEvent {
            when_epoch_secs: request_when_epoch,
            register: $register,
            op_name: $op_name,
            attempts,
            final_outcome,
            total_struggle_ms: ms_u32(request_started_at.elapsed()),
        };

        let result: Result<_, ModbusError> = 'retry: {
            for attempt in 0..=$self.max_retries {
                if attempt > 0 {
                    $self.total_wire_retry_attempts += 1;
                    let delay = $self.calculate_retry_delay(attempt);
                    trace!(
                        "Retry attempt {}/{} for {} (register {}), delay: {}ms",
                        attempt,
                        $self.max_retries,
                        $op_name,
                        $register,
                        delay.as_millis()
                    );
                    sleep(delay).await;
                }

                CtcActor::wait_for_inter_request_gap($self.inter_request_gap, $self.last_wire_op)
                    .await;

                // Re-read each iteration so a retry sees the just-completed
                // previous attempt's end-time, not the request's entry-time.
                let prev_wire_op = $self.last_wire_op;
                let attempt_start = Instant::now();

                // $operation MUST be evaluated inside the loop to build a
                // fresh future each iteration.
                let future = $operation;
                let result = timeout($self.operation_timeout, future).await;

                // One clock read for last_wire_op, rate_window, and
                // wire_elapsed — they describe a single event.
                let wire_end = Instant::now();
                let wire_elapsed = wire_end.duration_since(attempt_start);
                $self.last_wire_op = Some(wire_end);
                $self.total_wire_ops += 1;
                $self.rate_window.push(wire_end);

                // Cap-guarded insert-or-bump on the per-register map.
                {
                    let key: u16 = $register;
                    if $self.per_register.contains_key(&key)
                        || $self.per_register.len() < PER_REGISTER_CAP
                    {
                        let c = $self.per_register.entry(key).or_default();
                        match $kind {
                            WireOpKind::Read => c.reads += 1,
                            WireOpKind::Write => c.writes += 1,
                        }
                    } else {
                        warn!(
                            "ctc_actor::with_retry: per_register cap ({}) reached; \
                             dropping new register {}",
                            PER_REGISTER_CAP, key
                        );
                    }
                }

                let gap_ms: u32 = prev_wire_op
                    .map(|t| {
                        ms_u32(
                            attempt_start
                                .checked_duration_since(t)
                                .unwrap_or(Duration::ZERO),
                        )
                    })
                    .unwrap_or(0);
                let since_first_ms: u32 = ms_u32(
                    attempt_start
                        .checked_duration_since(request_started_at)
                        .unwrap_or(Duration::ZERO),
                );
                let elapsed_ms: u32 = ms_u32(wire_elapsed);

                // Captures Copy values only, so this doesn't borrow
                // `attempts` and the caller can still push into it.
                let make_attempt = |outcome: AttemptOutcome| AttemptDetail {
                    ms_since_prev_wire_op: gap_ms,
                    ms_since_request_first_attempt: since_first_ms,
                    elapsed_ms,
                    outcome,
                };

                match result {
                    Ok(Ok(value)) => {
                        $self.record_success();
                        // .ok() swallows the OOR error — a 60+ second op
                        // would saturate, but operation_timeout truncates
                        // well before that in practice.
                        let elapsed_u64 =
                            u64::try_from(wire_elapsed.as_millis()).unwrap_or(u64::MAX);
                        match $kind {
                            WireOpKind::Read => {
                                $self.read_histo.record(elapsed_u64).ok();
                            }
                            WireOpKind::Write => {
                                $self.write_histo.record(elapsed_u64).ok();
                            }
                        }
                        attempts.push(make_attempt(AttemptOutcome::Success));
                        trace!(
                            "{} succeeded on attempt {} (register {})",
                            $op_name,
                            attempt + 1,
                            $register
                        );
                        // Past attempt 0 means the request needed a retry
                        // to succeed — emit one event.
                        if attempt > 0 {
                            $self.retry_ring.push(make_event(
                                attempts,
                                FinalOutcome::Succeeded {
                                    attempts_used: attempt + 1,
                                },
                            ));
                        }
                        break 'retry Ok(value);
                    }
                    Ok(Err(e)) => {
                        let transient = e.is_transient();
                        let exc_text = format!("{e}");
                        warn!(
                            "{} failed on attempt {}/{}: {} (register {})",
                            $op_name,
                            attempt + 1,
                            $self.max_retries + 1,
                            e,
                            $register
                        );
                        attempts.push(make_attempt(AttemptOutcome::Exception(exc_text)));
                        if let Some(c) = $self.per_register.get_mut(&$register) {
                            c.retry_attempts += 1;
                        }
                        last_error = Some(e);
                        if !transient {
                            break;
                        }
                    }
                    Err(_elapsed) => {
                        $self.total_wire_timeouts += 1;
                        let timeout_err = ModbusError::Timeout {
                            register: $register,
                            operation: format!(
                                "{} timed out after {:?}",
                                $op_name, $self.operation_timeout
                            ),
                        };
                        warn!(
                            "{} timeout on attempt {}/{} (register {})",
                            $op_name,
                            attempt + 1,
                            $self.max_retries + 1,
                            $register
                        );
                        attempts.push(make_attempt(AttemptOutcome::Timeout));
                        if let Some(c) = $self.per_register.get_mut(&$register) {
                            c.retry_attempts += 1;
                        }
                        last_error = Some(timeout_err);
                    }
                }
            }

            $self.record_failure();
            if let Some(c) = $self.per_register.get_mut(&$register) {
                c.final_failures += 1;
            }

            let final_error = last_error.unwrap_or_else(|| ModbusError::ProtocolError {
                reason: format!("{}: no error captured during retries", $op_name),
            });

            error!(
                "{} failed after {} attempts (register {}): {}",
                $op_name,
                $self.max_retries + 1,
                $register,
                final_error
            );

            $self.retry_ring.push(make_event(
                attempts,
                FinalOutcome::FinalFailure {
                    reason: format!("{final_error}"),
                },
            ));

            // Return the structured error (not stringified) for logging.
            Err(final_error)
        };

        result
    }};
}

/// Check if parameter is visible against an (optionally populated) cache.
///
/// `visible == 0` always returns `Ok(true)` so registers like
/// `CTC_ALARM_INFO_BUFFER` remain accessible even if the visibility scan
/// failed.
///
/// When `cache` is `None` (scan failed) the function falls back to the
/// optimistic "assume visible" path rather than poisoning every read. Reads
/// may then attempt registers the device doesn't actually support and get a
/// clean Modbus exception, but the actor is not stuck.
fn check_visibility_against(
    cache: Option<&[u16; VISIBILITY_REG_COUNT]>,
    param: &CTCModbusParameter,
) -> Result<bool, ModbusError> {
    // visible == 0 means always visible — check BEFORE touching cache.
    if param.visible == 0 {
        return Ok(true);
    }

    let Some(cache) = cache else {
        trace!(
            "ctc_actor::check_visibility: cache unavailable, assuming register {} visible",
            param.id
        );
        return Ok(true);
    };

    if param.visible < VISIBILITY_REG_START || param.visible > VISIBILITY_REG_END {
        return Err(ModbusError::InvalidVisibilityRegister(param.visible));
    }

    let index = (param.visible - VISIBILITY_REG_START) as usize;
    Ok(param.is_visible(cache[index]))
}

impl CtcActor {
    /// Calculate exponential backoff delay for a given retry attempt
    ///
    /// # Arguments
    /// * `attempt` - The current retry attempt number (0-indexed)
    ///
    /// # Returns
    /// Duration to wait before the next retry
    fn calculate_retry_delay(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            Duration::from_millis(0)
        } else {
            // Cap the computed delay before the f64 -> u64 cast. With a large
            // multiplier or many attempts, `powi` can overflow to `f64::INFINITY`
            // and saturate to `u64::MAX` on cast, producing an effectively
            // infinite sleep. Clamp to 60 seconds.
            const MAX_DELAY_MS: f64 = 60_000.0;
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            #[allow(clippy::cast_precision_loss)]
            #[allow(clippy::cast_possible_wrap)]
            let raw_ms = self.initial_retry_delay.as_millis() as f64
                * self.backoff_multiplier.powi(attempt as i32 - 1);
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            let delay_ms = raw_ms.min(MAX_DELAY_MS) as u64;
            Duration::from_millis(delay_ms)
        }
    }

    /// Sleep just long enough that at least `inter_request_gap` has elapsed
    /// since the last wire transaction. First call after construction (or
    /// after a respawn) is free; subsequent calls only pay the remainder.
    ///
    /// Takes `Duration` + `Option<Instant>` by value so the returned future
    /// captures only `Copy` values — keeping `&CtcActor` out of the future
    /// matters because the actor is `!Sync` (the `tokio_modbus` context isn't).
    async fn wait_for_inter_request_gap(gap: Duration, last: Option<Instant>) {
        if gap.is_zero() {
            return;
        }
        if let Some(last) = last {
            let elapsed = last.elapsed();
            if elapsed < gap {
                sleep(gap.saturating_sub(elapsed)).await;
            }
        }
    }

    /// Record successful operation
    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_success = Some(Instant::now());
        self.total_operations += 1;
    }

    /// Record failed operation and check if critical threshold reached
    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.total_failures += 1;
        if self.consecutive_failures > self.max_consecutive_failures_seen {
            self.max_consecutive_failures_seen = self.consecutive_failures;
        }

        if self.consecutive_failures >= self.max_consecutive_failures {
            error!(
                "CRITICAL: {} consecutive Modbus failures detected (total operations: {}, total failures: {})",
                self.consecutive_failures, self.total_operations, self.total_failures
            );
        }
    }

    /// Batch read all visibility registers in one Modbus call
    ///
    /// Reads the configured visibility register range (62500-62548 by default)
    /// containing visibility bitmasks for hardware capability detection. The
    /// cache is populated lazily on first parameter access.
    async fn scan_visibility(&mut self) -> Result<(), ModbusError> {
        info!(
            "Scanning visibility registers ({} registers starting at {})",
            VISIBILITY_REG_COUNT, VISIBILITY_REG_START
        );

        #[allow(clippy::cast_possible_truncation)]
        let values: Vec<u16> = with_retry!(
            self,
            "scan_visibility",
            VISIBILITY_REG_START,
            WireOpKind::Read,
            async {
                self.context
                    .read_holding_registers(VISIBILITY_REG_START, VISIBILITY_REG_COUNT as u16)
                    .await
                    .map_err(|e| ModbusError::ReadError {
                        register: VISIBILITY_REG_START,
                        reason: e.to_string(),
                    })
                    .and_then(|result| {
                        result.map_err(|e| ModbusError::ReadError {
                            register: VISIBILITY_REG_START,
                            reason: format!("Modbus exception: {e}"),
                        })
                    })
            }
        )?;

        // EXPLICIT length check before try_into() for clear error path
        if values.len() != VISIBILITY_REG_COUNT {
            return Err(ModbusError::ReadError {
                register: VISIBILITY_REG_START,
                reason: format!(
                    "Expected {} visibility registers, got {}",
                    VISIBILITY_REG_COUNT,
                    values.len()
                ),
            });
        }

        // Now try_into() is guaranteed to succeed
        let cache: [u16; VISIBILITY_REG_COUNT] =
            values.try_into().expect("length already validated");

        self.visibility_cache = Some(cache);
        info!(
            "Visibility cache populated with {} registers",
            VISIBILITY_REG_COUNT
        );
        Ok(())
    }

    /// Check if parameter is visible on this hardware
    ///
    /// # Important
    /// `visible == 0` returns true WITHOUT touching the cache, ensuring registers
    /// like `CTC_ALARM_INFO_BUFFER` remain accessible even if visibility scan fails.
    ///
    /// If the visibility cache has not been populated (initial scan failed),
    /// fall back to optimistic "assume visible" rather than poisoning every read.
    /// Reads may then attempt registers the device doesn't actually support, but
    /// will get a clean Modbus exception rather than the actor being stuck.
    fn check_visibility(&self, param: &CTCModbusParameter) -> Result<bool, ModbusError> {
        check_visibility_against(self.visibility_cache.as_ref(), param)
    }

    async fn read_parameter(&mut self, param: &CTCModbusParameter) -> Result<f32, ModbusError> {
        let raw = self.read_parameter_raw(param).await?;
        Ok(param.get_scaled_value(raw))
    }

    /// Read a parameter and return the raw u16 register value without scaling.
    ///
    /// Used by write verification to compare against the raw value that was
    /// actually written, avoiding floating-point mismatches when the user's
    /// scaled value doesn't align with the register's factor.
    async fn read_parameter_raw(&mut self, param: &CTCModbusParameter) -> Result<u16, ModbusError> {
        with_retry!(
            self,
            "read_holding_registers",
            param.id,
            WireOpKind::Read,
            async {
                self.context
                .read_holding_registers(param.id, 1)
                .await
                .map_err(|e| ModbusError::ProtocolError {
                    reason: format!("Error reading register {}: {e}", param.id),
                })
                .and_then(|raw_values| {
                    raw_values
                        .map_err(|e| ModbusError::ReadError {
                            register: param.id,
                            reason: format!("{e}"),
                        })
                        .and_then(|raw_values| {
                            trace!(
                                "ctc_actor::read_parameter_raw: Raw values for parameter {:?}: {:?}",
                                param, raw_values
                            );
                            raw_values.first().copied().ok_or_else(|| {
                                ModbusError::ReadError {
                                    register: param.id,
                                    reason: "No value returned".to_string(),
                                }
                            })
                        })
                })
            }
        )
    }

    async fn read_min_max_step(
        &mut self,
        param: &CTCModbusParameter,
    ) -> Result<(u16, u16, u16), ModbusError> {
        let Some(reg_max) = param.reg_max else {
            return Err(ModbusError::ValidationReadError {
                register: param.id,
                reason: "Parameter not configured for min/max reading".to_string(),
            });
        };

        let param_id = param.id;

        with_retry!(
            self,
            "read_validation_parameters",
            reg_max,
            WireOpKind::Read,
            async {
                self.context
                    .read_holding_registers(reg_max, 3)
                    .await
                    .map_err(|e| ModbusError::ProtocolError {
                        reason: format!(
                            "Error reading validation parameters at register {reg_max}: {e}"
                        ),
                    })
                    .and_then(|raw_values| {
                        raw_values
                            .map_err(|e| ModbusError::ValidationReadError {
                                register: param_id,
                                reason: format!("{e}"),
                            })
                            .and_then(|raw_values| {
                                if raw_values.len() < 3 {
                                    return Err(ModbusError::ValidationReadError {
                                        register: param_id,
                                        reason: format!(
                                            "Expected 3 values, got {}",
                                            raw_values.len()
                                        ),
                                    });
                                }
                                trace!(
                                    "ctc_actor::read_min_max: Raw values max/min {:?}",
                                    raw_values
                                );
                                Ok((raw_values[0], raw_values[1], raw_values[2]))
                            })
                    })
            }
        )
    }

    #[allow(clippy::too_many_lines)]
    async fn write_parameter(
        &mut self,
        param: &CTCModbusParameter,
        value: f32,
    ) -> Result<(), ModbusError> {
        trace!(
            "ctc_actor::write_parameter: START - param={:?}, value={}",
            param, value
        );

        if param.is_read_only() {
            error!(
                "ctc_actor::write_parameter: Parameter {} is read-only",
                param.id
            );
            return Err(ModbusError::ReadOnly { register: param.id });
        }

        // Special validation for alarm/info text buffer (register 65100)
        // Must be in alarm range (ALARM_REF_MIN..=ALARM_REF_MAX) or
        // info range (INFO_REF_OFFSET..=INFO_REF_MAX)
        if param.id == 65100 {
            // Check for negative values or values outside valid range
            if !(f32::from(ALARM_REF_MIN)..=f32::from(INFO_REF_MAX)).contains(&value) {
                error!(
                    "ctc_actor::write_parameter: Invalid alarm/info value: {}",
                    value
                );
                return Err(ModbusError::InvalidAlarmInfoValue(value));
            }
            trace!(
                "ctc_actor::write_parameter: Alarm/info value {} validated",
                value
            );
        }

        let Some(raw_value) = param.get_raw_value(value) else {
            error!(
                "ctc_actor::write_parameter: value {} out of representable u16 range for register {}",
                value, param.id
            );
            let (min, max) = if param.signed {
                (
                    f32::from(i16::MIN) * param.factor,
                    f32::from(i16::MAX) * param.factor,
                )
            } else {
                (0.0, f32::from(u16::MAX) * param.factor)
            };
            return Err(ModbusError::OutOfRange {
                value,
                min,
                max,
                register: param.id,
            });
        };
        trace!(
            "ctc_actor::write_parameter: Converted to raw_value={}",
            raw_value
        );

        // Skip min/max/step validation for write-only registers (they don't have these)
        if param.access == Access::W {
            trace!(
                "ctc_actor::write_parameter: Write-only register, skipping min/max/step validation"
            );
        } else {
            trace!("ctc_actor::write_parameter: Reading min/max/step for validation");
            let (max, min, step) = self.read_min_max_step(param).await?;

            trace!(
                "ctc_actor::write_parameter: Validation bounds - min={}, max={}, step={}",
                min, max, step
            );

            // Check if value is within range
            if raw_value < min || raw_value > max {
                error!(
                    "ctc_actor::write_parameter: VALIDATION FAILED - raw_value={} not in range [{}, {}]",
                    raw_value, min, max
                );
                return Err(ModbusError::OutOfRange {
                    value,
                    min: param.get_scaled_value(min),
                    max: param.get_scaled_value(max),
                    register: param.id,
                });
            }

            // Check if value is valid step from minimum
            if !(raw_value - min).is_multiple_of(step) {
                error!(
                    "ctc_actor::write_parameter: VALIDATION FAILED - raw_value={} not valid step from min",
                    raw_value
                );
                return Err(ModbusError::InvalidStep {
                    value,
                    min: param.get_scaled_value(min),
                    step: param.get_scaled_value(step),
                    register: param.id,
                });
            }

            trace!("ctc_actor::write_parameter: Validation PASSED");
        }

        trace!(
            "ctc_actor::write_parameter: Calling Modbus write_single_register for register {}",
            param.id
        );

        with_retry!(
            self,
            "write_single_register",
            param.id,
            WireOpKind::Write,
            async {
                self.context
                    .write_single_register(param.id, raw_value)
                    .await
                    .map_err(|e| {
                        error!("ctc_actor::write_parameter: Modbus write FAILED: {}", e);
                        ModbusError::WriteError {
                            register: param.id,
                            value,
                            reason: format!("{e}"),
                        }
                    })
                    .and_then(|result| {
                        result.map_err(|e| ModbusError::WriteError {
                            register: param.id,
                            value,
                            reason: format!("Modbus exception: {e}"),
                        })
                    })
            }
        )
    }

    /// Handle a read operation
    async fn handle_read_operation(
        &mut self,
        param: &CTCModbusParameter,
        respond_to: ResponseChannel,
    ) {
        trace!("ctc_actor::run: Operation=READ, parameter={:?}", param);

        // Lazy init: scan visibility on first access. If the scan fails we
        // continue without a cache; check_visibility falls back to optimistic
        // "assume visible" so a single transient bus glitch at boot doesn't
        // poison every subsequent read.
        if self.visibility_cache.is_none()
            && let Err(e) = self.scan_visibility().await
        {
            warn!(
                "ctc_actor::run: Visibility scan FAILED, proceeding without cache: {}",
                e
            );
        }

        // Check visibility - NO record_failure() here, this is a client error not I/O failure
        match self.check_visibility(param) {
            Ok(false) => {
                // Parameter not available on this hardware - client error, not I/O failure
                trace!(
                    "ctc_actor::run: Parameter {} not visible on this hardware",
                    param.id
                );
                respond_to
                    .send(Err(ModbusError::ParameterNotVisible { register: param.id }))
                    .ok();
                return;
            }
            Err(e) => {
                // Invalid visibility register config - also not an I/O failure
                error!("ctc_actor::run: Visibility check error: {}", e);
                respond_to.send(Err(e)).ok();
                return;
            }
            Ok(true) => {} // Continue with read
        }

        match self.read_parameter(param).await {
            Ok(value) => {
                trace!(
                    "ctc_actor::run: Read SUCCESS, value={}, sending response",
                    value
                );
                respond_to
                    .send(Ok(ModbusResponse::Value(value)))
                    .unwrap_or_else(|_| {
                        debug!("ctc_actor::run: Client disconnected before response sent");
                    });
                trace!("ctc_actor::run: Read response sent");
            }
            Err(e) => {
                error!("ctc_actor::run: Read FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|_| {
                    debug!("ctc_actor::run: Client disconnected before error response sent");
                });
                trace!("ctc_actor::run: Read error response sent");
            }
        }
    }

    /// Handle a visibility register read operation
    ///
    /// Reads a specific visibility register (62500-62548) and returns the raw bitmask.
    /// Lazy-initializes the visibility cache on first access.
    async fn handle_visibility_operation(&mut self, register: u16, respond_to: ResponseChannel) {
        trace!(
            "ctc_actor::run: Operation=READ_VISIBILITY, register={}",
            register
        );

        // Validate register is in range
        if !(VISIBILITY_REG_START..=VISIBILITY_REG_END).contains(&register) {
            error!(
                "ctc_actor::run: Invalid visibility register {register} (valid range: {VISIBILITY_REG_START}-{VISIBILITY_REG_END})"
            );
            respond_to
                .send(Err(ModbusError::InvalidVisibilityRegister(register)))
                .ok();
            return;
        }

        // Lazy init: scan visibility on first access
        if self.visibility_cache.is_none()
            && let Err(e) = self.scan_visibility().await
        {
            error!("ctc_actor::run: Visibility scan FAILED: {}", e);
            respond_to.send(Err(e)).ok();
            return;
        }

        // Get cached value. Use `.get()` so a future relaxation of the
        // range check at the function entry doesn't quietly become an OOB
        // panic — the index invariant is load-bearing on `VISIBILITY_REG_END`
        // being exclusive.
        if let Some(cache) = &self.visibility_cache {
            let index = (register - VISIBILITY_REG_START) as usize;
            let Some(raw) = cache.get(index).copied() else {
                error!(
                    "ctc_actor::run: Visibility index {index} out of bounds (cache len {})",
                    cache.len()
                );
                respond_to
                    .send(Err(ModbusError::InvalidVisibilityRegister(register)))
                    .ok();
                return;
            };
            let value = f32::from(raw);
            trace!(
                "ctc_actor::run: Visibility register {} = {} (0x{:04X})",
                register, raw, raw
            );
            respond_to
                .send(Ok(ModbusResponse::Value(value)))
                .unwrap_or_else(|_| {
                    debug!("ctc_actor::run: Client disconnected before visibility response sent");
                });
        } else {
            // Should never happen since we just scanned
            error!("ctc_actor::run: Visibility cache unexpectedly empty");
            respond_to.send(Err(ModbusError::VisibilityNotScanned)).ok();
        }
    }

    /// Handle reading all visibility registers
    ///
    /// Returns all visibility registers (62500-62548 by default) as `RawRegisters`.
    /// Lazy-initializes the visibility cache on first access.
    async fn handle_all_visibility_operation(&mut self, respond_to: ResponseChannel) {
        trace!("ctc_actor::run: Operation=READ_ALL_VISIBILITY");

        // Lazy init: scan visibility on first access
        if self.visibility_cache.is_none()
            && let Err(e) = self.scan_visibility().await
        {
            error!("ctc_actor::run: Visibility scan FAILED: {}", e);
            respond_to.send(Err(e)).ok();
            return;
        }

        // Get all cached values
        if let Some(cache) = &self.visibility_cache {
            trace!(
                "ctc_actor::run: Returning all {} visibility registers",
                cache.len()
            );
            respond_to
                .send(Ok(ModbusResponse::RawRegisters {
                    start: VISIBILITY_REG_START,
                    values: cache.to_vec(),
                }))
                .unwrap_or_else(|_| {
                    debug!(
                        "ctc_actor::run: Client disconnected before all visibility response sent"
                    );
                });
        } else {
            // Should never happen since we just scanned
            error!("ctc_actor::run: Visibility cache unexpectedly empty");
            respond_to.send(Err(ModbusError::VisibilityNotScanned)).ok();
        }
    }

    /// Handle a write operation with verification (unless write-only)
    #[allow(clippy::too_many_lines)]
    async fn handle_write_operation(
        &mut self,
        param: &CTCModbusParameter,
        value: f32,
        respond_to: ResponseChannel,
    ) {
        trace!(
            "ctc_actor::run: Operation=WRITE, parameter={:?}, value={}",
            param, value
        );

        // Lazy init: scan visibility on first access. If the scan fails we
        // continue without a cache; check_visibility falls back to optimistic
        // "assume visible" so a single transient bus glitch at boot doesn't
        // poison every subsequent write.
        if self.visibility_cache.is_none()
            && let Err(e) = self.scan_visibility().await
        {
            warn!(
                "ctc_actor::run: Visibility scan FAILED, proceeding without cache: {}",
                e
            );
        }

        // Check visibility - NO record_failure() here, this is a client error not I/O failure
        match self.check_visibility(param) {
            Ok(false) => {
                // Parameter not available on this hardware - client error, not I/O failure
                trace!(
                    "ctc_actor::run: Parameter {} not visible on this hardware",
                    param.id
                );
                respond_to
                    .send(Err(ModbusError::ParameterNotVisible { register: param.id }))
                    .ok();
                return;
            }
            Err(e) => {
                // Invalid visibility register config - also not an I/O failure
                error!("ctc_actor::run: Visibility check error: {}", e);
                respond_to.send(Err(e)).ok();
                return;
            }
            Ok(true) => {} // Continue with write
        }

        match self.write_parameter(param, value).await {
            Ok(()) => {
                // Skip read-back verification for write-only registers
                if param.access == Access::W {
                    trace!("ctc_actor::run: Write-only register, skipping verification");
                    respond_to
                        .send(Ok(ModbusResponse::Value(value)))
                        .unwrap_or_else(|_| {
                            debug!(
                                "ctc_actor::run: Client disconnected before write response sent"
                            );
                        });
                    trace!("ctc_actor::run: Write operation COMPLETE (no verification)");
                    return;
                }

                trace!("ctc_actor::run: Write SUCCESS, reading back to verify");
                // Compare raw u16 values rather than scaled floats. A user value
                // like 23.55 written to a 0.1-factor register is snapped to 23.6,
                // which would fail a scaled f32::EPSILON comparison even though
                // the write succeeded.
                // get_raw_value returns Option<u16>; the prior range check
                // already rejected out-of-range values, so None here is a
                // logic bug — surface it as a verification mismatch.
                let Some(expected_raw) = param.get_raw_value(value) else {
                    respond_to
                        .send(Err(ModbusError::VerificationError {
                            expected: value,
                            actual: value,
                            register: param.id,
                        }))
                        .ok();
                    return;
                };
                match self.read_parameter_raw(param).await {
                    Ok(actual_raw) => {
                        trace!(
                            "ctc_actor::run: Read-back raw={}, comparing with written raw={}",
                            actual_raw, expected_raw
                        );

                        if actual_raw == expected_raw {
                            trace!("ctc_actor::run: Read-back MATCHES, sending success response");
                            respond_to
                                .send(Ok(ModbusResponse::Value(value)))
                                .unwrap_or_else(|_| {
                                    debug!("ctc_actor::run: Client disconnected before write success response sent");
                                });
                            trace!("ctc_actor::run: Write operation COMPLETE");
                        } else {
                            let actual_scaled = param.get_scaled_value(actual_raw);
                            error!(
                                "ctc_actor::run: Read-back MISMATCH: wrote raw={} but read raw={}",
                                expected_raw, actual_raw
                            );
                            respond_to
                                .send(Err(ModbusError::VerificationError {
                                    expected: value,
                                    actual: actual_scaled,
                                    register: param.id,
                                }))
                                .unwrap_or_else(|_| {
                                    debug!("ctc_actor::run: Client disconnected before mismatch error sent");
                                });
                            trace!("ctc_actor::run: Write operation FAILED (mismatch)");
                        }
                    }
                    Err(e) => {
                        error!("ctc_actor::run: Read-back FAILED: {}", e);
                        respond_to.send(Err(e)).unwrap_or_else(|_| {
                            debug!(
                                "ctc_actor::run: Client disconnected before read-back error sent"
                            );
                        });
                        trace!("ctc_actor::run: Write operation FAILED (read-back error)");
                    }
                }
            }
            Err(e) => {
                error!("ctc_actor::run: Write FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|_| {
                    debug!("ctc_actor::run: Client disconnected before write error sent");
                });
                trace!("ctc_actor::run: Write error response sent");
            }
        }
    }

    /// Handle a bulk raw register read operation (Modbus function 0x03)
    ///
    /// Reads `count` consecutive registers starting at `start` without applying
    /// any scaling. Returns `ModbusResponse::RawRegisters`.
    async fn handle_read_raw_registers(
        &mut self,
        start: u16,
        count: u16,
        respond_to: ResponseChannel,
    ) {
        trace!(
            "ctc_actor::run: Operation=READ_RAW_REGISTERS, start={}, count={}",
            start, count
        );

        let result = with_retry!(self, "read_raw_registers", start, WireOpKind::Read, async {
            self.context
                .read_holding_registers(start, count)
                .await
                .map_err(|e| ModbusError::ReadError {
                    register: start,
                    reason: e.to_string(),
                })
                .and_then(|r| {
                    r.map_err(|e| ModbusError::ReadError {
                        register: start,
                        reason: format!("Modbus exception: {e}"),
                    })
                })
        });

        match result {
            Ok(values) => {
                trace!(
                    "ctc_actor::run: Read raw registers SUCCESS, {} values starting at {}",
                    values.len(),
                    start
                );
                respond_to
                    .send(Ok(ModbusResponse::RawRegisters { start, values }))
                    .unwrap_or_else(|_| {
                        debug!(
                            "ctc_actor::run: Client disconnected before raw registers response sent"
                        );
                    });
            }
            Err(e) => {
                error!("ctc_actor::run: Read raw registers FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|_| {
                    debug!("ctc_actor::run: Client disconnected before raw registers error sent");
                });
            }
        }
    }

    /// Handle a single raw register write operation (Modbus function 0x06)
    ///
    /// Writes `value` to `register` without applying any scaling.
    /// Returns `ModbusResponse::Value` with the written value.
    async fn handle_write_raw_register(
        &mut self,
        register: u16,
        value: u16,
        respond_to: ResponseChannel,
    ) {
        trace!(
            "ctc_actor::run: Operation=WRITE_RAW_REGISTER, register={}, value={}",
            register, value
        );

        let result = with_retry!(
            self,
            "write_raw_register",
            register,
            WireOpKind::Write,
            async {
                self.context
                    .write_single_register(register, value)
                    .await
                    .map_err(|e| ModbusError::WriteError {
                        register,
                        value: f32::from(value),
                        reason: e.to_string(),
                    })
                    .and_then(|r| {
                        r.map_err(|e| ModbusError::WriteError {
                            register,
                            value: f32::from(value),
                            reason: format!("Modbus exception: {e}"),
                        })
                    })
            }
        );

        match result {
            Ok(()) => {
                trace!(
                    "ctc_actor::run: Write raw register SUCCESS, register={}, value={}",
                    register, value
                );
                respond_to
                    .send(Ok(ModbusResponse::Value(f32::from(value))))
                    .unwrap_or_else(|_| {
                        debug!(
                            "ctc_actor::run: Client disconnected before raw write response sent"
                        );
                    });
            }
            Err(e) => {
                error!("ctc_actor::run: Write raw register FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|_| {
                    debug!("ctc_actor::run: Client disconnected before raw write error sent");
                });
            }
        }
    }

    /// Build a fully-owned snapshot of every telemetry counter the actor
    /// holds, plus the supervisor counters it shares via `Arc`. Synchronous
    /// (no `.await`) so the dispatch loop can serve it without parking.
    fn snapshot_stats(&self) -> ModbusStats {
        use std::sync::atomic::Ordering::Relaxed;

        let now = Instant::now();
        // Saturating cast: the only realistic way as_millis() exceeds u64 is
        // a clock that's been monotonically running for ~584M years.
        let ms_to_u64 = |t: Instant| -> u64 {
            let ms = now
                .checked_duration_since(t)
                .unwrap_or(Duration::ZERO)
                .as_millis();
            u64::try_from(ms).unwrap_or(u64::MAX)
        };
        let ms_since_last_success = self.last_success.map(ms_to_u64);
        let ms_since_last_wire_op = self.last_wire_op.map(ms_to_u64);

        #[allow(clippy::cast_precision_loss)]
        let per_sec = |secs: u64| -> f64 {
            let n = self.rate_window.count_within(Duration::from_secs(secs));
            (n as f64) / (secs as f64)
        };

        let mut registers: Vec<RegisterCountsEntry> = self
            .per_register
            .iter()
            .map(|(addr, c)| RegisterCountsEntry {
                address: *addr,
                reads: c.reads,
                writes: c.writes,
                retry_attempts: c.retry_attempts,
                final_failures: c.final_failures,
            })
            .collect();
        // Sort by retry_attempts desc, then reads+writes desc. Trouble
        // registers float to the top of the response.
        registers.sort_by(|a, b| {
            b.retry_attempts
                .cmp(&a.retry_attempts)
                .then((b.reads + b.writes).cmp(&(a.reads + a.writes)))
                .then(a.address.cmp(&b.address))
        });

        let last_respawn_raw = self.sup_stats.last_respawn_epoch_secs.load(Relaxed);
        let last_respawn_epoch_secs = if last_respawn_raw == 0 {
            None
        } else {
            Some(last_respawn_raw)
        };
        // Sample monotonic uptime once so `actor_uptime_secs` and the
        // wall-clock `actor_started_at_secs` agree (otherwise two reads
        // across a second boundary could disagree by 1).
        let uptime_secs = self.actor_started_at.elapsed().as_secs();
        let supervisor = SupervisorStatsSnapshot {
            respawns: self.sup_stats.respawns.load(Relaxed),
            port_open_failures: self.sup_stats.port_open_failures.load(Relaxed),
            last_respawn_epoch_secs,
            actor_uptime_secs: uptime_secs,
        };

        // actor_started_at_secs is a wall-clock anchor; derive it from
        // the current wall clock minus the monotonic uptime sampled above.
        let actor_started_at_secs = now_epoch_secs().saturating_sub(uptime_secs);

        ModbusStats {
            actor_started_at_secs,
            client_ops: ClientOpStats {
                total: self.total_operations,
                by_kind: self.op_counts.clone(),
            },
            wire_ops: WireOpStats {
                total: self.total_wire_ops,
                timeouts: self.total_wire_timeouts,
                retry_attempts: self.total_wire_retry_attempts,
                final_failures: self.total_failures,
                consecutive_failures: self.consecutive_failures,
                max_consecutive_failures: self.max_consecutive_failures_seen,
                ms_since_last_success,
                ms_since_last_wire_op,
                per_sec_last_10s: per_sec(10),
                per_sec_last_60s: per_sec(60),
                per_sec_last_5min: per_sec(300),
            },
            supervisor,
            read_durations_ms: HistogramSnapshot::from_histo(&self.read_histo),
            write_durations_ms: HistogramSnapshot::from_histo(&self.write_histo),
            registers,
            recent_retries: self.retry_ring.iter().cloned().collect(),
        }
    }

    /// Handle a stats snapshot request. Synchronous internally — does not
    /// touch the bus.
    fn handle_get_stats(&self, respond_to: ResponseChannel) {
        let stats = self.snapshot_stats();
        respond_to
            .send(Ok(ModbusResponse::Stats(Box::new(stats))))
            .unwrap_or_else(|_| {
                debug!("ctc_actor::run: Client disconnected before stats response sent");
            });
    }

    /// Main actor loop that processes incoming parameter operations.
    /// Handles both read and write operations for Modbus parameters.
    pub async fn run(&mut self, receiver: &mut mpsc::Receiver<ModbusRequest>) {
        info!("ctc_actor::run: Actor loop starting");
        loop {
            tokio::select! {
                Some((operation, respond_to)) = receiver.recv() => {
                    // Skip if client already disconnected (e.g., browser refresh)
                    if respond_to.is_closed() {
                        debug!("ctc_actor::run: Skipping {:?} - client disconnected", operation);
                        // Count toward total_operations so request-rate metrics aren't
                        // distorted; failure counters are intentionally left alone
                        // because a disconnected client isn't a Modbus failure.
                        self.total_operations += 1;
                        continue;
                    }
                    // Per-variant client-op accounting. Counted after the
                    // disconnected-client early-return so a hung-up caller
                    // doesn't skew the per-variant counts.
                    self.op_counts.bump(&operation);
                    match operation {
                        ParameterOperation::Read(param) => {
                            self.handle_read_operation(&param, respond_to).await;
                        },
                        ParameterOperation::Write(param, value) => {
                            self.handle_write_operation(&param, value, respond_to).await;
                        },
                        ParameterOperation::ReadVisibility(register) => {
                            self.handle_visibility_operation(register, respond_to).await;
                        },
                        ParameterOperation::ReadAllVisibility => {
                            self.handle_all_visibility_operation(respond_to).await;
                        },
                        ParameterOperation::ReadRawRegisters { start, count } => {
                            self.handle_read_raw_registers(start, count, respond_to).await;
                        },
                        ParameterOperation::WriteRawRegister { register, value } => {
                            self.handle_write_raw_register(register, value, respond_to).await;
                        },
                        ParameterOperation::GetStats => {
                            self.handle_get_stats(respond_to);
                        },
                    }
                }
                else => {
                    error!("ctc_actor::run: Channel closed or error, actor loop TERMINATING");
                    break;
                }
            }
        }
        error!(
            "ctc_actor::run: Actor loop has EXITED - this should not happen in normal operation!"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::panic::AssertUnwindSafe;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Verifies the supervisor's panic-recovery semantics: a panic inside the
    /// actor's `run` is caught, the actor is rebuilt, and a subsequent request
    /// is processed.
    ///
    /// `CtcActorBuilder::spawn_supervised` cannot be invoked from a unit test
    /// because `build()` opens a real serial port via `tokio_serial`. The body
    /// of this test mirrors the supervisor loop exactly (same primitives:
    /// `AssertUnwindSafe + catch_unwind`, then sleep, then rebuild), with a
    /// stub actor that panics on its first build and processes a request on
    /// its second. If the supervisor pattern regresses, this test fails.
    #[tokio::test(flavor = "current_thread")]
    async fn supervisor_respawns_actor_after_panic_and_processes_next_request() {
        let (tx, mut rx) = mpsc::channel::<ModbusRequest>(8);
        let builds = Arc::new(AtomicU32::new(0));
        let processed = Arc::new(AtomicU32::new(0));

        let builds_super = Arc::clone(&builds);
        let processed_super = Arc::clone(&processed);

        // Mirror of `spawn_supervised`'s loop body.
        let supervisor = tokio::spawn(async move {
            const RESPAWN_DELAY: Duration = Duration::from_millis(10);
            loop {
                let attempt = builds_super.fetch_add(1, Ordering::SeqCst);
                let processed_inner = Arc::clone(&processed_super);
                let fut = async {
                    // Wait for a request, then either panic (first iteration)
                    // or respond (subsequent iterations).
                    if let Some((_op, respond_to)) = rx.recv().await {
                        assert!(attempt != 0, "induced panic inside run");
                        processed_inner.fetch_add(1, Ordering::SeqCst);
                        let _ = respond_to.send(Ok(ModbusResponse::Value(7.0)));
                    }
                };
                let result = AssertUnwindSafe(fut).catch_unwind().await;
                if attempt >= 1 && result.is_ok() {
                    // Test driver got its answer; stop the supervisor.
                    break;
                }
                sleep(RESPAWN_DELAY).await;
            }
        });

        // First request: triggers the panic. The oneshot is dropped when the
        // panicking future unwinds, so the receiver side resolves with Err.
        let (r1_tx, r1_rx) = oneshot::channel();
        tx.send((ParameterOperation::ReadVisibility(62500), r1_tx))
            .await
            .unwrap();
        let r1 = r1_rx.await;
        assert!(
            r1.is_err(),
            "panicking actor must drop the responder, surfacing as oneshot error"
        );

        // Second request: should be processed by the respawned actor.
        let (r2_tx, r2_rx) = oneshot::channel();
        tx.send((ParameterOperation::ReadVisibility(62500), r2_tx))
            .await
            .unwrap();
        let r2 = r2_rx.await.expect("response from respawned actor");
        match r2 {
            Ok(ModbusResponse::Value(v)) => assert!((v - 7.0).abs() < f32::EPSILON),
            other => panic!("unexpected response: {other:?}"),
        }

        supervisor.await.unwrap();

        assert_eq!(
            builds.load(Ordering::SeqCst),
            2,
            "supervisor must rebuild the actor once after the panic"
        );
        assert_eq!(
            processed.load(Ordering::SeqCst),
            1,
            "respawned actor must process the queued request"
        );
    }

    fn dummy_param(visible: u16, bit: u8) -> CTCModbusParameter {
        CTCModbusParameter {
            id: 12345,
            signed: false,
            access: Access::R,
            reg_max: None,
            reg_min: None,
            reg_step: None,
            visible,
            bit,
            factor: 1.0,
            description: "test parameter",
        }
    }

    /// When the visibility scan failed (cache is `None`), a parameter with a
    /// non-zero `visible` register must still pass the visibility check via
    /// the optimistic fallback. Otherwise every read after a transient boot
    /// glitch would be rejected with `ParameterNotVisible`.
    #[test]
    fn check_visibility_falls_back_to_optimistic_when_cache_missing() {
        let param = dummy_param(VISIBILITY_REG_START + 5, 3);
        let visible = check_visibility_against(None, &param);
        assert!(
            matches!(visible, Ok(true)),
            "expected optimistic Ok(true), got {visible:?}"
        );
    }

    /// `visible == 0` registers (e.g. `CTC_ALARM_INFO_BUFFER`) must always be
    /// reported visible regardless of cache state — the optimistic fallback
    /// is a stricter case of this rule.
    #[test]
    fn check_visibility_zero_visible_register_always_visible() {
        let param = dummy_param(0, 0);
        assert!(matches!(check_visibility_against(None, &param), Ok(true)));

        let cache = [0u16; VISIBILITY_REG_COUNT];
        assert!(matches!(
            check_visibility_against(Some(&cache), &param),
            Ok(true)
        ));
    }

    /// When the cache IS populated, the bitmask is consulted: a clear bit
    /// must report not visible. Establishes that the fallback isn't masking
    /// real visibility checks when the scan succeeded.
    #[test]
    fn check_visibility_respects_cache_when_populated() {
        // Param looks at register VISIBILITY_REG_START (cache index 0), bit 3.
        let param = dummy_param(VISIBILITY_REG_START, 3);
        let mut cache = [0u16; VISIBILITY_REG_COUNT];

        // Bit 3 clear → not visible.
        assert!(matches!(
            check_visibility_against(Some(&cache), &param),
            Ok(false)
        ));

        // Bit 3 set → visible.
        cache[0] = 1 << 3;
        assert!(matches!(
            check_visibility_against(Some(&cache), &param),
            Ok(true)
        ));
    }

    /// A parameter whose `visible` register address is outside the known
    /// range must yield `InvalidVisibilityRegister`, but only when the cache
    /// has been populated — the optimistic-no-cache path short-circuits
    /// earlier.
    #[test]
    fn check_visibility_out_of_range_only_errors_when_cache_present() {
        let param = dummy_param(VISIBILITY_REG_END + 1, 0);
        // No cache → optimistic Ok(true) wins before bounds check.
        assert!(matches!(check_visibility_against(None, &param), Ok(true)));

        let cache = [0u16; VISIBILITY_REG_COUNT];
        let err = check_visibility_against(Some(&cache), &param).unwrap_err();
        assert!(matches!(err, ModbusError::InvalidVisibilityRegister(_)));
    }

    // ===== Telemetry primitive tests =====

    /// HDR histogram sanity: known input distribution produces percentiles
    /// within the documented ±tolerance at 3 sig figs.
    #[test]
    fn hdr_histogram_percentile_sanity() {
        let mut h = Histogram::<u64>::new_with_bounds(1, 60_000, 3).unwrap();
        for _ in 0..1000 {
            h.record(100).unwrap();
        }
        for _ in 0..10 {
            h.record(500).unwrap();
        }
        // p50 ≈ 100, p99 within the long-tail bucket, max == 500 exactly.
        assert!(
            h.value_at_percentile(50.0).abs_diff(100) <= 2,
            "p50 expected ~100, got {}",
            h.value_at_percentile(50.0)
        );
        // 1000 samples at 100 + 10 samples at 500 = 1010 total. p99 is the
        // 1000th sample, still in the 100 group; p99.9 (the ~1009th sample)
        // lands in the tail. Assert p99.9 sits in the tail bucket — this
        // verifies HDR is actually tracking outliers rather than collapsing
        // them into p50. At 3 sig figs over 1..60_000 the 500-bucket
        // resolves nearly exactly.
        let p99 = h.value_at_percentile(99.0);
        let p99_9 = h.value_at_percentile(99.9);
        assert_eq!(p99, 100, "p99 of 1000@100+10@500 is still at 100");
        assert!(
            (400..=600).contains(&p99_9),
            "p99.9 should land in the tail bucket near 500, got {p99_9}"
        );
        assert_eq!(h.max(), 500, "max should be the largest recorded sample");
        assert_eq!(h.len(), 1010, "count should match total records");
    }

    type OpCountGetter = fn(&OpCounts) -> u64;

    /// `OpCounts::bump` exhaustively touches the right field for every
    /// `ParameterOperation` variant, and only that field.
    #[test]
    fn op_counts_bump_per_variant() {
        let dummy = CTCModbusParameter {
            id: 1,
            signed: false,
            access: Access::R,
            reg_max: None,
            reg_min: None,
            reg_step: None,
            visible: 0,
            bit: 0,
            factor: 1.0,
            description: "",
        };

        let cases: Vec<(ParameterOperation, OpCountGetter)> = vec![
            (ParameterOperation::Read(dummy), |c| c.read),
            (ParameterOperation::Write(dummy, 1.0), |c| c.write),
            (
                ParameterOperation::ReadVisibility(VISIBILITY_REG_START),
                |c| c.read_visibility,
            ),
            (ParameterOperation::ReadAllVisibility, |c| {
                c.read_all_visibility
            }),
            (
                ParameterOperation::ReadRawRegisters { start: 0, count: 1 },
                |c| c.read_raw_registers,
            ),
            (
                ParameterOperation::WriteRawRegister {
                    register: 0,
                    value: 0,
                },
                |c| c.write_raw_register,
            ),
            (ParameterOperation::GetStats, |c| c.get_stats),
        ];

        for (op, getter) in cases {
            let mut counts = OpCounts::default();
            counts.bump(&op);
            assert_eq!(getter(&counts), 1, "wrong field bumped for {op:?}");
            // Every other field stays 0.
            let total = counts.read
                + counts.write
                + counts.read_visibility
                + counts.read_all_visibility
                + counts.read_raw_registers
                + counts.write_raw_register
                + counts.get_stats;
            assert_eq!(total, 1, "bumped more than one field for {op:?}");
        }
    }

    /// `RetryRing::push` is FIFO and caps at 100.
    #[test]
    fn retry_ring_fifo_and_caps_at_100() {
        let mut ring = RetryRing::default();
        for i in 0..100u16 {
            ring.push(make_retry_event(i));
        }
        assert_eq!(ring.len(), 100);
        // Push one more; oldest should drop and newest appear at the back.
        ring.push(make_retry_event(100));
        assert_eq!(ring.len(), 100);
        let registers: Vec<u16> = ring.iter().map(|e| e.register).collect();
        assert_eq!(
            *registers.first().unwrap(),
            1,
            "oldest entry should be evicted"
        );
        assert_eq!(
            *registers.last().unwrap(),
            100,
            "newest entry should be at back"
        );
    }

    fn make_retry_event(register: u16) -> RetryEvent {
        RetryEvent {
            when_epoch_secs: 0,
            register,
            op_name: "test",
            attempts: Vec::new(),
            final_outcome: FinalOutcome::Succeeded { attempts_used: 1 },
            total_struggle_ms: 0,
        }
    }

    /// `per_register` cap-256 guard: 256 distinct keys fit, the 257th is
    /// dropped (with a warn), but bumps to an already-present key still
    /// work past the cap.
    #[test]
    fn per_register_cap_at_256() {
        let mut map: HashMap<u16, RegisterCounters> = HashMap::new();
        // Use the same insert-or-bump path the macro uses.
        for i in 0..256u16 {
            let len_before = map.len();
            let already = map.contains_key(&i);
            if already || len_before < PER_REGISTER_CAP {
                map.entry(i).or_default().reads += 1;
            }
        }
        assert_eq!(map.len(), 256);

        // 257th distinct address — dropped.
        let i = 256u16;
        let len_before = map.len();
        let already = map.contains_key(&i);
        if already || len_before < PER_REGISTER_CAP {
            map.entry(i).or_default().reads += 1;
        }
        assert_eq!(map.len(), 256, "new address must be dropped at cap");
        assert!(!map.contains_key(&i));

        // Bumping an already-present key still works.
        let already = map.contains_key(&0);
        let len_before = map.len();
        if already || len_before < PER_REGISTER_CAP {
            map.entry(0u16).or_default().reads += 1;
        }
        assert_eq!(map.get(&0).unwrap().reads, 2);
    }

    /// `RateWindow` counts entries within the requested window and evicts
    /// entries older than the 5-min ceiling on push.
    #[test]
    fn rate_window_count_and_eviction() {
        let mut win = RateWindow::default();
        let now = Instant::now();
        // Manually inject timestamps spread across the recent past.
        // 10 entries within the last 5 s, 30 entries within the last 60 s
        // (cumulative), 100 entries within the last 5 min (cumulative).
        for i in 0_u64..10 {
            win.inner.push_back(now - Duration::from_millis(500 * i));
        }
        for i in 0_u64..20 {
            win.inner.push_back(now - Duration::from_secs(10 + i));
        }
        for i in 0_u64..70 {
            win.inner.push_back(now - Duration::from_secs(60 + 3 * i));
        }

        // Counts grow with window size, monotone.
        let c10 = win.count_within(Duration::from_secs(10));
        let c60 = win.count_within(Duration::from_secs(60));
        let c300 = win.count_within(Duration::from_secs(300));
        assert!(c10 <= c60, "count_within must be monotone (10s <= 60s)");
        assert!(c60 <= c300, "count_within must be monotone (60s <= 300s)");
        assert!(c10 >= 10, "10s window should see the ten ≤5s entries");
        // 100 entries injected, all within the past 300 s — the full window
        // should see all of them.
        assert_eq!(c300, 100, "5min window should see every injected entry");

        // Eviction: push a timestamp 6 min ahead and confirm all old
        // entries fall off. (Using "now + 6min" via push to drive eviction
        // against the ceiling.)
        let future = now + Duration::from_secs(360);
        win.push(future);
        // After eviction, only entries from `future` minus 5 min onward
        // remain. The injected entries (clustered around `now`) are all
        // older than 5 min relative to `future`.
        assert_eq!(
            win.len(),
            1,
            "all stale entries should have been evicted by push"
        );
    }

    /// Sanity: when nothing fails, the macro emits no retry event. We test
    /// this by exercising the macro against a stub actor whose op always
    /// returns Ok on the first attempt.
    #[tokio::test(flavor = "current_thread")]
    async fn with_retry_no_event_on_first_shot_success() {
        let mut a = StubActor::new(2);
        // `$operation` evaluates to a future yielding Result<T, ModbusError>.
        let r: Result<u16, ModbusError> =
            with_retry!(a, "stub_read", 42u16, WireOpKind::Read, async {
                Ok::<u16, ModbusError>(7)
            });
        assert!(matches!(r, Ok(7)));
        assert!(
            a.retry_ring.is_empty(),
            "no retry event on first-shot success"
        );
        assert_eq!(a.total_wire_ops, 1);
        assert_eq!(a.total_wire_timeouts, 0);
        assert_eq!(a.total_wire_retry_attempts, 0);
        assert_eq!(a.read_histo.len(), 1);
    }

    /// Retry-then-success emits exactly one event with `attempts_used=2`
    /// and outcomes Exception-then-Success.
    #[tokio::test(flavor = "current_thread")]
    async fn with_retry_event_succeeded_after_one_retry() {
        let mut a = StubActor::new(2);
        // Cell so the closure can mutate per-attempt state.
        let call = std::cell::Cell::new(0u32);
        let r: Result<u16, ModbusError> =
            with_retry!(a, "stub_read", 42u16, WireOpKind::Read, async {
                call.set(call.get() + 1);
                if call.get() == 1 {
                    Err::<u16, ModbusError>(ModbusError::ReadError {
                        register: 42,
                        reason: "first attempt boom".to_string(),
                    })
                } else {
                    Ok::<u16, ModbusError>(7)
                }
            });
        assert!(matches!(r, Ok(7)));
        assert_eq!(a.retry_ring.len(), 1, "exactly one retry event expected");
        let evt = a.retry_ring.iter().next().unwrap();
        assert_eq!(evt.attempts.len(), 2, "two attempts captured");
        match &evt.final_outcome {
            FinalOutcome::Succeeded { attempts_used } => assert_eq!(*attempts_used, 2),
            FinalOutcome::FinalFailure { reason } => {
                panic!("expected Succeeded, got FinalFailure({reason})")
            }
        }
        // First attempt's outcome is Exception (a transient ReadError); the
        // second is Success.
        assert!(matches!(
            evt.attempts[0].outcome,
            AttemptOutcome::Exception(_)
        ));
        assert!(matches!(evt.attempts[1].outcome, AttemptOutcome::Success));
        assert_eq!(a.total_wire_ops, 2);
        assert_eq!(a.total_wire_retry_attempts, 1);
    }

    /// Retry exhaustion emits exactly one event with `FinalFailure` and
    /// `attempts.len() == max_retries + 1`.
    #[tokio::test(flavor = "current_thread")]
    async fn with_retry_event_final_failure_after_exhaustion() {
        let mut a = StubActor::new(2); // max_retries=2 → up to 3 attempts
        let r: Result<u16, ModbusError> =
            with_retry!(a, "stub_read", 42u16, WireOpKind::Read, async {
                Err::<u16, ModbusError>(ModbusError::ReadError {
                    register: 42,
                    reason: "persistent".to_string(),
                })
            });
        assert!(r.is_err());
        assert_eq!(a.retry_ring.len(), 1);
        let evt = a.retry_ring.iter().next().unwrap();
        assert_eq!(evt.attempts.len(), 3, "max_retries=2 means 3 attempts");
        assert!(matches!(
            evt.final_outcome,
            FinalOutcome::FinalFailure { .. }
        ));
        let counts = a.per_register.get(&42u16).copied().unwrap_or_default();
        assert_eq!(counts.final_failures, 1);
        assert_eq!(
            counts.retry_attempts, 3,
            "every failed attempt bumps retry_attempts"
        );
    }

    /// Exercises `SupervisorStats::record_build_failure` and
    /// `record_respawn` — the same helpers `spawn_supervised` calls in
    /// production. `spawn_supervised` itself can't run in unit tests
    /// (its `build()` opens a real serial port), but as long as
    /// production funnels through these named helpers, the helpers'
    /// contract is the contract.
    #[test]
    fn supervisor_stats_helpers_record_build_failures_and_respawns() {
        use std::sync::atomic::Ordering::Relaxed;
        let stats = SupervisorStats::default();

        stats.record_build_failure();
        stats.record_build_failure();
        stats.record_respawn();

        assert_eq!(
            stats.port_open_failures.load(Relaxed),
            2,
            "record_build_failure must bump port_open_failures by 1 per call"
        );
        assert_eq!(
            stats.respawns.load(Relaxed),
            1,
            "record_respawn must bump respawns by 1"
        );
        assert!(
            stats.last_respawn_epoch_secs.load(Relaxed) > 0,
            "record_respawn must stamp last_respawn_epoch_secs"
        );
        // Build failures alone must not move respawn fields.
        let only_failures = SupervisorStats::default();
        only_failures.record_build_failure();
        assert_eq!(only_failures.respawns.load(Relaxed), 0);
        assert_eq!(only_failures.last_respawn_epoch_secs.load(Relaxed), 0);
    }

    // ----- StubActor: duck-types the fields/methods `with_retry!` touches -----

    /// Test-only stand-in for `CtcActor`. Carries every field the macro
    /// reads, plus `record_success` / `record_failure` / `calculate_retry_delay`
    /// with the same signatures. The macro is generic over `$self` so it
    /// will happily expand against this type.
    struct StubActor {
        operation_timeout: Duration,
        max_retries: u32,
        initial_retry_delay: Duration,
        backoff_multiplier: f64,
        inter_request_gap: Duration,
        last_wire_op: Option<Instant>,
        consecutive_failures: u32,
        max_consecutive_failures_seen: u32,
        max_consecutive_failures: u32,
        last_success: Option<Instant>,
        total_operations: u64,
        total_failures: u64,
        read_histo: Histogram<u64>,
        write_histo: Histogram<u64>,
        per_register: HashMap<u16, RegisterCounters>,
        retry_ring: RetryRing,
        rate_window: RateWindow,
        total_wire_ops: u64,
        total_wire_timeouts: u64,
        total_wire_retry_attempts: u64,
    }

    impl StubActor {
        fn new(max_retries: u32) -> Self {
            Self {
                operation_timeout: Duration::from_secs(5),
                max_retries,
                initial_retry_delay: Duration::from_millis(1),
                backoff_multiplier: 1.0,
                inter_request_gap: Duration::ZERO,
                last_wire_op: None,
                consecutive_failures: 0,
                max_consecutive_failures_seen: 0,
                max_consecutive_failures: u32::MAX,
                last_success: None,
                total_operations: 0,
                total_failures: 0,
                read_histo: Histogram::<u64>::new_with_bounds(1, 60_000, 3).unwrap(),
                write_histo: Histogram::<u64>::new_with_bounds(1, 60_000, 3).unwrap(),
                per_register: HashMap::new(),
                retry_ring: RetryRing::default(),
                rate_window: RateWindow::default(),
                total_wire_ops: 0,
                total_wire_timeouts: 0,
                total_wire_retry_attempts: 0,
            }
        }

        fn calculate_retry_delay(&self, attempt: u32) -> Duration {
            if attempt == 0 {
                Duration::ZERO
            } else {
                self.initial_retry_delay
            }
        }

        fn record_success(&mut self) {
            self.consecutive_failures = 0;
            self.last_success = Some(Instant::now());
            self.total_operations += 1;
        }

        fn record_failure(&mut self) {
            self.consecutive_failures += 1;
            self.total_failures += 1;
            if self.consecutive_failures > self.max_consecutive_failures_seen {
                self.max_consecutive_failures_seen = self.consecutive_failures;
            }
        }
    }
}
