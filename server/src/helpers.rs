//! Helper functions for Modbus parameter operations
//!
//! This module provides generic helper functions to reduce code duplication
//! across API endpoint handlers.

use axum::http::StatusCode;
use tracing::{debug, error};

use crate::modbus::CTCModbusParameter;
use crate::routes::ctc_actor::{ModbusSender, ParameterOperation};

/// Helper function to read a parameter and return just the value (not JSON)
///
/// This is useful when you need to read multiple parameters and build a custom response
pub async fn read_parameter_value(
    tx: &ModbusSender,
    param: CTCModbusParameter,
    log_context: &str,
) -> Result<f32, (StatusCode, String)> {
    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(param), response_tx))
        .await
        .unwrap();

    // Wait for response on this request's channel
    match response_rx.await {
        Ok(Ok(value)) => {
            debug!("{log_context}: {value}");
            Ok(value)
        }
        Ok(Err(e)) => {
            error!("Error reading parameter in {log_context}: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        }
        Err(e) => {
            error!("Failed to receive response in {log_context}: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to receive response".to_string(),
            ))
        }
    }
}

/// Generic helper function to read a Modbus parameter
///
/// # Arguments
/// * `tx` - The Modbus actor sender channel
/// * `param` - The parameter to read
/// * `json_key` - The JSON key name for the response
/// * `log_context` - Context string for logging (e.g., `get_room_temp`)
///
/// # Returns
/// JSON string with the parameter value or an error
pub async fn read_parameter(
    tx: &ModbusSender,
    param: CTCModbusParameter,
    json_key: &str,
    log_context: &str,
) -> Result<String, (StatusCode, String)> {
    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(param), response_tx))
        .await
        .unwrap();

    // Wait for response on this request's channel
    match response_rx.await {
        Ok(Ok(value)) => {
            debug!("{log_context}: {value}");
            Ok(format!("{{\"{json_key}\": {value}}}\n"))
        }
        Ok(Err(e)) => {
            error!("Error reading parameter in {log_context}: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        }
        Err(e) => {
            error!("Failed to receive response in {log_context}: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to receive response".to_string(),
            ))
        }
    }
}

/// Generic helper function to write a Modbus parameter
///
/// # Arguments
/// * `tx` - The Modbus actor sender channel
/// * `param` - The parameter to write
/// * `value` - The value to write
/// * `json_key` - The JSON key name for the response
/// * `log_context` - Context string for logging (e.g., `set_room_set_temp`)
///
/// # Returns
/// JSON string with the written value or an error
pub async fn write_parameter(
    tx: &ModbusSender,
    param: CTCModbusParameter,
    value: f32,
    json_key: &str,
    log_context: &str,
) -> Result<String, (StatusCode, String)> {
    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Write(param, value), response_tx))
        .await
        .unwrap();

    // Wait for response on this request's channel
    match response_rx.await {
        Ok(Ok(_)) => {
            debug!("{log_context}: {value}");
            Ok(format!("{{\"{json_key}\": {value}}}\n"))
        }
        Ok(Err(e)) => {
            error!("Error writing parameter in {log_context}: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        }
        Err(e) => {
            error!("Failed to receive response in {log_context}: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to receive response".to_string(),
            ))
        }
    }
}
