use std::time::{Duration, SystemTime};

use crate::config::SmartGridConfig;
use crate::energy::price::PriceState;
use crate::error::ApiError;
use crate::modbus::bms_parameters::{get_ctc_parameter_by_id, get_custom_ctc_parameter_by_addr};
use crate::modbus::{ModbusSender, read_parameter, write_parameter};
use crate::smartgrid::{GpioController, SmartGridMode, apply_mode};
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
    sender: ModbusSender,
    request_timeout_secs: u64,
    gpio: Option<GpioController>,
    price_state: PriceState,
    smartgrid_config: SmartGridConfig,
}

pub fn routes(
    sender: ModbusSender,
    request_timeout_secs: u64,
    gpio_controller: Option<GpioController>,
    price_state: PriceState,
    smartgrid_config: SmartGridConfig,
) -> Router {
    let state = CtcState {
        sender,
        request_timeout_secs,
        gpio: gpio_controller,
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
    /// cheapest 15-min slot inside the configured window.
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

    let controller = state.gpio.as_ref().ok_or_else(|| {
        error!("set_power_save: GPIO not available - powersave control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    // Use SmartGrid Blocking mode for powersave, Normal when inactive.
    let mode = if params.active {
        SmartGridMode::Blocking
    } else {
        SmartGridMode::Normal
    };

    let fires_at = apply_mode(
        controller,
        mode,
        params.schedule_resume,
        &state.price_state,
        &state.smartgrid_config,
    )
    .map_err(|e| {
        error!("set_power_save: {e}");
        ApiError::InternalError
    })?;

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

    let controller = state.gpio.as_ref().ok_or_else(|| {
        error!("get_power_save: GPIO not available - powersave control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    let mode = controller.read_mode().map_err(|e| {
        error!("get_power_save: GPIO read error - {e}");
        ApiError::InternalError
    })?;

    let active = matches!(mode, SmartGridMode::Blocking);
    let scheduled_resume_at = controller.scheduled_resume_at().map(format_system_time);

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

fn format_system_time(t: SystemTime) -> String {
    DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}
