//! Real-world adapters that bridge the narrow `ModbusWriter` / `SgController`
//! traits used by the `DhwActor` to the production `CtcActor` (via the
//! `ModbusSender` mpsc channel) and `SmartGridHandle`.
//!
//! These are the *only* place the trait shape meets the concrete types — all
//! actor code talks to traits so tests can swap in fakes. Task 14 wires them
//! into `main.rs`.

use std::time::Duration;

use async_trait::async_trait;

use crate::dhw::actor::{ModbusWriter, SgController};
use crate::modbus::ModbusSender;
use crate::modbus::bms_parameters::get_ctc_parameter_by_id;
use crate::modbus::operations::{read_parameter_value, write_parameter_value};
use crate::smartgrid::SmartGridHandle;
use crate::smartgrid::mode::SmartGridMode;

/// Forwards scaled Modbus reads/writes to the real `CtcActor` over its mpsc
/// channel. The `CtcActor` applies the parameter's scaling factor internally,
/// so callers pass and receive values in physical units (°C, kW, …).
pub struct CtcActorModbus {
    tx: ModbusSender,
    timeout: Duration,
}

impl CtcActorModbus {
    #[must_use]
    pub fn new(tx: ModbusSender, timeout: Duration) -> Self {
        Self { tx, timeout }
    }
}

#[async_trait]
impl ModbusWriter for CtcActorModbus {
    async fn write_scaled(&self, addr: u16, value: f32) -> Result<(), String> {
        let param =
            *get_ctc_parameter_by_id(addr).ok_or_else(|| format!("unknown parameter id {addr}"))?;
        write_parameter_value(&self.tx, param, value, "dhw_adapter", self.timeout)
            .await
            .map_err(|e| format!("{e:?}"))
    }

    async fn read_scaled(&self, addr: u16) -> Result<f32, String> {
        let param =
            *get_ctc_parameter_by_id(addr).ok_or_else(|| format!("unknown parameter id {addr}"))?;
        read_parameter_value(&self.tx, param, "dhw_adapter", self.timeout)
            .await
            .map_err(|e| format!("{e:?}"))
    }
}

/// Drives `SmartGridMode::Normal` / `Overcapacity` over the real
/// `SmartGridHandle`. `schedule_resume=false` on both — the DHW actor manages
/// its own boost timer and never wants the SG auto-resume scheduler stamping
/// a separate flip on top.
pub struct SmartGridAdapter {
    handle: SmartGridHandle,
}

impl SmartGridAdapter {
    #[must_use]
    pub fn new(handle: SmartGridHandle) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl SgController for SmartGridAdapter {
    async fn set_normal(&self) -> Result<(), String> {
        self.handle
            .set_mode(SmartGridMode::Normal, false)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    async fn set_overcapacity(&self) -> Result<(), String> {
        self.handle
            .set_mode(SmartGridMode::Overcapacity, false)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
