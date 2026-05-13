//! Visibility register API endpoints
//!
//! These endpoints provide access to visibility registers (62500-62548) which
//! indicate which parameters are available on the connected hardware.

use std::time::Duration;

use axum::{
    Router,
    extract::{Path, State},
    routing::get,
};
use serde::Serialize;
use tracing::{debug, error};

use crate::error::ApiError;
use crate::modbus::bms_parameters::get_ctc_parameter_by_id;
use crate::modbus::{ModbusResponse, ModbusSender, ParameterOperation};

/// Response body for `GET /api/v1/visibility/parameter/{addr}`.
///
/// Field order matches the original hand-rolled JSON so the wire format
/// is preserved. `note` is omitted when absent (matching the original
/// "known parameter, visible" branch which had no `note`); other optional
/// fields serialise as JSON `null` when `None`.
#[derive(Serialize)]
struct ParameterVisibilityResponse {
    address: u16,
    visible: Option<bool>,
    description: Option<&'static str>,
    visibility_register: Option<u16>,
    bit: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'static str>,
}

/// First visibility register address (inclusive)
const VISIBILITY_REG_START: u16 = 62500;
/// Last visibility register address (inclusive)
const VISIBILITY_REG_END: u16 = 62548;

/// State for visibility routes
#[derive(Clone)]
pub struct VisibilityState {
    sender: ModbusSender,
    request_timeout_secs: u64,
}

pub fn routes(sender: ModbusSender, request_timeout_secs: u64) -> Router {
    let state = VisibilityState {
        sender,
        request_timeout_secs,
    };

    Router::new()
        .route("/api/v1/visibility", get(get_all_visibility))
        .route("/api/v1/visibility/{register}", get(get_visibility))
        .route(
            "/api/v1/visibility/parameter/{addr}",
            get(get_parameter_visibility),
        )
        .with_state(state)
}

/// Get visibility register value
/// GET /api/v1/visibility/{register}
///
/// Returns the raw bitmask value for the specified visibility register.
/// Valid registers: 62500-62548
///
/// Response format:
/// ```json
/// {
///   "register": 62500,
///   "value": 65535,
///   "hex": "0xFFFF",
///   "bits": "1111111111111111"
/// }
/// ```
async fn get_visibility(
    State(state): State<VisibilityState>,
    Path(register): Path<u16>,
) -> Result<String, ApiError> {
    debug!("get_visibility: START - register={}", register);

    // Send read request to actor
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((ParameterOperation::ReadVisibility(register), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    debug!("get_visibility: Read request sent to actor");

    // Wait for response with timeout
    match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx).await {
        Ok(Ok(Ok(ModbusResponse::Value(value)))) => {
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            let raw_value = value as u16;
            debug!(
                "get_visibility: SUCCESS - register={register}, value={raw_value} (0x{raw_value:04X})"
            );
            Ok(format!(
                "{{\"register\": {register}, \"value\": {raw_value}, \"hex\": \"0x{raw_value:04X}\", \"bits\": \"{raw_value:016b}\"}}\n"
            ))
        }
        Ok(Ok(Ok(ModbusResponse::RawRegisters { .. }))) => {
            error!("get_visibility: Unexpected RawRegisters response");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Ok(ModbusResponse::Stats(_)))) => {
            error!("get_visibility: Unexpected Stats response");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Err(e))) => {
            error!("get_visibility: Modbus error - {}", e);
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!("get_visibility: Failed to receive response - {}", e);
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "get_visibility: Timeout after {}s",
                state.request_timeout_secs
            );
            Err(ApiError::Timeout)
        }
    }
}

/// Get all visibility registers
/// GET /api/v1/visibility
///
/// Returns all visibility registers in the configured range (62500-62548 by
/// default) with their bitmask values.
///
/// Response format:
/// ```json
/// {
///   "registers": [
///     {"register": 62500, "value": 65535, "hex": "0xFFFF"},
///     ...
///   ]
/// }
/// ```
async fn get_all_visibility(State(state): State<VisibilityState>) -> Result<String, ApiError> {
    debug!("get_all_visibility: START");

    // Send read all visibility request to actor
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((ParameterOperation::ReadAllVisibility, response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    debug!("get_all_visibility: Read request sent to actor");

    // Wait for response with timeout
    match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx).await {
        Ok(Ok(Ok(ModbusResponse::RawRegisters { start, values }))) => {
            debug!(
                "get_all_visibility: SUCCESS - {} registers starting at {}",
                values.len(),
                start
            );

            // Build JSON array of register objects
            let registers_json: Vec<String> = values
                .iter()
                .enumerate()
                .map(|(i, &value)| {
                    let register = start + u16::try_from(i).unwrap_or(0);
                    format!(
                        "{{\"register\": {register}, \"value\": {value}, \"hex\": \"0x{value:04X}\"}}"
                    )
                })
                .collect();

            Ok(format!(
                "{{\"registers\": [{}]}}\n",
                registers_json.join(", ")
            ))
        }
        Ok(Ok(Ok(ModbusResponse::Value(_)))) => {
            error!("get_all_visibility: Unexpected Value response");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Ok(ModbusResponse::Stats(_)))) => {
            error!("get_all_visibility: Unexpected Stats response");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Err(e))) => {
            error!("get_all_visibility: Modbus error - {}", e);
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!("get_all_visibility: Failed to receive response - {}", e);
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "get_all_visibility: Timeout after {}s",
                state.request_timeout_secs
            );
            Err(ApiError::Timeout)
        }
    }
}

/// Check if a specific BMS parameter is visible
/// GET /api/v1/visibility/parameter/{addr}
///
/// Returns visibility status for a BMS parameter by its address.
/// If the parameter is known, also returns its description and visibility register info.
///
/// Response format for known parameter:
/// ```json
/// {
///   "address": 61509,
///   "visible": true,
///   "description": "Heating system 1: Set room temperature",
///   "visibility_register": 62500,
///   "bit": 9
/// }
/// ```
///
/// Response format for unknown parameter (custom address):
/// ```json
/// {
///   "address": 65001,
///   "visible": null,
///   "description": null,
///   "visibility_register": null,
///   "bit": null,
///   "note": "Parameter not in BMS catalog - visibility unknown"
/// }
/// ```
#[allow(clippy::too_many_lines)]
async fn get_parameter_visibility(
    State(state): State<VisibilityState>,
    Path(addr): Path<u16>,
) -> Result<String, ApiError> {
    debug!("get_parameter_visibility: START - addr={}", addr);

    // Look up the parameter in BMS catalog
    let Some(p) = get_ctc_parameter_by_id(addr) else {
        // Parameter not in catalog
        debug!(
            "get_parameter_visibility: Parameter {} not in BMS catalog",
            addr
        );
        let response = ParameterVisibilityResponse {
            address: addr,
            visible: None,
            description: None,
            visibility_register: None,
            bit: None,
            note: Some("Parameter not in BMS catalog - visibility unknown"),
        };
        return serialize_response(&response);
    };

    // Parameter found - check its visibility
    let vis_register = p.visible;

    // Validate visibility register is in range
    if !(VISIBILITY_REG_START..=VISIBILITY_REG_END).contains(&vis_register) {
        // Parameter has visibility register 0 (always visible)
        if vis_register == 0 {
            debug!(
                "get_parameter_visibility: Parameter {} is always visible",
                addr
            );
            let response = ParameterVisibilityResponse {
                address: addr,
                visible: Some(true),
                description: Some(p.description),
                visibility_register: None,
                bit: None,
                note: Some("Always visible (no visibility check required)"),
            };
            return serialize_response(&response);
        }
        error!(
            "get_parameter_visibility: Invalid visibility register {} for parameter {}",
            vis_register, addr
        );
        return Err(ApiError::InternalError);
    }

    // Read the visibility register
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((
            ParameterOperation::ReadVisibility(vis_register),
            response_tx,
        ))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx).await {
        Ok(Ok(Ok(ModbusResponse::Value(value)))) => {
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            let raw_value = value as u16;
            let is_visible = p.is_visible(raw_value);
            debug!(
                "get_parameter_visibility: SUCCESS - addr={}, visible={}, vis_reg={}, bit={}",
                addr, is_visible, vis_register, p.bit
            );
            let response = ParameterVisibilityResponse {
                address: addr,
                visible: Some(is_visible),
                description: Some(p.description),
                visibility_register: Some(vis_register),
                bit: Some(p.bit),
                note: None,
            };
            serialize_response(&response)
        }
        Ok(Ok(Ok(ModbusResponse::RawRegisters { .. }))) => {
            error!("get_parameter_visibility: Unexpected RawRegisters response");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Ok(ModbusResponse::Stats(_)))) => {
            error!("get_parameter_visibility: Unexpected Stats response");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Err(e))) => {
            error!("get_parameter_visibility: Modbus error - {}", e);
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!(
                "get_parameter_visibility: Failed to receive response - {}",
                e
            );
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "get_parameter_visibility: Timeout after {}s",
                state.request_timeout_secs
            );
            Err(ApiError::Timeout)
        }
    }
}

fn serialize_response(resp: &ParameterVisibilityResponse) -> Result<String, ApiError> {
    let mut json = serde_json::to_string(resp).map_err(|e| {
        error!("get_parameter_visibility: Failed to serialize response - {e}");
        ApiError::InternalError
    })?;
    json.push('\n');
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ModbusError;
    use crate::modbus::actor::ModbusResult;
    use tokio::sync::mpsc;

    type MockReceiver = mpsc::Receiver<(
        ParameterOperation,
        tokio::sync::oneshot::Sender<ModbusResult>,
    )>;

    fn create_mock_state() -> (VisibilityState, MockReceiver) {
        let (tx, rx) = mpsc::channel(10);
        let state = VisibilityState {
            sender: tx,
            request_timeout_secs: 5,
        };
        (state, rx)
    }

    #[tokio::test]
    async fn test_get_visibility_success() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_visibility(State(state), Path(62500)).await });

        // Receive the read request
        if let Some((ParameterOperation::ReadVisibility(reg), response_tx)) = rx.recv().await {
            assert_eq!(reg, 62500);
            response_tx
                .send(Ok(ModbusResponse::Value(65535.0)))
                .unwrap();
        }

        let json = handle.await.unwrap().expect("get_visibility");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["register"].as_u64(), Some(62500));
        assert_eq!(v["value"].as_u64(), Some(65535));
        assert_eq!(v["hex"].as_str(), Some("0xFFFF"));
        assert_eq!(v["bits"].as_str(), Some("1111111111111111"));
    }

    #[tokio::test]
    async fn test_get_visibility_invalid_register() {
        let (state, mut rx) = create_mock_state();

        // Use a register outside the valid range but within u16 bounds
        let handle = tokio::spawn(async move { get_visibility(State(state), Path(65000)).await });

        // Receive the read request - actor should return error
        if let Some((ParameterOperation::ReadVisibility(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::InvalidVisibilityRegister(65000)))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::NotFound));
    }

    #[tokio::test]
    async fn test_get_visibility_modbus_error() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_visibility(State(state), Path(62500)).await });

        // Respond with error
        if let Some((ParameterOperation::ReadVisibility(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::Timeout {
                    register: 62500,
                    operation: "test".to_string(),
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    #[tokio::test]
    async fn test_get_visibility_channel_closed() {
        let (state, rx) = create_mock_state();

        // Drop receiver to simulate actor shutdown
        drop(rx);

        let result = get_visibility(State(state), Path(62500)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_get_visibility_partial_bits() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_visibility(State(state), Path(62510)).await });

        // Return a value with some bits set
        if let Some((ParameterOperation::ReadVisibility(reg), response_tx)) = rx.recv().await {
            assert_eq!(reg, 62510);
            // Binary: 0000000000000101 (bits 0 and 2 set)
            response_tx.send(Ok(ModbusResponse::Value(5.0))).unwrap();
        }

        let json = handle.await.unwrap().expect("get_visibility");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["register"].as_u64(), Some(62510));
        assert_eq!(v["value"].as_u64(), Some(5));
        assert_eq!(v["hex"].as_str(), Some("0x0005"));
        assert_eq!(v["bits"].as_str(), Some("0000000000000101"));
    }

    // Tests for get_all_visibility
    #[tokio::test]
    async fn test_get_all_visibility_success() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_all_visibility(State(state)).await });

        // Receive the read all visibility request
        if let Some((ParameterOperation::ReadAllVisibility, response_tx)) = rx.recv().await {
            // Return 3 sample registers for testing
            response_tx
                .send(Ok(ModbusResponse::RawRegisters {
                    start: 62500,
                    values: vec![65535, 0, 5],
                }))
                .unwrap();
        }

        let json = handle.await.unwrap().expect("get_all_visibility");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let regs = v["registers"].as_array().expect("registers array");
        assert_eq!(regs.len(), 3);
        let expected = [
            (62500u64, 65535u64, "0xFFFF"),
            (62501, 0, "0x0000"),
            (62502, 5, "0x0005"),
        ];
        for (i, (reg, val, hex)) in expected.iter().enumerate() {
            assert_eq!(regs[i]["register"].as_u64(), Some(*reg));
            assert_eq!(regs[i]["value"].as_u64(), Some(*val));
            assert_eq!(regs[i]["hex"].as_str(), Some(*hex));
        }
    }

    #[tokio::test]
    async fn test_get_all_visibility_channel_closed() {
        let (state, rx) = create_mock_state();

        // Drop receiver to simulate actor shutdown
        drop(rx);

        let result = get_all_visibility(State(state)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_get_all_visibility_modbus_error() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_all_visibility(State(state)).await });

        // Respond with error
        if let Some((ParameterOperation::ReadAllVisibility, response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::VisibilityNotScanned))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InternalError));
    }

    // Tests for get_parameter_visibility
    #[tokio::test]
    async fn test_get_parameter_visibility_known_visible() {
        let (state, mut rx) = create_mock_state();

        // Use HEATSYSTEM_ROOM_SETTEMP (61509) which has visibility register 62500, bit 9
        let handle =
            tokio::spawn(async move { get_parameter_visibility(State(state), Path(61509)).await });

        // Receive the visibility register read request
        if let Some((ParameterOperation::ReadVisibility(reg), response_tx)) = rx.recv().await {
            assert_eq!(reg, 62500); // HEATSYSTEM_ROOM_SETTEMP.visible
            // Return value with bit 9 set (0x200 = 512)
            response_tx.send(Ok(ModbusResponse::Value(512.0))).unwrap();
        }

        let json = handle.await.unwrap().expect("get_parameter_visibility");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["address"].as_u64(), Some(61509));
        assert_eq!(v["visible"].as_bool(), Some(true));
        assert_eq!(v["visibility_register"].as_u64(), Some(62500));
        assert_eq!(v["bit"].as_u64(), Some(9));
        assert!(v["description"].is_string());
    }

    #[tokio::test]
    async fn test_get_parameter_visibility_known_not_visible() {
        let (state, mut rx) = create_mock_state();

        // Use HEATSYSTEM_ROOM_SETTEMP (61509) which has visibility register 62500, bit 9
        let handle =
            tokio::spawn(async move { get_parameter_visibility(State(state), Path(61509)).await });

        // Receive the visibility register read request
        if let Some((ParameterOperation::ReadVisibility(reg), response_tx)) = rx.recv().await {
            assert_eq!(reg, 62500);
            // Return value with bit 9 NOT set (bit 8 set instead = 256)
            response_tx.send(Ok(ModbusResponse::Value(256.0))).unwrap();
        }

        let json = handle.await.unwrap().expect("get_parameter_visibility");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["address"].as_u64(), Some(61509));
        assert_eq!(v["visible"].as_bool(), Some(false));
    }

    #[tokio::test]
    async fn test_get_parameter_visibility_unknown_parameter() {
        let (state, _rx) = create_mock_state();

        // Use an address that doesn't exist in the BMS catalog (valid u16)
        let result = get_parameter_visibility(State(state), Path(64000)).await;

        let json = result.expect("get_parameter_visibility");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["address"].as_u64(), Some(64000));
        assert!(v["visible"].is_null());
        assert!(v["description"].is_null());
        assert_eq!(
            v["note"].as_str(),
            Some("Parameter not in BMS catalog - visibility unknown")
        );
    }

    #[tokio::test]
    async fn test_get_parameter_visibility_always_visible() {
        let (state, _rx) = create_mock_state();

        // CTC_ALARM_INFO_COUNT (65001) has visibility register 0 (always visible)
        let result = get_parameter_visibility(State(state), Path(65001)).await;

        let json = result.expect("get_parameter_visibility");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["address"].as_u64(), Some(65001));
        assert_eq!(v["visible"].as_bool(), Some(true));
        assert_eq!(
            v["note"].as_str(),
            Some("Always visible (no visibility check required)")
        );
    }

    #[tokio::test]
    async fn test_get_parameter_visibility_channel_closed() {
        let (state, rx) = create_mock_state();

        // Drop receiver to simulate actor shutdown
        drop(rx);

        // Use a parameter that requires visibility check
        let result = get_parameter_visibility(State(state), Path(61509)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }
}
