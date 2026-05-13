//! Modbus actor for CTC heating system
//!
//! This module provides an actor-based interface to the Modbus RTU protocol
//! for communicating with CTC heating systems. The actor ensures exclusive
//! access to the serial port and processes operations sequentially.

use crate::error::ModbusError;
use crate::modbus::bms_parameters::{ALARM_REF_MIN, INFO_REF_MAX};
use crate::modbus::{Access, CTCModbusParameter};
use std::io;
use std::time::Duration;
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
    last_success: Option<Instant>,
    total_operations: u64,
    total_failures: u64,
    // Visibility cache: registers 62500-62548, lazy-loaded on first access
    visibility_cache: Option<[u16; VISIBILITY_REG_COUNT]>,
}

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
        }
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
            last_success: None,
            total_operations: 0,
            total_failures: 0,
            visibility_cache: None,
        })
    }

    /// Spawn the actor under a supervisor task that respawns it on unexpected exit.
    ///
    /// The supervisor owns the request `receiver` across respawns, so the
    /// `mpsc::Sender` held by the rest of the application remains valid even
    /// when the underlying actor task exits or panics. On exit (clean or via
    /// panic) the supervisor sleeps briefly, rebuilds the actor (which reopens
    /// the serial port), and resumes processing requests.
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
/// - Returns `Result<T, ModbusError>`
///
/// # Important
/// - The `$operation` expression is evaluated INSIDE the loop, creating a fresh future
///   each iteration. Never poll the same future twice.
/// - On final failure, returns the stored `ModbusError` (not stringified), allowing
///   structured logging before conversion to `ApiError`.
macro_rules! with_retry {
    ($self:expr, $op_name:expr, $register:expr, $operation:expr) => {{
        let mut last_error: Option<ModbusError> = None;

        let result: Result<_, ModbusError> = 'retry: {
            for attempt in 0..=$self.max_retries {
                if attempt > 0 {
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

                // Enforce the inter-request gap: a CTC firmware needs settle
                // time between back-to-back wire transactions. The first
                // operation pays nothing; subsequent ones only pay the
                // remainder of the gap.
                CtcActor::wait_for_inter_request_gap(
                    $self.inter_request_gap,
                    $self.last_wire_op,
                )
                .await;

                // IMPORTANT: $operation is evaluated here, inside the loop,
                // creating a fresh future each iteration
                let future = $operation;

                let result = timeout($self.operation_timeout, future).await;
                $self.last_wire_op = Some(Instant::now());
                match result {
                    Ok(Ok(value)) => {
                        $self.record_success();
                        trace!(
                            "{} succeeded on attempt {} (register {})",
                            $op_name,
                            attempt + 1,
                            $register
                        );
                        break 'retry Ok(value);
                    }
                    Ok(Err(e)) => {
                        let transient = e.is_transient();
                        warn!(
                            "{} failed on attempt {}/{}: {} (register {})",
                            $op_name,
                            attempt + 1,
                            $self.max_retries + 1,
                            e,
                            $register
                        );
                        last_error = Some(e);
                        if !transient {
                            // Permanent error: skip remaining retries.
                            break;
                        }
                    }
                    Err(_elapsed) => {
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
                        last_error = Some(timeout_err);
                    }
                }
            }

            // All retries exhausted - call record_failure() exactly once
            $self.record_failure();

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

            // Return the structured error (not stringified) for logging
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
    /// matters because the actor is `!Sync` (the tokio_modbus context isn't).
    async fn wait_for_inter_request_gap(gap: Duration, last: Option<Instant>) {
        if gap.is_zero() {
            return;
        }
        if let Some(last) = last {
            let elapsed = last.elapsed();
            if elapsed < gap {
                sleep(gap - elapsed).await;
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
        let values: Vec<u16> = with_retry!(self, "scan_visibility", VISIBILITY_REG_START, async {
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
        })?;

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
        with_retry!(self, "read_holding_registers", param.id, async {
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
        })
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

        with_retry!(self, "read_validation_parameters", reg_max, async {
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
                                    reason: format!("Expected 3 values, got {}", raw_values.len()),
                                });
                            }
                            trace!(
                                "ctc_actor::read_min_max: Raw values max/min {:?}",
                                raw_values
                            );
                            Ok((raw_values[0], raw_values[1], raw_values[2]))
                        })
                })
        })
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

        with_retry!(self, "write_single_register", param.id, async {
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
        })
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

        let result = with_retry!(self, "read_raw_registers", start, async {
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

        let result = with_retry!(self, "write_raw_register", register, async {
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
        });

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
}
