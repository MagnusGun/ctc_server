//! `SmartGrid` control API endpoints
//!
//! These endpoints provide control over the `SmartGrid` functionality using GPIO relays.
//! `SmartGrid` modes are set by controlling the K24 (Smart A) and K25 (Smart B) GPIO pins
//! which correspond to the CTC heat pump's external smart grid input terminals.
//!
//! All side effects are dispatched through [`SmartGridHandle`], an mpsc
//! sender backed by the [`crate::smartgrid::actor`] task. Commands are
//! processed serially in that task, so concurrent POSTs cannot interleave
//! the bump → cancel → set → schedule sequence.

use std::str::FromStr;

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use chrono::{DateTime, SecondsFormat, Utc};

use crate::config::SmartGridConfig;
use crate::energy::price::PriceState;
use crate::error::ApiError;
use crate::smartgrid::{SmartGridError, SmartGridHandle, SmartGridMode};

/// State for `SmartGrid` routes.
#[derive(Clone)]
pub struct SmartGridState {
    handle: Option<SmartGridHandle>,
    price_state: PriceState,
    config: SmartGridConfig,
}

pub fn routes(
    handle: Option<SmartGridHandle>,
    price_state: PriceState,
    config: SmartGridConfig,
    _request_timeout_secs: u64,
) -> Router {
    let state = SmartGridState {
        handle,
        price_state,
        config,
    };

    Router::new()
        .route("/api/v1/smartgrid", get(get_smartgrid))
        .route("/api/v1/smartgrid", post(set_smartgrid))
        .route(
            "/api/v1/smartgrid/proposed_resume",
            get(get_proposed_resume),
        )
        .route(
            "/api/v1/smartgrid/scheduled_resume",
            delete(delete_scheduled_resume),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct SmartGridQuery {
    mode: String,
    #[serde(default)]
    schedule_resume: bool,
}

#[derive(Debug, Serialize)]
struct SmartGridResponse {
    smartgrid_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduled_resume_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct SetSmartGridResponse {
    smartgrid_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduled_resume_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProposedResumeResponse {
    starts_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spot_sek: Option<f64>,
    window_hours: u64,
}

/// Set `SmartGrid` mode via GPIO.
///
/// `POST /api/v1/smartgrid?mode=blocking&schedule_resume=true`
///
/// Valid modes: normal, blocking, lowprice, overcapacity
///
/// When `mode=blocking&schedule_resume=true`, the server picks the cheapest
/// 15-minute price slot inside the configured horizon and schedules an
/// automatic flip back to Normal at that time.
async fn set_smartgrid(
    State(state): State<SmartGridState>,
    Query(query): Query<SmartGridQuery>,
) -> Result<String, ApiError> {
    debug!(
        "set_smartgrid: START - mode={} schedule_resume={}",
        query.mode, query.schedule_resume
    );

    let handle = state.handle.as_ref().ok_or_else(|| {
        error!("set_smartgrid: GPIO not available - SmartGrid control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    let mode = SmartGridMode::from_str(&query.mode).map_err(|e| {
        error!("set_smartgrid: Invalid mode '{}': {}", query.mode, e);
        ApiError::BadRequest
    })?;

    let fires_at = handle
        .set_mode(mode, query.schedule_resume)
        .await
        .map_err(map_smartgrid_error)?;

    let response = SetSmartGridResponse {
        smartgrid_mode: mode.to_string(),
        scheduled_resume_at: fires_at.map(format_system_time),
    };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("set_smartgrid: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Get current `SmartGrid` mode from GPIO.
///
/// GET /api/v1/smartgrid
async fn get_smartgrid(State(state): State<SmartGridState>) -> Result<String, ApiError> {
    debug!("get_smartgrid: START");

    let handle = state.handle.as_ref().ok_or_else(|| {
        error!("get_smartgrid: GPIO not available - SmartGrid control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    let mode = handle.read_mode().await.map_err(map_smartgrid_error)?;
    let changed_at = handle
        .mode_changed_at()
        .await
        .map_err(map_smartgrid_error)?;
    let scheduled_resume_at = handle
        .scheduled_resume_at()
        .await
        .map_err(map_smartgrid_error)?;

    let response = SmartGridResponse {
        smartgrid_mode: mode.to_string(),
        changed_at: changed_at.map(format_system_time),
        scheduled_resume_at: scheduled_resume_at.map(format_system_time),
    };

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("get_smartgrid: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Preview the cheapest slot the server *would* schedule if asked, without
/// performing any side effect. Drives the dashboard confirmation dialog.
async fn get_proposed_resume(State(state): State<SmartGridState>) -> Result<String, ApiError> {
    let window =
        std::time::Duration::from_secs(state.config.auto_resume_window_hours.saturating_mul(3600));
    let slot = state.price_state.cheapest_within(window);

    let response = ProposedResumeResponse {
        starts_at: slot.as_ref().map(|p| p.starts_at.clone()),
        spot_sek: slot.as_ref().map(|p| p.spot_sek),
        window_hours: state.config.auto_resume_window_hours,
    };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("get_proposed_resume: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Cancel any pending auto-resume without changing the current `SmartGrid` mode.
///
/// Idempotent — calling this when no schedule exists returns `204 No Content`.
async fn delete_scheduled_resume(
    State(state): State<SmartGridState>,
) -> Result<StatusCode, ApiError> {
    debug!("delete_scheduled_resume: START");

    let handle = state.handle.as_ref().ok_or_else(|| {
        error!("delete_scheduled_resume: GPIO not available");
        ApiError::ServiceUnavailable
    })?;

    handle
        .cancel_scheduled_resume()
        .await
        .map_err(map_smartgrid_error)?;

    Ok(StatusCode::NO_CONTENT)
}

fn map_smartgrid_error(err: SmartGridError) -> ApiError {
    match err {
        SmartGridError::ActorGone => {
            error!("SmartGrid: actor unavailable");
            ApiError::ServiceUnavailable
        }
        e @ (SmartGridError::Apply(_) | SmartGridError::Internal(_)) => {
            error!("SmartGrid: {e}");
            ApiError::InternalError
        }
    }
}

fn format_system_time(t: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smartgrid::actor::test_support::spawn_with_test_gpio;
    use tokio_util::sync::CancellationToken;

    fn test_config() -> SmartGridConfig {
        SmartGridConfig {
            auto_resume_enabled: true,
            auto_resume_window_hours: 8,
        }
    }

    fn create_state_without_handle() -> SmartGridState {
        SmartGridState {
            handle: None,
            price_state: PriceState::new("SE3".to_string()),
            config: test_config(),
        }
    }

    #[tokio::test]
    async fn test_get_smartgrid_no_gpio() {
        let state = create_state_without_handle();
        let result = get_smartgrid(State(state)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_set_smartgrid_no_gpio() {
        let state = create_state_without_handle();
        let query = SmartGridQuery {
            mode: "blocking".to_string(),
            schedule_resume: false,
        };
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_set_smartgrid_invalid_mode() {
        let state = create_state_without_handle();
        let query = SmartGridQuery {
            mode: "invalid_mode".to_string(),
            schedule_resume: false,
        };
        // With no GPIO, we get ServiceUnavailable before mode validation.
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_proposed_resume_no_prices() {
        let state = create_state_without_handle();
        let result = get_proposed_resume(State(state)).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(result.trim()).unwrap();
        assert!(parsed["starts_at"].is_null());
        assert_eq!(parsed["window_hours"], 8);
    }

    #[tokio::test]
    async fn test_delete_scheduled_resume_no_gpio() {
        let state = create_state_without_handle();
        let result = delete_scheduled_resume(State(state)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_delete_scheduled_resume_with_handle_returns_no_content() {
        let cancel = CancellationToken::new();
        let (handle, _join) =
            spawn_with_test_gpio(PriceState::new("SE3".to_string()), test_config(), cancel);
        let state = SmartGridState {
            handle: Some(handle),
            price_state: PriceState::new("SE3".to_string()),
            config: test_config(),
        };
        let status = delete_scheduled_resume(State(state.clone())).await.unwrap();
        assert_eq!(status, StatusCode::NO_CONTENT);

        let again = delete_scheduled_resume(State(state)).await.unwrap();
        assert_eq!(again, StatusCode::NO_CONTENT);
    }
}
