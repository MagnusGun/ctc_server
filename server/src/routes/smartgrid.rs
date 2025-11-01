//! `SmartGrid` control API endpoints
//!
//! These endpoints provide control over the `SmartGrid` functionality using register 1100.
//! `SmartGrid` control uses Control Parameters (1000-1999) which support unlimited writes
//! with keepalive, preventing flash write cycle exhaustion on configuration registers.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use axum::{
    Router,
    extract::{Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use tracing::{debug, error};

use crate::error::ApiError;
use crate::modbus::{ModbusSender, ParameterOperation, SmartGridKeepalive, SmartGridMode};

/// State for `SmartGrid` routes
#[derive(Clone)]
pub struct SmartGridState {
    sender: ModbusSender,
    keepalive: Arc<RwLock<SmartGridKeepalive>>,
    request_timeout_secs: u64,
}

pub fn routes(
    sender: ModbusSender,
    keepalive: SmartGridKeepalive,
    request_timeout_secs: u64,
) -> Router {
    let state = SmartGridState {
        sender,
        keepalive: Arc::new(RwLock::new(keepalive)),
        request_timeout_secs,
    };

    Router::new()
        .route("/api/v1/smartgrid", get(get_smartgrid))
        .route("/api/v1/smartgrid", post(set_smartgrid))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct SmartGridQuery {
    mode: String,
}

/// Set `SmartGrid` mode
/// POST /api/v1/smartgrid?mode=blocking
///
/// Valid modes: normal, blocking, lowprice, overcapacity
async fn set_smartgrid(
    State(state): State<SmartGridState>,
    Query(query): Query<SmartGridQuery>,
) -> Result<String, ApiError> {
    debug!("set_smartgrid: START - mode={}", query.mode);

    // Parse the mode string
    let mode = SmartGridMode::from_str(&query.mode).map_err(|e| {
        error!("set_smartgrid: Invalid mode '{}': {}", query.mode, e);
        ApiError::BadRequest
    })?;

    debug!("set_smartgrid: Parsed mode={}", mode);

    // Update the keepalive state
    {
        let keepalive = state.keepalive.read().await;
        keepalive.set_mode(mode);
        debug!("set_smartgrid: Keepalive state updated");
    }

    // Send immediate write to actor
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((ParameterOperation::WriteSmartGrid(mode), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    debug!("set_smartgrid: Write request sent to actor");

    // Wait for response with timeout
    match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx).await {
        Ok(Ok(Ok(_))) => {
            debug!("set_smartgrid: SUCCESS");
            Ok(format!("{{\"smartgrid_mode\": \"{mode}\"}}\n"))
        }
        Ok(Ok(Err(e))) => {
            error!("set_smartgrid: Modbus error - {}", e);
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!("set_smartgrid: Failed to receive response - {}", e);
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "set_smartgrid: Timeout after {}s",
                state.request_timeout_secs
            );
            Err(ApiError::Timeout)
        }
    }
}

/// Get current `SmartGrid` mode
/// GET /api/v1/smartgrid
async fn get_smartgrid(State(state): State<SmartGridState>) -> Result<String, ApiError> {
    debug!("get_smartgrid: START");

    let keepalive = state.keepalive.read().await;
    let mode = keepalive.get_mode();

    debug!("get_smartgrid: Current mode={}", mode);

    Ok(format!("{{\"smartgrid_mode\": \"{mode}\"}}\n"))
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

    fn create_mock_state() -> (SmartGridState, MockReceiver) {
        let (tx, rx) = mpsc::channel(10);
        let keepalive = SmartGridKeepalive::new(tx.clone(), SmartGridMode::Normal, Some(240));
        let state = SmartGridState {
            sender: tx,
            keepalive: Arc::new(RwLock::new(keepalive)),
            request_timeout_secs: 5,
        };
        (state, rx)
    }

    #[tokio::test]
    async fn test_get_smartgrid() {
        let (state, _rx) = create_mock_state();

        let result = get_smartgrid(State(state)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "{\"smartgrid_mode\": \"Normal\"}\n");
    }

    #[tokio::test]
    async fn test_set_smartgrid_success() {
        let (state, mut rx) = create_mock_state();

        let query = SmartGridQuery {
            mode: "blocking".to_string(),
        };

        let handle = tokio::spawn(async move { set_smartgrid(State(state), Query(query)).await });

        // Receive the write request
        if let Some((ParameterOperation::WriteSmartGrid(mode), response_tx)) = rx.recv().await {
            assert_eq!(mode, SmartGridMode::Blocking);
            response_tx.send(Ok(0.0)).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "{\"smartgrid_mode\": \"Blocking\"}\n");
    }

    #[tokio::test]
    async fn test_set_smartgrid_invalid_mode() {
        let (state, _rx) = create_mock_state();

        let query = SmartGridQuery {
            mode: "invalid_mode".to_string(),
        };

        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest));
    }

    #[tokio::test]
    async fn test_set_smartgrid_all_modes() {
        for mode_str in &["normal", "blocking", "lowprice", "overcapacity"] {
            let (state, mut rx) = create_mock_state();

            let query = SmartGridQuery {
                mode: (*mode_str).to_string(),
            };

            let handle =
                tokio::spawn(async move { set_smartgrid(State(state), Query(query)).await });

            // Receive and respond
            if let Some((ParameterOperation::WriteSmartGrid(_), response_tx)) = rx.recv().await {
                response_tx.send(Ok(0.0)).unwrap();
            }

            let result = handle.await.unwrap();
            assert!(result.is_ok(), "Failed for mode: {mode_str}");
        }
    }

    #[tokio::test]
    async fn test_set_smartgrid_modbus_error() {
        let (state, mut rx) = create_mock_state();

        let query = SmartGridQuery {
            mode: "blocking".to_string(),
        };

        let handle = tokio::spawn(async move { set_smartgrid(State(state), Query(query)).await });

        // Respond with error
        if let Some((ParameterOperation::WriteSmartGrid(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::Timeout {
                    register: 1100,
                    operation: "test".to_string(),
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }
}
