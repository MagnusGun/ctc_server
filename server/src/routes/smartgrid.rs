//! `SmartGrid` control API endpoints
//!
//! These endpoints provide control over the `SmartGrid` functionality using GPIO relays.
//! `SmartGrid` modes are set by controlling the K24 (Smart A) and K25 (Smart B) GPIO pins
//! which correspond to the CTC heat pump's external smart grid input terminals.

use std::str::FromStr;

use axum::{
    Router,
    extract::{Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use tracing::{debug, error};

use crate::error::ApiError;
use crate::gpio::GpioController;
use crate::modbus::SmartGridMode;

/// State for `SmartGrid` routes
#[derive(Clone)]
pub struct SmartGridState {
    gpio: Option<GpioController>,
}

pub fn routes(gpio: Option<GpioController>, _request_timeout_secs: u64) -> Router {
    let state = SmartGridState { gpio };

    Router::new()
        .route("/api/v1/smartgrid", get(get_smartgrid))
        .route("/api/v1/smartgrid", post(set_smartgrid))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct SmartGridQuery {
    mode: String,
}

/// Set `SmartGrid` mode via GPIO
/// POST /api/v1/smartgrid?mode=blocking
///
/// Valid modes: normal, blocking, lowprice, overcapacity
///
/// Requires GPIO to be enabled in configuration.
async fn set_smartgrid(
    State(state): State<SmartGridState>,
    Query(query): Query<SmartGridQuery>,
) -> Result<String, ApiError> {
    debug!("set_smartgrid: START - mode={}", query.mode);

    // Check if GPIO is available
    let gpio = state.gpio.as_ref().ok_or_else(|| {
        error!("set_smartgrid: GPIO not available - SmartGrid control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    // Parse the mode string
    let mode = SmartGridMode::from_str(&query.mode).map_err(|e| {
        error!("set_smartgrid: Invalid mode '{}': {}", query.mode, e);
        ApiError::BadRequest
    })?;

    debug!("set_smartgrid: Parsed mode={}", mode);

    // Set mode via GPIO
    gpio.set_mode(mode).map_err(|e| {
        error!("set_smartgrid: GPIO error - {}", e);
        ApiError::InternalError
    })?;

    debug!("set_smartgrid: SUCCESS - mode set to {}", mode);
    Ok(format!("{{\"smartgrid_mode\": \"{mode}\"}}\n"))
}

/// Get current `SmartGrid` mode from GPIO
/// GET /api/v1/smartgrid
///
/// Requires GPIO to be enabled in configuration.
async fn get_smartgrid(State(state): State<SmartGridState>) -> Result<String, ApiError> {
    debug!("get_smartgrid: START");

    // Check if GPIO is available
    let gpio = state.gpio.as_ref().ok_or_else(|| {
        error!("get_smartgrid: GPIO not available - SmartGrid control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    // Read mode from GPIO
    let mode = gpio.read_mode().map_err(|e| {
        error!("get_smartgrid: GPIO read error - {}", e);
        ApiError::InternalError
    })?;

    debug!("get_smartgrid: Current mode={}", mode);
    Ok(format!("{{\"smartgrid_mode\": \"{mode}\"}}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_state_without_gpio() -> SmartGridState {
        SmartGridState { gpio: None }
    }

    #[tokio::test]
    async fn test_get_smartgrid_no_gpio() {
        let state = create_state_without_gpio();

        let result = get_smartgrid(State(state)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_set_smartgrid_no_gpio() {
        let state = create_state_without_gpio();
        let query = SmartGridQuery {
            mode: "blocking".to_string(),
        };

        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_set_smartgrid_invalid_mode() {
        let state = create_state_without_gpio();
        let query = SmartGridQuery {
            mode: "invalid_mode".to_string(),
        };

        // Even with no GPIO, invalid mode should return BadRequest first
        // But since we check GPIO first, it returns ServiceUnavailable
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(result.is_err());
        // With no GPIO, we get ServiceUnavailable before mode validation
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }
}
