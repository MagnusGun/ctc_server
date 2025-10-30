use crate::modbus::CTCModbusParameter;
use tokio_modbus::client::Writer;
use tokio_serial::SerialPortBuilderExt;
use tokio::sync::{mpsc, oneshot};
use tokio_serial::{DataBits, Parity, StopBits, FlowControl};
use tokio_modbus::prelude::{Slave, rtu, Reader};
use tracing::{debug, error, info};
use std::time::Duration;
use std::io;

pub type ModbusResponse = Result<f32, String>;
pub type ResponseChannel = oneshot::Sender<ModbusResponse>;
pub type ModbusRequest = (ParameterOperation, ResponseChannel);
pub type ModbusSender = mpsc::Sender<ModbusRequest>;

pub enum ParameterOperation {
    Read(CTCModbusParameter),
    // ReadVector(&Vec<'static CTCModbusParameter>),
    Write(CTCModbusParameter, f32),
}

pub struct CtcActor {
    pub receiver: mpsc::Receiver<(ParameterOperation, oneshot::Sender<Result<f32, String>>)>,
    context: tokio_modbus::client::Context,
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
}

#[allow(dead_code)]
impl CtcActorBuilder {
    /// Create a new builder with just the TTY path
    /// All other parameters should be set via builder methods
    pub fn new(tty_path: impl Into<String>) -> Self {
        Self {
            tty_path: tty_path.into(),
            baud_rate: 9600,              // Will be overridden by config
            data_bits: DataBits::Eight,    // Will be overridden by config
            parity: Parity::Even,          // Will be overridden by config
            stop_bits: StopBits::One,      // Will be overridden by config
            flow_control: FlowControl::Hardware, // Will be overridden by config
            timeout: Duration::from_secs(1),     // Will be overridden by config
            slave_id: 1,                   // Will be overridden by config
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

    pub fn build(self, receiver: mpsc::Receiver<(ParameterOperation, oneshot::Sender<Result<f32, String>>)>) -> io::Result<CtcActor> {
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
            context: ctx 
        })
    }
}

impl CtcActor {
    async fn read_parameter(&mut self, param: &CTCModbusParameter) -> Result<f32, String> {
        self.context.read_holding_registers(param.id, 1)
            .await
            .map_err(|e| format!("ctc_actor::read_parameter: Error reading parameter {param:?}: {e}"))
            .and_then(|raw_values| {
                raw_values
                    .map_err(|e| format!("ctc_actor::read_parameter: Error reading raw values for parameter {param:?}: {e}"))
                    .and_then(|raw_values| {
                        debug!("ctc_actor::read_parameter: Raw values for parameter {param:?}: {raw_values:?}");
                        let scaled_values = param.get_scaled_value_vector(&raw_values);
                        scaled_values.first()
                            .copied()
                            .ok_or_else(|| format!("ctc_actor::read_parameter: No value returned for parameter {param:?}"))
                    })
            })
    }

    async fn read_min_max_step(&mut self, param: &CTCModbusParameter) -> Result<(u16, u16, u16), String> {
        let Some(reg_max) = param.reg_max else { return Err(format!("ctc_actor::read_min_max: Parameter {param:?} is not configured for min/max reading (as reg_max is None)")) };
        
        self.context.read_holding_registers(reg_max, 3)
            .await
            .map_err(|e| format!("ctc_actor::read_min_max: Error reading min/max for parameter {param:?}: {e}"))
            .and_then(|raw_values| {
                raw_values
                    .map_err(|e| format!("ctc_actor::read_min_max: Error reading raw values for parameter {param:?}: {e}"))
                    .and_then(|raw_values| {
                        if raw_values.len() < 2 {
                            return Err(format!("ctc_actor::read_min_max: Not enough values returned for parameter {param:?}"));
                        }
                        debug!("ctc_actor::read_min_max: Raw values max/min {raw_values:?}");
                        Ok((raw_values[0], raw_values[1], raw_values[2]))
                    })
            })
    }

    async fn write_parameter(&mut self, param: &CTCModbusParameter, value: f32) -> Result<(), String> {
        debug!("ctc_actor::write_parameter: START - param={:?}, value={}", param, value);

        if param.is_read_only() {
            error!("ctc_actor::write_parameter: Parameter {} is read-only", param.id);
            return Err(format!("ctc_actor::write_parameter: Parameter {} is read-only and cannot be written to", param.id));
        }

        let raw_value = param.get_raw_value(value);
        debug!("ctc_actor::write_parameter: Converted to raw_value={}", raw_value);

        debug!("ctc_actor::write_parameter: Reading min/max/step for validation");
        match self.read_min_max_step(param).await {
            Ok((max, min, step)) => {
                debug!("ctc_actor::write_parameter: Validation bounds - min={}, max={}, step={}", min, max, step);
                // Check if value is within range and is a valid step from the minimum
                if raw_value < min || raw_value > max  || !(raw_value - min).is_multiple_of(step) {
                    error!("ctc_actor::write_parameter: VALIDATION FAILED - raw_value={} not in range [{}, {}] or not valid step from min", raw_value, min, max);
                    return Err(format!("ctc_actor::write_parameter: Value {value} didnt fit in min/max/step for parameter {}: max {}, min {}, step {}", param.description, param.get_scaled_value(max), param.get_scaled_value(min), param.get_scaled_value(step)));
                }
                debug!("ctc_actor::write_parameter: Validation PASSED");
            }
            Err(e) => {
                error!("ctc_actor::write_parameter: Failed to read min/max/step: {}", e);
                return Err(format!("ctc_actor::write_parameter: Error reading min/max for parameter {param:?}: {e}"));
            }
        }

        debug!("ctc_actor::write_parameter: Calling Modbus write_single_register for register {}", param.id);
        match self.context.write_single_register(param.id, raw_value).await{
            Ok(_) => {
                debug!("ctc_actor::write_parameter: Modbus write SUCCESS");
                Ok(())
            }
            Err(e) => {
                error!("ctc_actor::write_parameter: Modbus write FAILED: {}", e);
                Err(format!("ctc_actor::write_parameter: Error writing value {value} to parameter {param:?}: {e}"))
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
                            debug!("ctc_actor::run: Operation=READ, parameter={:?}", param);
                            match self.read_parameter(&param).await {
                                Ok(value) => {
                                    debug!("ctc_actor::run: Read SUCCESS, value={}, sending response", value);
                                    respond_to.send(Ok(value)).unwrap_or_else(|e| {
                                        error!("ctc_actor::run: CRITICAL - Failed to send read response on oneshot channel: {e:?}");
                                    });
                                    debug!("ctc_actor::run: Read response sent");
                                },
                                Err(e) => {
                                    error!("ctc_actor::run: Read FAILED: {}", e);
                                    respond_to.send(Err(e)).unwrap_or_else(|e| {
                                        error!("ctc_actor::run: CRITICAL - Failed to send read error response: {e:?}");
                                    });
                                    debug!("ctc_actor::run: Read error response sent");
                                }
                            }
                        },
                        ParameterOperation::Write(param, value) => {
                            debug!("ctc_actor::run: Operation=WRITE, parameter={:?}, value={}", param, value);
                            match self.write_parameter(&param, value).await {
                                Ok(()) => {
                                    debug!("ctc_actor::run: Write SUCCESS, reading back to verify");
                                    match self.read_parameter(&param).await {
                                        Ok(return_value) => {
                                            debug!("ctc_actor::run: Read-back value={}, comparing with written value={}", return_value, value);

                                            // Consider using an epsilon-based comparison for better float handling
                                            // #[allow(clippy::float_cmp)]
                                            if (return_value - value).abs() < f32::EPSILON {
                                                debug!("ctc_actor::run: Read-back MATCHES, sending success response");
                                                respond_to.send(Ok(value)).unwrap_or_else(|e| {
                                                    error!("ctc_actor::run: CRITICAL - Failed to send success response: {e:?}");
                                                });
                                                debug!("ctc_actor::run: Write operation COMPLETE");
                                            }
                                            else {
                                                error!("ctc_actor::run: Read-back MISMATCH: wrote {} but read {}", value, return_value);
                                                respond_to.send(Err(format!("Read-back value {return_value} does not match written value {value} for parameter {param:?}"))).unwrap_or_else(|e| {
                                                    error!("ctc_actor::run: CRITICAL - Failed to send mismatch error: {e:?}");
                                                });
                                                debug!("ctc_actor::run: Write operation FAILED (mismatch)");
                                            }
                                        },
                                        Err(e) => {
                                            error!("ctc_actor::run: Read-back FAILED: {}", e);
                                            respond_to.send(Err(e)).unwrap_or_else(|e| {
                                                error!("ctc_actor::run: CRITICAL - Failed to send read-back error: {e:?}");
                                            });
                                            debug!("ctc_actor::run: Write operation FAILED (read-back error)");
                                        }
                                    }
                                },
                                Err(e) => {
                                    error!("ctc_actor::run: Write FAILED: {}", e);
                                    respond_to.send(Err(e)).unwrap_or_else(|e| {
                                        error!("ctc_actor::run: CRITICAL - Failed to send write error response: {e:?}");
                                    });
                                    debug!("ctc_actor::run: Write error response sent");
                                }
                            }
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
        error!("ctc_actor::run: Actor loop has EXITED - this should not happen in normal operation!");
    }
}