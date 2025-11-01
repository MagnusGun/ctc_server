//! Modbus actor for CTC heating system
//!
//! This module provides an actor-based interface to the Modbus RTU protocol
//! for communicating with CTC heating systems. The actor ensures exclusive
//! access to the serial port and processes operations sequentially.

use crate::error::ModbusError;
use crate::modbus::{CTCModbusParameter, SmartGridMode};
use std::io;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout};
use tokio_modbus::client::Writer;
use tokio_modbus::prelude::{Reader, Slave, rtu};
use tokio_serial::SerialPortBuilderExt;
use tokio_serial::{DataBits, FlowControl, Parity, StopBits};
use tracing::{debug, error, info, warn};

pub type ModbusResponse = Result<f32, ModbusError>;
pub type ResponseChannel = oneshot::Sender<ModbusResponse>;
pub type ModbusRequest = (ParameterOperation, ResponseChannel);
pub type ModbusSender = mpsc::Sender<ModbusRequest>;

pub enum ParameterOperation {
    Read(CTCModbusParameter),
    // ReadVector(&Vec<'static CTCModbusParameter>),
    Write(CTCModbusParameter, f32),
    /// Write `SmartGrid` mode to register 1100
    /// This uses Control Parameters that support unlimited writes with keepalive
    WriteSmartGrid(crate::modbus::SmartGridMode),
}

pub struct CtcActor {
    pub receiver: mpsc::Receiver<(
        ParameterOperation,
        oneshot::Sender<Result<f32, ModbusError>>,
    )>,
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

    pub fn build(
        self,
        receiver: mpsc::Receiver<(
            ParameterOperation,
            oneshot::Sender<Result<f32, ModbusError>>,
        )>,
    ) -> io::Result<CtcActor> {
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
        })
    }
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

    async fn read_parameter(&mut self, param: &CTCModbusParameter) -> Result<f32, ModbusError> {
        let param_id = param.id;
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            // Add delay with exponential backoff (except first attempt)
            if attempt > 0 {
                let delay = self.calculate_retry_delay(attempt);
                debug!(
                    "Retry attempt {}/{} for read_holding_registers (register {}), delay: {}ms",
                    attempt,
                    self.max_retries,
                    param_id,
                    delay.as_millis()
                );
                sleep(delay).await;
            }

            // Execute with timeout
            let operation = async {
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
                                debug!(
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
            };

            match timeout(self.operation_timeout, operation).await {
                Ok(Ok(result)) => {
                    self.record_success();
                    debug!(
                        "read_holding_registers succeeded on attempt {} (register {})",
                        attempt + 1,
                        param_id
                    );
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    warn!(
                        "read_holding_registers failed on attempt {}/{}: {} (register {})",
                        attempt + 1,
                        self.max_retries + 1,
                        e,
                        param_id
                    );
                    last_error = Some(e);
                }
                Err(_elapsed) => {
                    let timeout_err = ModbusError::Timeout {
                        register: param_id,
                        operation: format!(
                            "read_holding_registers timed out after {:?}",
                            self.operation_timeout
                        ),
                    };
                    warn!(
                        "read_holding_registers timeout on attempt {}/{} (register {})",
                        attempt + 1,
                        self.max_retries + 1,
                        param_id
                    );
                    last_error = Some(timeout_err);
                }
            }
        }

        // All retries exhausted
        self.record_failure();

        let final_error = last_error.unwrap_or_else(|| ModbusError::ProtocolError {
            reason: "No error captured during retries".to_string(),
        });

        error!(
            "read_holding_registers failed after {} attempts (register {}): {}",
            self.max_retries + 1,
            param_id,
            final_error
        );

        Err(ModbusError::MaxRetriesExceeded {
            register: param_id,
            retries: self.max_retries,
            last_error: final_error.to_string(),
        })
    }

    #[allow(clippy::too_many_lines)]
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
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            // Add delay with exponential backoff (except first attempt)
            if attempt > 0 {
                let delay = self.calculate_retry_delay(attempt);
                debug!(
                    "Retry attempt {}/{} for read_validation_parameters (register {}), delay: {}ms",
                    attempt,
                    self.max_retries,
                    reg_max,
                    delay.as_millis()
                );
                sleep(delay).await;
            }

            // Execute with timeout
            let operation = async {
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
                                debug!(
                                    "ctc_actor::read_min_max: Raw values max/min {:?}",
                                    raw_values
                                );
                                Ok((raw_values[0], raw_values[1], raw_values[2]))
                            })
                    })
            };

            match timeout(self.operation_timeout, operation).await {
                Ok(Ok(result)) => {
                    self.record_success();
                    debug!(
                        "read_validation_parameters succeeded on attempt {} (register {})",
                        attempt + 1,
                        reg_max
                    );
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    warn!(
                        "read_validation_parameters failed on attempt {}/{}: {} (register {})",
                        attempt + 1,
                        self.max_retries + 1,
                        e,
                        reg_max
                    );
                    last_error = Some(e);
                }
                Err(_elapsed) => {
                    let timeout_err = ModbusError::Timeout {
                        register: reg_max,
                        operation: format!(
                            "read_validation_parameters timed out after {:?}",
                            self.operation_timeout
                        ),
                    };
                    warn!(
                        "read_validation_parameters timeout on attempt {}/{} (register {})",
                        attempt + 1,
                        self.max_retries + 1,
                        reg_max
                    );
                    last_error = Some(timeout_err);
                }
            }
        }

        // All retries exhausted
        self.record_failure();

        let final_error = last_error.unwrap_or_else(|| ModbusError::ProtocolError {
            reason: "No error captured during retries".to_string(),
        });

        error!(
            "read_validation_parameters failed after {} attempts (register {}): {}",
            self.max_retries + 1,
            reg_max,
            final_error
        );

        Err(ModbusError::MaxRetriesExceeded {
            register: reg_max,
            retries: self.max_retries,
            last_error: final_error.to_string(),
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn write_parameter(
        &mut self,
        param: &CTCModbusParameter,
        value: f32,
    ) -> Result<(), ModbusError> {
        debug!(
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

        let raw_value = param.get_raw_value(value);
        debug!(
            "ctc_actor::write_parameter: Converted to raw_value={}",
            raw_value
        );

        debug!("ctc_actor::write_parameter: Reading min/max/step for validation");
        let (max, min, step) = self.read_min_max_step(param).await?;

        debug!(
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

        debug!("ctc_actor::write_parameter: Validation PASSED");

        debug!(
            "ctc_actor::write_parameter: Calling Modbus write_single_register for register {}",
            param.id
        );

        // Write with retry and timeout
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            // Add delay with exponential backoff (except first attempt)
            if attempt > 0 {
                let delay = self.calculate_retry_delay(attempt);
                debug!(
                    "Retry attempt {}/{} for write_single_register (register {}), delay: {}ms",
                    attempt,
                    self.max_retries,
                    param.id,
                    delay.as_millis()
                );
                sleep(delay).await;
            }

            // Execute with timeout
            let operation = async {
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
            };

            match timeout(self.operation_timeout, operation).await {
                Ok(Ok(())) => {
                    self.record_success();
                    debug!(
                        "write_single_register succeeded on attempt {} (register {})",
                        attempt + 1,
                        param.id
                    );
                    debug!("ctc_actor::write_parameter: Modbus write SUCCESS");
                    return Ok(());
                }
                Ok(Err(e)) => {
                    warn!(
                        "write_single_register failed on attempt {}/{}: {} (register {})",
                        attempt + 1,
                        self.max_retries + 1,
                        e,
                        param.id
                    );
                    last_error = Some(e);
                }
                Err(_elapsed) => {
                    let timeout_err = ModbusError::Timeout {
                        register: param.id,
                        operation: format!(
                            "write_single_register timed out after {:?}",
                            self.operation_timeout
                        ),
                    };
                    warn!(
                        "write_single_register timeout on attempt {}/{} (register {})",
                        attempt + 1,
                        self.max_retries + 1,
                        param.id
                    );
                    last_error = Some(timeout_err);
                }
            }
        }

        // All retries exhausted
        self.record_failure();

        let final_error = last_error.unwrap_or_else(|| ModbusError::ProtocolError {
            reason: "No error captured during retries".to_string(),
        });

        error!(
            "write_single_register failed after {} attempts (register {}): {}",
            self.max_retries + 1,
            param.id,
            final_error
        );

        Err(ModbusError::MaxRetriesExceeded {
            register: param.id,
            retries: self.max_retries,
            last_error: final_error.to_string(),
        })
    }

    /// Write `SmartGrid` mode to register 1100
    /// This is a simplified write without validation since `SmartGrid` control
    /// uses Control Parameters (1000-1999) that support unlimited writes
    async fn write_smartgrid(
        &mut self,
        mode: crate::modbus::SmartGridMode,
    ) -> Result<(), ModbusError> {
        use crate::modbus::SMARTGRID_CONTROL_REGISTER;

        let register = SMARTGRID_CONTROL_REGISTER;
        let raw_value = mode.to_register_value();

        debug!(
            "ctc_actor::write_smartgrid: Writing mode={} (0x{:04X}) to register {}",
            mode, raw_value, register
        );

        // Write with retry and timeout
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            // Add delay with exponential backoff (except first attempt)
            if attempt > 0 {
                let delay = self.calculate_retry_delay(attempt);
                debug!(
                    "Retry attempt {}/{} for write_smartgrid (register {}), delay: {}ms",
                    attempt,
                    self.max_retries,
                    register,
                    delay.as_millis()
                );
                sleep(delay).await;
            }

            // Execute with timeout
            let operation = async {
                self.context
                    .write_single_register(register, raw_value)
                    .await
                    .map_err(|e| {
                        error!("ctc_actor::write_smartgrid: Modbus write FAILED: {}", e);
                        ModbusError::WriteError {
                            register,
                            value: f32::from(raw_value),
                            reason: format!("{e}"),
                        }
                    })
                    .and_then(|result| {
                        result.map_err(|e| ModbusError::WriteError {
                            register,
                            value: f32::from(raw_value),
                            reason: format!("Modbus exception: {e}"),
                        })
                    })
            };

            match timeout(self.operation_timeout, operation).await {
                Ok(Ok(())) => {
                    self.record_success();
                    debug!(
                        "write_smartgrid succeeded on attempt {} (register {})",
                        attempt + 1,
                        register
                    );
                    return Ok(());
                }
                Ok(Err(e)) => {
                    warn!(
                        "write_smartgrid failed on attempt {}/{}: {} (register {})",
                        attempt + 1,
                        self.max_retries + 1,
                        e,
                        register
                    );
                    last_error = Some(e);
                }
                Err(_elapsed) => {
                    let timeout_err = ModbusError::Timeout {
                        register,
                        operation: format!(
                            "write_smartgrid timed out after {:?}",
                            self.operation_timeout
                        ),
                    };
                    warn!(
                        "write_smartgrid timeout on attempt {}/{} (register {})",
                        attempt + 1,
                        self.max_retries + 1,
                        register
                    );
                    last_error = Some(timeout_err);
                }
            }
        }

        // All retries exhausted
        self.record_failure();

        let final_error = last_error.unwrap_or_else(|| ModbusError::ProtocolError {
            reason: "No error captured during retries".to_string(),
        });

        error!(
            "write_smartgrid failed after {} attempts (register {}): {}",
            self.max_retries + 1,
            register,
            final_error
        );

        Err(ModbusError::MaxRetriesExceeded {
            register,
            retries: self.max_retries,
            last_error: final_error.to_string(),
        })
    }

    /// Handle a read operation
    async fn handle_read_operation(
        &mut self,
        param: &CTCModbusParameter,
        respond_to: ResponseChannel,
    ) {
        debug!("ctc_actor::run: Operation=READ, parameter={:?}", param);
        match self.read_parameter(param).await {
            Ok(value) => {
                debug!(
                    "ctc_actor::run: Read SUCCESS, value={}, sending response",
                    value
                );
                respond_to.send(Ok(value)).unwrap_or_else(|e| {
                    error!(
                        "ctc_actor::run: CRITICAL - Failed to send read response on oneshot channel: {e:?}"
                    );
                });
                debug!("ctc_actor::run: Read response sent");
            }
            Err(e) => {
                error!("ctc_actor::run: Read FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send read error response: {e:?}");
                });
                debug!("ctc_actor::run: Read error response sent");
            }
        }
    }

    /// Handle a `SmartGrid` write operation
    async fn handle_smartgrid_operation(
        &mut self,
        mode: SmartGridMode,
        respond_to: ResponseChannel,
    ) {
        debug!("ctc_actor::run: Operation=WRITE_SMARTGRID, mode={}", mode);
        match self.write_smartgrid(mode).await {
            Ok(()) => {
                debug!("ctc_actor::run: WriteSmartGrid SUCCESS");
                // Return 0.0 as dummy value since WriteSmartGrid doesn't return a value
                respond_to.send(Ok(0.0)).unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send success response: {e:?}");
                });
            }
            Err(e) => {
                error!("ctc_actor::run: WriteSmartGrid FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send error response: {e:?}");
                });
            }
        }
    }

    /// Handle a write operation with verification
    async fn handle_write_operation(
        &mut self,
        param: &CTCModbusParameter,
        value: f32,
        respond_to: ResponseChannel,
    ) {
        debug!(
            "ctc_actor::run: Operation=WRITE, parameter={:?}, value={}",
            param, value
        );
        match self.write_parameter(param, value).await {
            Ok(()) => {
                debug!("ctc_actor::run: Write SUCCESS, reading back to verify");
                match self.read_parameter(param).await {
                    Ok(return_value) => {
                        debug!(
                            "ctc_actor::run: Read-back value={}, comparing with written value={}",
                            return_value, value
                        );

                        if (return_value - value).abs() < f32::EPSILON {
                            debug!("ctc_actor::run: Read-back MATCHES, sending success response");
                            respond_to.send(Ok(value)).unwrap_or_else(|e| {
                                error!(
                                    "ctc_actor::run: CRITICAL - Failed to send success response: {e:?}"
                                );
                            });
                            debug!("ctc_actor::run: Write operation COMPLETE");
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
                            debug!("ctc_actor::run: Write operation FAILED (mismatch)");
                        }
                    }
                    Err(e) => {
                        error!("ctc_actor::run: Read-back FAILED: {}", e);
                        respond_to.send(Err(e)).unwrap_or_else(|e| {
                            error!(
                                "ctc_actor::run: CRITICAL - Failed to send read-back error: {e:?}"
                            );
                        });
                        debug!("ctc_actor::run: Write operation FAILED (read-back error)");
                    }
                }
            }
            Err(e) => {
                error!("ctc_actor::run: Write FAILED: {}", e);
                respond_to.send(Err(e)).unwrap_or_else(|e| {
                    error!("ctc_actor::run: CRITICAL - Failed to send write error response: {e:?}");
                });
                debug!("ctc_actor::run: Write error response sent");
            }
        }
    }

    /// Main actor loop that processes incoming parameter operations.
    /// Handles both read and write operations for Modbus parameters.
    pub async fn run(&mut self) {
        info!("ctc_actor::run: Actor loop starting");
        loop {
            debug!("ctc_actor::run: Waiting for next message");
            tokio::select! {
                Some((operation, respond_to)) = self.receiver.recv() => {
                    debug!("ctc_actor::run: Message received from channel");
                    match operation {
                        ParameterOperation::Read(param) => {
                            self.handle_read_operation(&param, respond_to).await;
                        },
                        ParameterOperation::WriteSmartGrid(mode) => {
                            self.handle_smartgrid_operation(mode, respond_to).await;
                        },
                        ParameterOperation::Write(param, value) => {
                            self.handle_write_operation(&param, value, respond_to).await;
                        }
                    }
                    debug!("ctc_actor::run: Message processing complete, looping back");
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
