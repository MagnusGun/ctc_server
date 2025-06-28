use ctc_server::modbus::CTCModbusParameter;
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
    pub fn new(tty_path: impl Into<String>) -> Self {
        Self {
            tty_path: tty_path.into(),
            baud_rate: 9600,
            data_bits: DataBits::Eight,
            parity: Parity::Even,
            stop_bits: StopBits::One,
            flow_control: FlowControl::Hardware,
            timeout: Duration::from_secs(1),
            slave_id: 1,
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
                        let scaled_values = param.get_scaled_value_vector(&raw_values);
                        scaled_values.first()
                            .copied()
                            .ok_or_else(|| format!("ctc_actor::read_parameter: No value returned for parameter {param:?}"))
                    })
            })
    }

    async fn read_min_max_step(&mut self, param: &CTCModbusParameter) -> Result<(u16, u16, u16), String> {
        if param.reg_max == 0 {
            return Err(format!("ctc_actor::read_min_max: Parameter {param:?} is not configured for min/max reading (as reg_max is 0)"));
        }
        self.context.read_holding_registers(param.reg_max, 3)
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
        if param.is_read_only() {
            return Err(format!("ctc_actor::write_parameter: Parameter {} is read-only and cannot be written to", param.id));
        }
        let scaled_value = param.to_scaled_value_vector(value);


        match self.read_min_max_step(param).await {
            Ok((max, min, step)) => {
                if scaled_value[0] < min || scaled_value[0] > max  || scaled_value[0] % step != 0 {
                    return Err(format!("ctc_actor::write_parameter: Value {value} didnt fit in min/max/step for parameter {}: max {}, min {}, step {}", param.description, param.get_scaled_value(max), param.get_scaled_value(min), param.get_scaled_value(step)));
                }
            }
            Err(e) => return Err(format!("ctc_actor::write_parameter: Error reading min/max for parameter {param:?}: {e}"))
        }

        debug!("ctc_actor::write_parameter: Writing value {value} to parameter {param:?} as scaled value {scaled_value:?}");
        match self.context.write_single_register(param.id, scaled_value[0]).await{
            Ok(_) => Ok(()),
            Err(e) => Err(format!("ctc_actor::write_parameter: Error writing value {value} to parameter {param:?}: {e}")),
        }
    }

    pub async fn run(&mut self) {
        loop {
            tokio::select! {
                Some((operation, respond_to)) = self.receiver.recv() => {
                    match operation {
                        ParameterOperation::Read(param) => {
                            debug!("ctc_actor::run: Reading parameter {param:?}");
                            match self.read_parameter(&param).await {
                                Ok(value) => {
                                    debug!("ctc_actor::run: Successfully read value {value} for parameter {param:?}");
                                    respond_to.send(Ok(value)).unwrap_or_else(|e| {
                                        error!("ctc_actor::run: Failed to send read response on the one-shot channel: {e:?}");
                                    });
                                },
                                Err(e) => {
                                    debug!("ctc_actor::run: Error reading parameter {param:?}: {e}");
                                    respond_to.send(Err(e)).unwrap_or_else(|e| {
                                        error!("ctc_actor::run: Failed to send read error response on the one-shot channel: {e:?}");
                                    });
                                }
                            }
                        },
                        ParameterOperation::Write(param, value) => {
                            debug!("ctc_actor::run: Writing value {value} to parameter {param:?}");
                            match self.write_parameter(&param, value).await {
                                Ok(()) => {
                                    debug!("ctc_actor::run: Successfully wrote value {value} to parameter {param:?}, reading back to confirm");
                                    match self.read_parameter(&param).await {
                                        Ok(return_value) => {
                                            debug!("ctc_actor::run: Successfully read back value {return_value} for parameter {param:?}");

                                            #[allow(clippy::float_cmp)]
                                            if return_value == value {
                                                debug!("ctc_actor::run: Read-back value matches written value for parameter {param:?}");
                                                respond_to.send(Ok(value)).unwrap_or_else(|e| {
                                                    error!("ctc_actor::run: Failed to send read-back response on the one-shot channel: {e:?}");
                                                });
                                            }
                                            else {
                                                error!("ctc_actor::run: Read-back value {return_value} does not match written value {value} for parameter {param:?}");
                                                respond_to.send(Err(format!("Read-back value {return_value} does not match written value {value} for parameter {param:?}"))).unwrap_or_else(|e| {
                                                    error!("ctc_actor::run: Failed to send read-back mismatch response on the one-shot channel: {e:?}");
                                                });

                                            }
                                        },
                                        Err(e) => {
                                            error!("ctc_actor::run: Error reading back parameter {param:?} after write: {e}");
                                            respond_to.send(Err(e)).unwrap_or_else(|e| {
                                                error!("ctc_actor::run: Failed to send read-back error response on the one-shot channel: {e:?}");
                                            });
                                        }
                                    }
                                },
                                Err(e) => {
                                    debug!("ctc_actor::run: Error writing parameter {param:?}: {e}");
                                    respond_to.send(Err(e)).unwrap_or_else(|e| {
                                        error!("ctc_actor::run: Failed to send write error response on the one-shot channel: {e:?}");
                                    });
                                }
                            }
                        }
                    }
                }
                else => break,
            }
        }
    }
}