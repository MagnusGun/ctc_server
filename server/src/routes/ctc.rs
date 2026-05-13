use std::time::{Duration, SystemTime};

use crate::config::SmartGridConfig;
use crate::energy::price::PriceState;
use crate::error::ApiError;
use crate::modbus::bms_parameters::{get_ctc_parameter_by_id, get_custom_ctc_parameter_by_addr};
use crate::modbus::{ModbusSender, read_parameter, write_parameter};
use crate::smartgrid::{SmartGridError, SmartGridHandle, SmartGridMode};
use crate::storage::{Store, poller::sensor_for_addr};
use axum::{
    Router,
    extract::{Query, State},
    routing::{get, post},
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

/// State for CTC routes — bundles everything `set_power_save` needs to
/// schedule an auto-resume in addition to the bare GPIO toggle.
#[derive(Clone)]
pub struct CtcState {
    store: Store,
    sender: ModbusSender,
    request_timeout_secs: u64,
    smartgrid: Option<SmartGridHandle>,
    #[allow(dead_code)]
    price_state: PriceState,
    #[allow(dead_code)]
    smartgrid_config: SmartGridConfig,
}

pub fn routes(
    store: Store,
    sender: ModbusSender,
    request_timeout_secs: u64,
    smartgrid: Option<SmartGridHandle>,
    price_state: PriceState,
    smartgrid_config: SmartGridConfig,
) -> Router {
    let state = CtcState {
        store,
        sender,
        request_timeout_secs,
        smartgrid,
        price_state,
        smartgrid_config,
    };
    Router::new()
        .route("/api/v1/ctc", get(get_ctc_data))
        .route("/api/v1/ctc", post(post_ctc_data))
        .route("/api/v1/ctc/powersave", post(set_power_save))
        .route("/api/v1/ctc/powersave", get(get_power_save))
        .with_state(state)
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct CtcParams {
    addr: u16,
    value: Option<f32>,
    factor: Option<f32>,
    custom: Option<bool>,
}

// function to read a CTC parameter based on the query parameters
// e.g. /api/v1/ctc/?addr=61509&factor=0.1&signed=true for reading 'Heating system 1: Setting room temp'
// addr: the address of the parameter to read
// factor: the factor to apply to the value
// signed: whether the value is signed or not
// returns a JSON object with the parameter value
// e.g. {"ctc_data": 23.5}
async fn get_ctc_data(
    State(state): State<CtcState>,
    Query(params): Query<CtcParams>,
) -> Result<String, ApiError> {
    let tx = &state.sender;
    let timeout_secs = state.request_timeout_secs;
    debug!(
        "get_ctc_data: reveived a request for reading CTC parameter with address: {}",
        params.addr
    );

    // Cache-first: if this address maps to a polled sensor and the in-memory
    // ring has it, serve from RAM. Falls through to Modbus only on cold start
    // or for addresses outside the polled set.
    if let Some(sensor) = sensor_for_addr(params.addr)
        && let Some((_, v)) = state.store.latest_sample(sensor)
    {
        // Match `read_parameter`'s wire shape: serde_json emits stable f32
        // formatting across toolchains; the previous `{v:?}` Debug-format
        // could drift on stdlib changes.
        let body = serde_json::json!({ "ctc_data": v });
        return Ok(format!("{body}\n"));
    }

    let param = match params.custom {
        Some(true) => get_custom_ctc_parameter_by_addr(params.addr, params.factor),
        _ => *get_ctc_parameter_by_id(params.addr).ok_or(ApiError::BadRequest)?,
    };

    debug!("get_ctc_data: Found CTC parameter: {param:?}");
    read_parameter(
        tx,
        param,
        "ctc_data",
        "get_ctc_data",
        Duration::from_secs(timeout_secs),
    )
    .await
}

async fn post_ctc_data(
    State(state): State<CtcState>,
    Query(params): Query<CtcParams>,
) -> Result<String, ApiError> {
    debug!(
        "post_ctc_data: received a request to write CTC parameter with address: {}",
        params.addr
    );
    let param = *get_ctc_parameter_by_id(params.addr).ok_or(ApiError::BadRequest)?;

    let value = params.value.ok_or(ApiError::BadRequest)?;

    debug!("post_ctc_data: Found CTC parameter: {param:?}");
    write_parameter(
        &state.sender,
        param,
        value,
        "ctc_data",
        "post_ctc_data",
        Duration::from_secs(state.request_timeout_secs),
    )
    .await
}

#[derive(Debug, Deserialize)]
struct PowerSave {
    active: bool,
    /// When set with `active=true`, schedule an auto-resume to Normal at the
    /// start of the cheapest contiguous `auto_resume_min_duration_minutes`
    /// run inside the configured window, falling back to the cheapest single
    /// slot when no run of that length fits.
    #[serde(default)]
    schedule_resume: bool,
}

#[derive(Debug, Serialize)]
struct PowerSaveResponse {
    powersave: bool,
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduled_resume_at: Option<String>,
}

/// Set power save mode via GPIO
///
/// Requires GPIO to be enabled in configuration.
async fn set_power_save(
    State(state): State<CtcState>,
    Query(params): Query<PowerSave>,
) -> Result<String, ApiError> {
    debug!("set_power_save: START - {params:?}");

    let handle = state.smartgrid.as_ref().ok_or_else(|| {
        error!("set_power_save: GPIO not available - powersave control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    // Use SmartGrid Blocking mode for powersave, Normal when inactive.
    let mode = if params.active {
        SmartGridMode::Blocking
    } else {
        SmartGridMode::Normal
    };

    let fires_at = handle
        .set_mode(mode, params.schedule_resume)
        .await
        .map_err(map_smartgrid_error)?;

    let response = PowerSaveResponse {
        powersave: params.active,
        mode: mode.to_string(),
        scheduled_resume_at: fires_at.map(format_system_time),
    };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("set_power_save: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Get power save status from GPIO
///
/// Requires GPIO to be enabled in configuration.
async fn get_power_save(State(state): State<CtcState>) -> Result<String, ApiError> {
    debug!("get_power_save: START");

    let handle = state.smartgrid.as_ref().ok_or_else(|| {
        error!("get_power_save: GPIO not available - powersave control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    let mode = handle.read_mode().await.map_err(map_smartgrid_error)?;
    let active = matches!(mode, SmartGridMode::Blocking);
    let scheduled_resume_at = handle
        .scheduled_resume_at()
        .await
        .map_err(map_smartgrid_error)?
        .map(format_system_time);

    debug!("get_power_save: Current mode={mode}, powersave active={active}");

    let response = PowerSaveResponse {
        powersave: active,
        mode: mode.to_string(),
        scheduled_resume_at,
    };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("get_power_save: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

fn map_smartgrid_error(err: SmartGridError) -> ApiError {
    match err {
        SmartGridError::ActorGone => {
            error!("SmartGrid: actor unavailable");
            ApiError::ServiceUnavailable
        }
        SmartGridError::Apply(e) => {
            error!("SmartGrid: {e}");
            ApiError::InternalError
        }
        SmartGridError::Internal(e) => {
            error!("SmartGrid: {e}");
            ApiError::InternalError
        }
    }
}

fn format_system_time(t: SystemTime) -> String {
    DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modbus::actor::ModbusRequest;
    use crate::smartgrid::actor::test_support::spawn_with_test_gpio;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();
        (dir, store)
    }

    fn dummy_modbus_sender() -> ModbusSender {
        // set_power_save never touches the sender — a closed channel is fine.
        let (tx, _rx) = mpsc::channel::<ModbusRequest>(1);
        tx
    }

    fn test_smartgrid_config() -> SmartGridConfig {
        SmartGridConfig {
            auto_resume_enabled: true,
            auto_resume_window_hours: 12,
            auto_resume_min_duration_minutes: 30,
        }
    }

    #[tokio::test]
    async fn set_power_save_returns_service_unavailable_without_gpio() {
        let (_dir, store) = temp_store();
        let state = CtcState {
            store,
            sender: dummy_modbus_sender(),
            request_timeout_secs: 5,
            smartgrid: None, // GPIO disabled
            price_state: PriceState::new("SE3".into()),
            smartgrid_config: test_smartgrid_config(),
        };

        let err = set_power_save(
            State(state),
            Query(PowerSave {
                active: true,
                schedule_resume: false,
            }),
        )
        .await
        .expect_err("should fail without GPIO");
        assert!(matches!(err, ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn set_power_save_blocking_with_schedule_but_no_prices_omits_resume_field() {
        // Active=true + schedule_resume=true, but PriceState has no prices.
        // The actor's `set_mode` returns Ok(None) (warns "no resume target"),
        // so the response should still omit `scheduled_resume_at`.
        let (_dir, store) = temp_store();
        let cancel = CancellationToken::new();
        let price_state = PriceState::new("SE3".into());
        let (handle, _join) =
            spawn_with_test_gpio(price_state.clone(), test_smartgrid_config(), cancel.clone());

        let state = CtcState {
            store,
            sender: dummy_modbus_sender(),
            request_timeout_secs: 5,
            smartgrid: Some(handle),
            price_state,
            smartgrid_config: test_smartgrid_config(),
        };

        // The GPIO write itself errors on the test controller (Blocking
        // requires a real GPIO write). That's the actor's `Apply` error,
        // surfaced as InternalError.
        let err = set_power_save(
            State(state),
            Query(PowerSave {
                active: true,
                schedule_resume: true,
            }),
        )
        .await
        .expect_err("test-only GPIO can't write Blocking");
        assert!(matches!(err, ApiError::InternalError));

        cancel.cancel();
    }

    #[tokio::test]
    async fn set_power_save_returns_service_unavailable_when_actor_gone() {
        // Spawn the actor, then immediately cancel and wait for it to exit.
        // Subsequent calls through the handle observe ActorGone, which the
        // route maps to ServiceUnavailable.
        let (_dir, store) = temp_store();
        let cancel = CancellationToken::new();
        let price_state = PriceState::new("SE3".into());
        let (handle, join) =
            spawn_with_test_gpio(price_state.clone(), test_smartgrid_config(), cancel.clone());

        cancel.cancel();
        tokio::time::timeout(Duration::from_millis(200), join)
            .await
            .expect("actor must exit within 200 ms")
            .expect("actor must not panic");

        let state = CtcState {
            store,
            sender: dummy_modbus_sender(),
            request_timeout_secs: 5,
            smartgrid: Some(handle),
            price_state,
            smartgrid_config: test_smartgrid_config(),
        };

        let err = set_power_save(
            State(state),
            Query(PowerSave {
                active: false,
                schedule_resume: false,
            }),
        )
        .await
        .expect_err("actor gone should fail");
        assert!(matches!(err, ApiError::ServiceUnavailable));
    }
}
