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
use tracing::{debug, error};

use crate::error::ApiError;
use crate::modbus::{ModbusSender, ParameterOperation};

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
        .route("/api/v1/visibility/{register}", get(get_visibility))
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
        Ok(Ok(Ok(value))) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ModbusError;
    use tokio::sync::mpsc;

    type MockReceiver = mpsc::Receiver<(
        ParameterOperation,
        tokio::sync::oneshot::Sender<Result<f32, ModbusError>>,
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
            response_tx.send(Ok(65535.0)).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("\"register\": 62500"));
        assert!(json.contains("\"value\": 65535"));
        assert!(json.contains("\"hex\": \"0xFFFF\""));
        assert!(json.contains("\"bits\": \"1111111111111111\""));
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
        assert!(matches!(result.unwrap_err(), ApiError::InternalError));
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
            response_tx.send(Ok(5.0)).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("\"register\": 62510"));
        assert!(json.contains("\"value\": 5"));
        assert!(json.contains("\"hex\": \"0x0005\""));
        assert!(json.contains("\"bits\": \"0000000000000101\""));
    }
}
