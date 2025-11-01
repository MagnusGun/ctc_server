//! Helper functions for Modbus parameter operations
//!
//! This module provides generic helper functions to reduce code duplication
//! across API endpoint handlers.

use tracing::{debug, error};

use crate::error::ApiError;
use crate::modbus::CTCModbusParameter;
use crate::routes::ctc_actor::{ModbusSender, ParameterOperation};

/// Helper function to read a parameter and return just the value (not JSON)
///
/// This is useful when you need to read multiple parameters and build a custom response
pub async fn read_parameter_value(
    tx: &ModbusSender,
    param: CTCModbusParameter,
    log_context: &str,
) -> Result<f32, ApiError> {
    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(param), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    // Wait for response on this request's channel
    match response_rx.await {
        Ok(Ok(value)) => {
            debug!("{log_context}: {value}");
            Ok(value)
        }
        Ok(Err(e)) => {
            // Log full error details internally
            error!("Error reading parameter in {log_context}: {e}");
            // Convert to ApiError (minimal exposure to client)
            Err(ApiError::from(e))
        }
        Err(e) => {
            error!("Failed to receive response in {log_context}: {e}");
            Err(ApiError::ServiceUnavailable)
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
) -> Result<String, ApiError> {
    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(param), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    // Wait for response on this request's channel
    match response_rx.await {
        Ok(Ok(value)) => {
            debug!("{log_context}: {value}");
            Ok(format!("{{\"{json_key}\": {value}}}\n"))
        }
        Ok(Err(e)) => {
            // Log full error details internally
            error!("Error reading parameter in {log_context}: {e}");
            // Convert to ApiError (minimal exposure to client)
            Err(ApiError::from(e))
        }
        Err(e) => {
            error!("Failed to receive response in {log_context}: {e}");
            Err(ApiError::ServiceUnavailable)
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
) -> Result<String, ApiError> {
    debug!(
        "{log_context}: write_parameter START - param={:?}, value={}",
        param, value
    );

    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    debug!("{log_context}: write_parameter - Sending to actor channel");

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Write(param, value), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    debug!("{log_context}: write_parameter - Message sent, awaiting response");

    // Wait for response on this request's channel
    match response_rx.await {
        Ok(Ok(_)) => {
            debug!("{log_context}: write_parameter - Response received: SUCCESS");
            Ok(format!("{{\"{json_key}\": {value}}}\n"))
        }
        Ok(Err(e)) => {
            // Log full error details internally
            error!("{log_context}: write_parameter - Response received: ERROR - {e}");
            // Convert to ApiError (minimal exposure to client)
            Err(ApiError::from(e))
        }
        Err(e) => {
            error!("{log_context}: write_parameter - Failed to receive response: {e}");
            Err(ApiError::ServiceUnavailable)
        }
    }
}
