//! Test-only fake Modbus actor: answers `ParameterOperation` requests from a
//! canned map of register address -> raw register value, so route handlers can
//! exercise their success paths without a serial port.
//!
//! The fake mirrors the real actor's response shapes (see
//! `server/src/modbus/actor.rs`):
//! - `Read(param)`           -> `ModbusResponse::Value(param.get_scaled_value(raw))`
//! - `Write(param, value)`   -> `ModbusResponse::Value(value)` (echo)
//! - `ReadVisibility(reg)`   -> `ModbusResponse::Value(raw as f32)` (raw bitmask)
//! - `ReadAllVisibility`     -> `ModbusResponse::RawRegisters { start, values }`
//! - `ReadRawRegisters {..}` -> `ModbusResponse::RawRegisters { start, values }`
//! - `WriteRawRegister {..}` -> `ModbusResponse::Value(value as f32)` (echo)
//! - `GetStats`              -> `Err(ModbusError::ProtocolError)`
//!
//! `GetStats` is not synthesized: `ModbusStats` is `pub(crate)`, has no public
//! constructor, and no route-handler success path consumes a `Stats` response
//! (only the dedicated stats endpoint does, which has its own tests). Returning
//! an error keeps the fake honest rather than fabricating a misleading payload.
//!
//! Unknown register reads default to `0`.

use std::collections::HashMap;

use tokio::sync::mpsc;

use crate::error::ModbusError;
use crate::modbus::actor::ModbusRequest;
use crate::modbus::{ModbusResponse, ModbusSender, ParameterOperation};

/// First visibility register address, mirroring the actor's constant.
const VISIBILITY_REG_START: u16 = 62500;

/// Spawn a fake Modbus actor and return a [`ModbusSender`] wired to it.
///
/// `reads` maps a register address to the raw `u16` value the fake returns
/// for reads of that register. For `Read(param)` ops the raw value is scaled
/// exactly as the real actor does (via [`CTCModbusParameter::get_scaled_value`]).
/// Write ops echo the written value back. Any register not present in `reads`
/// returns `0`.
///
/// [`CTCModbusParameter::get_scaled_value`]: crate::modbus::CTCModbusParameter::get_scaled_value
#[must_use]
pub fn spawn_fake_actor(reads: HashMap<u16, u16>) -> ModbusSender {
    let (tx, mut rx) = mpsc::channel::<ModbusRequest>(8);

    tokio::spawn(async move {
        while let Some((op, respond_to)) = rx.recv().await {
            let result = match op {
                ParameterOperation::Read(param) => {
                    let raw = reads.get(&param.id).copied().unwrap_or(0);
                    Ok(ModbusResponse::Value(param.get_scaled_value(raw)))
                }
                ParameterOperation::Write(_, value) => Ok(ModbusResponse::Value(value)),
                ParameterOperation::ReadVisibility(register) => {
                    let raw = reads.get(&register).copied().unwrap_or(0);
                    Ok(ModbusResponse::Value(f32::from(raw)))
                }
                ParameterOperation::ReadAllVisibility => {
                    // The real actor returns all cached visibility registers.
                    // Here we synthesize a single register starting at the base
                    // address; tests that care about specific values seed `reads`.
                    let raw = reads.get(&VISIBILITY_REG_START).copied().unwrap_or(0);
                    Ok(ModbusResponse::RawRegisters {
                        start: VISIBILITY_REG_START,
                        values: vec![raw],
                    })
                }
                ParameterOperation::ReadRawRegisters { start, count } => {
                    let values = (0..count)
                        .map(|offset| {
                            let reg = start.wrapping_add(offset);
                            reads.get(&reg).copied().unwrap_or(0)
                        })
                        .collect();
                    Ok(ModbusResponse::RawRegisters { start, values })
                }
                ParameterOperation::WriteRawRegister { value, .. } => {
                    Ok(ModbusResponse::Value(f32::from(value)))
                }
                ParameterOperation::GetStats => Err(ModbusError::ProtocolError {
                    reason: "fake_actor does not synthesize stats".to_string(),
                }),
            };

            // Ignore send errors: the caller may have dropped the oneshot
            // receiver (e.g. timed out), which is not a fault of the fake.
            let _ = respond_to.send(result);
        }
    });

    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modbus::bms_parameters::HEATSYSTEM_ROOM_SETTEMP;

    /// Smoke test: spawn the fake, send a `Read` for a known register, and
    /// assert the scaled value comes back, proving the helper works end to end.
    #[tokio::test]
    async fn read_returns_scaled_value() {
        // HEATSYSTEM_ROOM_SETTEMP has factor 0.1, so raw 221 -> 22.1.
        let mut reads = HashMap::new();
        reads.insert(HEATSYSTEM_ROOM_SETTEMP.id, 221_u16);

        let tx = spawn_fake_actor(reads);

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        tx.send((
            ParameterOperation::Read(HEATSYSTEM_ROOM_SETTEMP),
            response_tx,
        ))
        .await
        .unwrap();

        match response_rx.await.unwrap().unwrap() {
            ModbusResponse::Value(v) => {
                assert!((v - 22.1).abs() < f32::EPSILON, "expected 22.1, got {v}");
            }
            other => panic!("expected Value response, got {other:?}"),
        }
    }

    /// Unknown registers read as 0 (scaled).
    #[tokio::test]
    async fn unknown_register_reads_zero() {
        let tx = spawn_fake_actor(HashMap::new());

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        tx.send((
            ParameterOperation::Read(HEATSYSTEM_ROOM_SETTEMP),
            response_tx,
        ))
        .await
        .unwrap();

        match response_rx.await.unwrap().unwrap() {
            ModbusResponse::Value(v) => {
                assert!(v.abs() < f32::EPSILON, "expected 0.0, got {v}");
            }
            other => panic!("expected Value response, got {other:?}"),
        }
    }

    /// Write echoes the written value back.
    #[tokio::test]
    async fn write_echoes_value() {
        let tx = spawn_fake_actor(HashMap::new());

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        tx.send((
            ParameterOperation::Write(HEATSYSTEM_ROOM_SETTEMP, 23.0),
            response_tx,
        ))
        .await
        .unwrap();

        match response_rx.await.unwrap().unwrap() {
            ModbusResponse::Value(v) => {
                assert!((v - 23.0).abs() < f32::EPSILON, "expected 23.0, got {v}");
            }
            other => panic!("expected Value response, got {other:?}"),
        }
    }
}
