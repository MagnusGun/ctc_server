//! Modbus actor for CTC heating system
//!
//! This module provides an actor-based interface to the Modbus RTU protocol
//! for communicating with CTC heating systems. The actor ensures exclusive
//! access to the serial port and processes operations sequentially.

use crate::error::ModbusError;
use crate::modbus::{Access, CTCModbusParameter};
use std::io;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout};
use tokio_modbus::client::Writer;
use tokio_modbus::prelude::{Reader, Slave, rtu};
use tokio_serial::SerialPortBuilderExt;
use tokio_serial::{DataBits, FlowControl, Parity, StopBits};
use tracing::{error, info, trace, warn};

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

pub enum ParameterOperation {
    Read(CTCModbusParameter),
    // ReadVector(&Vec<'static CTCModbusParameter>),
    Write(CTCModbusParameter, f32),
    /// Read a specific visibility register (62500-62548)
    /// Returns the raw bitmask value as f32
    ReadVisibility(u16),
    /// Read all 49 visibility registers (62500-62548)
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
    pub receiver: mpsc::Receiver<ModbusRequest>,
    context: tokio_modbus::client::Context,
    // Timeout and retry configuration
    operation_timeout: Duration,
    max_retries: u32,
    initial_retry_delay: Duration,
    backoff_multiplier: f64,
    max_consecutive_failures: u32,
    // Tracking fields
    consecutive_failures: u32,
    last_success: Option<Instant>,
    total_operations: u64,
    total_failures: u64,
    // Visibility cache: registers 62500-62548, lazy-loaded on first access
    visibility_cache: Option<[u16; VISIBILITY_REG_COUNT]>,
}

#[allow(dead_code)]
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

    pub fn build(self, receiver: mpsc::Receiver<ModbusRequest>) -> io::Result<CtcActor> {
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
            receiver,
            context: ctx,
            operation_timeout: self.operation_timeout,
            max_retries: self.max_retries,
            initial_retry_delay: self.initial_retry_delay,
            backoff_multiplier: self.backoff_multiplier,
            max_consecutive_failures: self.max_consecutive_failures,
            consecutive_failures: 0,
            last_success: None,
            total_operations: 0,
            total_failures: 0,
            visibility_cache: None,
        })
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

                // IMPORTANT: $operation is evaluated here, inside the loop,
                // creating a fresh future each iteration
                let future = $operation;

                match timeout($self.operation_timeout, future).await {
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
                        warn!(
                            "{} failed on attempt {}/{}: {} (register {})",
                            $op_name,
                            attempt + 1,
                            $self.max_retries + 1,
                            e,
                            $register
                        );
                        last_error = Some(e);
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
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            #[allow(clippy::cast_precision_loss)]
            #[allow(clippy::cast_possible_wrap)]
            let delay_ms = (self.initial_retry_delay.as_millis() as f64
                * self.backoff_multiplier.powi(attempt as i32 - 1))
                as u64;
            Duration::from_millis(delay_ms)
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
    /// Reads registers 62500-62548 (49 registers) containing visibility bitmasks
    /// for hardware capability detection. The cache is populated lazily on first
    /// parameter access.
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
    fn check_visibility(&self, param: &CTCModbusParameter) -> Result<bool, ModbusError> {
        // FIRST: visible == 0 means always visible - check BEFORE touching cache
        // This ensures CTC_ALARM_INFO_BUFFER etc. work even if scan failed
        if param.visible == 0 {
            return Ok(true);
        }

        // NOW we need the cache - fail if not scanned
        let cache = self
            .visibility_cache
            .as_ref()
            .ok_or(ModbusError::VisibilityNotScanned)?;

        // Bounds check for future-proofing: reject registers outside known range
        if param.visible < VISIBILITY_REG_START || param.visible > VISIBILITY_REG_END {
            return Err(ModbusError::InvalidVisibilityRegister(param.visible));
        }

        // Calculate index: register 62500 = index 0
        let index = (param.visible - VISIBILITY_REG_START) as usize;
        Ok(param.is_visible(cache[index]))
    }

    async fn read_parameter(&mut self, param: &CTCModbusParameter) -> Result<f32, ModbusError> {
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
                                "ctc_actor::read_parameter: Raw values for parameter {:?}: {:?}",
                                param, raw_values
                            );
                            let scaled_values = param.get_scaled_value_vector(&raw_values);
                            scaled_values
                                .first()
                                .copied()
                                .ok_or_else(|| ModbusError::ReadError {
                                    register: param.id,
                                    reason: "No value returned".to_string(),
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
                            if raw_values.len() < 2 {
                                return Err(ModbusError::ValidationReadError {
                                    register: param_id,
                                    reason: "Not enough values returned".to_string(),
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
        // Must be 0-9999 (alarm) or 10000-19999 (info)
        if param.id == 65100 {
            // Check for negative values or values outside valid range
            if !(0.0..=19999.0).contains(&value) {
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

        let raw_value = param.get_raw_value(value);
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

        // Lazy init: scan visibility on first access
        // Note: scan_visibility() calls with_retry!, which handles record_success/record_failure
        if self.visibility_cache.is_none()
            && let Err(e) = self.scan_visibility().await
        {
            // scan_visibility already called record_failure() via with_retry!
            error!("ctc_actor::run: Visibility scan FAILED: {}", e);
            respond_to.send(Err(e)).ok();
            return;
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
                    .unwrap_or_else(|e| {
                        error!(
                        "ctc_actor::run: CRITICAL - Failed to send read response on oneshot channel: {e:?}"
                    );
                    });
                trace!("ctc_actor::run: Read response sent");
            }
            Err(e) => {
                error!("ctc_actor::run: Read FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send read error response: {e:?}");
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

        // Get cached value
        if let Some(cache) = &self.visibility_cache {
            let index = (register - VISIBILITY_REG_START) as usize;
            let value = f32::from(cache[index]);
            trace!(
                "ctc_actor::run: Visibility register {} = {} (0x{:04X})",
                register, cache[index], cache[index]
            );
            respond_to
                .send(Ok(ModbusResponse::Value(value)))
                .unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send visibility response: {e:?}");
                });
        } else {
            // Should never happen since we just scanned
            error!("ctc_actor::run: Visibility cache unexpectedly empty");
            respond_to.send(Err(ModbusError::VisibilityNotScanned)).ok();
        }
    }

    /// Handle reading all visibility registers
    ///
    /// Returns all 49 visibility registers (62500-62548) as `RawRegisters`.
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
                .unwrap_or_else(|e| {
                    error!(
                        "ctc_actor::run: CRITICAL - Failed to send all visibility response: {e:?}"
                    );
                });
        } else {
            // Should never happen since we just scanned
            error!("ctc_actor::run: Visibility cache unexpectedly empty");
            respond_to.send(Err(ModbusError::VisibilityNotScanned)).ok();
        }
    }

    /// Handle a write operation with verification (unless write-only)
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

        // Lazy init: scan visibility on first access
        // Note: scan_visibility() calls with_retry!, which handles record_success/record_failure
        if self.visibility_cache.is_none()
            && let Err(e) = self.scan_visibility().await
        {
            // scan_visibility already called record_failure() via with_retry!
            error!("ctc_actor::run: Visibility scan FAILED: {}", e);
            respond_to.send(Err(e)).ok();
            return;
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
                        .unwrap_or_else(|e| {
                            error!(
                                "ctc_actor::run: CRITICAL - Failed to send success response: {e:?}"
                            );
                        });
                    trace!("ctc_actor::run: Write operation COMPLETE (no verification)");
                    return;
                }

                trace!("ctc_actor::run: Write SUCCESS, reading back to verify");
                match self.read_parameter(param).await {
                    Ok(return_value) => {
                        trace!(
                            "ctc_actor::run: Read-back value={}, comparing with written value={}",
                            return_value, value
                        );

                        if (return_value - value).abs() < f32::EPSILON {
                            trace!("ctc_actor::run: Read-back MATCHES, sending success response");
                            respond_to
                                .send(Ok(ModbusResponse::Value(value)))
                                .unwrap_or_else(|e| {
                                    error!(
                                    "ctc_actor::run: CRITICAL - Failed to send success response: {e:?}"
                                );
                                });
                            trace!("ctc_actor::run: Write operation COMPLETE");
                        } else {
                            error!(
                                "ctc_actor::run: Read-back MISMATCH: wrote {} but read {}",
                                value, return_value
                            );
                            respond_to
                                .send(Err(ModbusError::VerificationError {
                                    expected: value,
                                    actual: return_value,
                                    register: param.id,
                                }))
                                .unwrap_or_else(|e| {
                                    error!(
                                        "ctc_actor::run: CRITICAL - Failed to send mismatch error: {e:?}"
                                    );
                                });
                            trace!("ctc_actor::run: Write operation FAILED (mismatch)");
                        }
                    }
                    Err(e) => {
                        error!("ctc_actor::run: Read-back FAILED: {}", e);
                        respond_to.send(Err(e)).unwrap_or_else(|e| {
                            error!(
                                "ctc_actor::run: CRITICAL - Failed to send read-back error: {e:?}"
                            );
                        });
                        trace!("ctc_actor::run: Write operation FAILED (read-back error)");
                    }
                }
            }
            Err(e) => {
                error!("ctc_actor::run: Write FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send write error response: {e:?}");
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
                    .unwrap_or_else(|e| {
                        error!(
                            "ctc_actor::run: CRITICAL - Failed to send raw registers response: {e:?}"
                        );
                    });
            }
            Err(e) => {
                error!("ctc_actor::run: Read raw registers FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send raw registers error: {e:?}");
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
                    .unwrap_or_else(|e| {
                        error!(
                            "ctc_actor::run: CRITICAL - Failed to send raw write response: {e:?}"
                        );
                    });
            }
            Err(e) => {
                error!("ctc_actor::run: Write raw register FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send raw write error: {e:?}");
                });
            }
        }
    }

    /// Main actor loop that processes incoming parameter operations.
    /// Handles both read and write operations for Modbus parameters.
    pub async fn run(&mut self) {
        info!("ctc_actor::run: Actor loop starting");
        loop {
            tokio::select! {
                Some((operation, respond_to)) = self.receiver.recv() => {
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
