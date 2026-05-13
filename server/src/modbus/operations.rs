//! Helper functions for Modbus parameter operations
//!
//! This module provides generic helper functions to reduce code duplication
//! across API endpoint handlers.

use std::time::Duration;
use tokio::time::timeout;
use tracing::{error, trace};

use crate::error::ApiError;
use crate::modbus::{CTCModbusParameter, ModbusResponse, ModbusSender, ParameterOperation};

/// Helper function to read a parameter and return just the value (not JSON)
///
/// This is useful when you need to read multiple parameters and build a custom response
pub async fn read_parameter_value(
    tx: &ModbusSender,
    param: CTCModbusParameter,
    log_context: &str,
    request_timeout: Duration,
) -> Result<f32, ApiError> {
    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(param), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    // Wait for response on this request's channel with timeout
    match timeout(request_timeout, response_rx).await {
        Ok(Ok(Ok(ModbusResponse::Value(value)))) => {
            trace!("{log_context}: {value}");
            Ok(value)
        }
        Ok(Ok(Ok(ModbusResponse::RawRegisters { .. }))) => {
            error!("Unexpected RawRegisters response in {log_context}");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Err(e))) => {
            // Log full error details internally
            error!("Error reading parameter in {log_context}: {e}");
            // Convert to ApiError (minimal exposure to client)
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!("Failed to receive response in {log_context}: {e}");
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "Request timeout in {log_context} after {:?}",
                request_timeout
            );
            Err(ApiError::Timeout)
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
/// * `request_timeout` - Timeout duration for waiting for actor response
///
/// # Returns
/// JSON string with the parameter value or an error
pub async fn read_parameter(
    tx: &ModbusSender,
    param: CTCModbusParameter,
    json_key: &str,
    log_context: &str,
    request_timeout: Duration,
) -> Result<String, ApiError> {
    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(param), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    // Wait for response on this request's channel with timeout
    match timeout(request_timeout, response_rx).await {
        Ok(Ok(Ok(ModbusResponse::Value(value)))) => {
            trace!("{log_context}: {value}");
            format_value_json(json_key, value)
        }
        Ok(Ok(Ok(ModbusResponse::RawRegisters { .. }))) => {
            error!("Unexpected RawRegisters response in {log_context}");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Err(e))) => {
            // Log full error details internally
            error!("Error reading parameter in {log_context}: {e}");
            // Convert to ApiError (minimal exposure to client)
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!("Failed to receive response in {log_context}: {e}");
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "Request timeout in {log_context} after {:?}",
                request_timeout
            );
            Err(ApiError::Timeout)
        }
    }
}

/// Format a single `{json_key: value}\n` response as proper JSON.
///
/// Uses `serde_json` so the value is emitted as a JSON number rather than
/// relying on `Debug`, which can produce odd shapes for integer-coded
/// registers and is not guaranteed to be valid JSON for all `f32` values.
fn format_value_json(json_key: &str, value: f32) -> Result<String, ApiError> {
    let body = serde_json::json!({ json_key: value });
    serde_json::to_string(&body).map(|s| s + "\n").map_err(|e| {
        error!("Failed to serialize {json_key} response: {e}");
        ApiError::InternalError
    })
}

/// Generic helper function to write a Modbus parameter
///
/// # Arguments
/// * `tx` - The Modbus actor sender channel
/// * `param` - The parameter to write
/// * `value` - The value to write
/// * `json_key` - The JSON key name for the response
/// * `log_context` - Context string for logging (e.g., `set_room_set_temp`)
/// * `request_timeout` - Timeout duration for waiting for actor response
///
/// # Returns
/// JSON string with the written value or an error
pub async fn write_parameter(
    tx: &ModbusSender,
    param: CTCModbusParameter,
    value: f32,
    json_key: &str,
    log_context: &str,
    request_timeout: Duration,
) -> Result<String, ApiError> {
    trace!(
        "{log_context}: write_parameter START - param={:?}, value={}",
        param, value
    );

    // Create a oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    trace!("{log_context}: write_parameter - Sending to actor channel");

    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Write(param, value), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    trace!("{log_context}: write_parameter - Message sent, awaiting response");

    // Wait for response on this request's channel with timeout
    match timeout(request_timeout, response_rx).await {
        Ok(Ok(Ok(_))) => {
            trace!("{log_context}: write_parameter - Response received: SUCCESS");
            format_value_json(json_key, value)
        }
        Ok(Ok(Err(e))) => {
            // Log full error details internally
            error!("{log_context}: write_parameter - Response received: ERROR - {e}");
            // Convert to ApiError (minimal exposure to client)
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!("{log_context}: write_parameter - Failed to receive response: {e}");
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "Request timeout in {log_context} after {:?}",
                request_timeout
            );
            Err(ApiError::Timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ModbusError;
    use crate::modbus::actor::ModbusResult;
    use crate::modbus::bms_parameters::CTC_ROOM_TEMP;
    use tokio::sync::mpsc;

    type MockReceiver = mpsc::Receiver<(
        ParameterOperation,
        tokio::sync::oneshot::Sender<ModbusResult>,
    )>;

    /// Helper to create a mock sender channel for testing
    fn create_mock_channel() -> (ModbusSender, MockReceiver) {
        mpsc::channel(10)
    }

    #[tokio::test]
    async fn test_read_parameter_value_success() {
        let (tx, mut rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        // Spawn a task to handle the request
        let handle = tokio::spawn(async move {
            read_parameter_value(&tx, CTC_ROOM_TEMP, "test_context", timeout).await
        });

        // Receive the request and respond
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx.send(Ok(ModbusResponse::Value(22.5))).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert!((result.unwrap() - 22.5).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_read_parameter_value_modbus_error() {
        let (tx, mut rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        let handle = tokio::spawn(async move {
            read_parameter_value(&tx, CTC_ROOM_TEMP, "test_context", timeout).await
        });

        // Respond with a Modbus error
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::ReadError {
                    register: 1000,
                    reason: "Test error".to_string(),
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InternalError));
    }

    #[tokio::test]
    async fn test_read_parameter_value_timeout() {
        let (tx, _rx) = create_mock_channel();
        // Very short timeout to trigger timeout error
        let timeout = Duration::from_millis(1);

        // Don't respond, let it timeout
        let result = read_parameter_value(&tx, CTC_ROOM_TEMP, "test_context", timeout).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    #[tokio::test]
    async fn test_read_parameter_value_channel_closed() {
        let (tx, rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        // Drop the receiver immediately to simulate actor shutdown
        drop(rx);

        let result = read_parameter_value(&tx, CTC_ROOM_TEMP, "test_context", timeout).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_read_parameter_success() {
        let (tx, mut rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        let handle = tokio::spawn(async move {
            read_parameter(&tx, CTC_ROOM_TEMP, "temperature", "test_context", timeout).await
        });

        // Receive the request and respond
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx.send(Ok(ModbusResponse::Value(22.5))).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "{\"temperature\":22.5}\n");
    }

    #[tokio::test]
    async fn test_read_parameter_modbus_error() {
        let (tx, mut rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        let handle = tokio::spawn(async move {
            read_parameter(&tx, CTC_ROOM_TEMP, "temperature", "test_context", timeout).await
        });

        // Respond with a Modbus error
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::Timeout {
                    register: 1000,
                    operation: "read".to_string(),
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    #[tokio::test]
    async fn test_read_parameter_timeout() {
        let (tx, _rx) = create_mock_channel();
        let timeout = Duration::from_millis(1);

        let result =
            read_parameter(&tx, CTC_ROOM_TEMP, "temperature", "test_context", timeout).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    #[tokio::test]
    async fn test_read_parameter_channel_closed() {
        let (tx, rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        drop(rx);

        let result =
            read_parameter(&tx, CTC_ROOM_TEMP, "temperature", "test_context", timeout).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_write_parameter_success() {
        let (tx, mut rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        let handle = tokio::spawn(async move {
            write_parameter(
                &tx,
                CTC_ROOM_TEMP,
                23.0,
                "temperature",
                "test_context",
                timeout,
            )
            .await
        });

        // Receive the request and respond
        if let Some((ParameterOperation::Write(_, value), response_tx)) = rx.recv().await {
            assert!((value - 23.0).abs() < f32::EPSILON);
            response_tx.send(Ok(ModbusResponse::Value(value))).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "{\"temperature\":23.0}\n");
    }

    #[tokio::test]
    async fn test_write_parameter_modbus_error() {
        let (tx, mut rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        let handle = tokio::spawn(async move {
            write_parameter(
                &tx,
                CTC_ROOM_TEMP,
                23.0,
                "temperature",
                "test_context",
                timeout,
            )
            .await
        });

        // Respond with a Modbus error
        if let Some((ParameterOperation::Write(_, _), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::ReadOnly { register: 1000 }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest));
    }

    #[tokio::test]
    async fn test_write_parameter_timeout() {
        let (tx, _rx) = create_mock_channel();
        let timeout = Duration::from_millis(1);

        let result = write_parameter(
            &tx,
            CTC_ROOM_TEMP,
            23.0,
            "temperature",
            "test_context",
            timeout,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    #[tokio::test]
    async fn test_write_parameter_channel_closed() {
        let (tx, rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        drop(rx);

        let result = write_parameter(
            &tx,
            CTC_ROOM_TEMP,
            23.0,
            "temperature",
            "test_context",
            timeout,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_write_parameter_verification_error() {
        let (tx, mut rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        let handle = tokio::spawn(async move {
            write_parameter(
                &tx,
                CTC_ROOM_TEMP,
                23.0,
                "temperature",
                "test_context",
                timeout,
            )
            .await
        });

        // Respond with verification error
        if let Some((ParameterOperation::Write(_, _), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::VerificationError {
                    register: 1000,
                    expected: 230.0,
                    actual: 220.0,
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InternalError));
    }

    #[tokio::test]
    async fn test_read_parameter_value_out_of_range_error() {
        let (tx, mut rx) = create_mock_channel();
        let timeout = Duration::from_secs(5);

        let handle = tokio::spawn(async move {
            read_parameter_value(&tx, CTC_ROOM_TEMP, "test_context", timeout).await
        });

        // Respond with out of range error
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::OutOfRange {
                    register: 1000,
                    value: 100.0,
                    min: 0.0,
                    max: 50.0,
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest));
    }
}
