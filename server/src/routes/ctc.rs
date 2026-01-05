use std::time::Duration;

use crate::error::ApiError;
use crate::modbus::bms_parameters::{get_ctc_parameter_by_id, get_custom_ctc_parameter_by_addr};
use crate::modbus::{ModbusSender, read_parameter, write_parameter};
use crate::smartgrid::{GpioController, SmartGridMode};
use axum::{
    Router,
    extract::{Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use tracing::{debug, error};

/// State type for CTC routes
type CtcState = (ModbusSender, u64, Option<GpioController>);

pub fn routes(
    sender: ModbusSender,
    request_timeout_secs: u64,
    gpio_controller: Option<GpioController>,
) -> Router {
    Router::new()
        .route("/api/v1/ctc", get(get_ctc_data))
        .route("/api/v1/ctc", post(post_ctc_data))
        .route("/api/v1/ctc/powersave", post(set_power_save))
        .route("/api/v1/ctc/powersave", get(get_power_save))
        .with_state((sender, request_timeout_secs, gpio_controller))
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
    State((tx, timeout_secs, _)): State<CtcState>,
    Query(params): Query<CtcParams>,
) -> Result<String, ApiError> {
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
        &tx,
        param,
        "ctc_data",
        "get_ctc_data",
        Duration::from_secs(timeout_secs),
    )
    .await
}

async fn post_ctc_data(
    State((tx, timeout_secs, _)): State<CtcState>,
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
        &tx,
        param,
        value,
        "ctc_data",
        "post_ctc_data",
        Duration::from_secs(timeout_secs),
    )
    .await
}

#[derive(Debug, Deserialize)]
struct PowerSave {
    active: bool,
}

/// Set power save mode via GPIO
///
/// Requires GPIO to be enabled in configuration.
async fn set_power_save(
    State((_, _, gpio)): State<CtcState>,
    Query(params): Query<PowerSave>,
) -> Result<String, ApiError> {
    debug!("set_power_save: START - {params:?}");

    // Check if GPIO is available
    let controller = gpio.as_ref().ok_or_else(|| {
        error!("set_power_save: GPIO not available - powersave control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    // Use SmartGrid Blocking mode for powersave, Normal when inactive
    let mode = if params.active {
        SmartGridMode::Blocking
    } else {
        SmartGridMode::Normal
    };

    debug!("set_power_save: Setting SmartGrid mode to {mode} via GPIO");

    controller.set_mode(mode).map_err(|e| {
        error!("set_power_save: GPIO error - {e}");
        ApiError::InternalError
    })?;

    debug!("set_power_save: SUCCESS via GPIO");
    Ok(format!(
        "{{\"powersave\": {}, \"mode\": \"{mode}\"}}\n",
        params.active
    ))
}

/// Get power save status from GPIO
///
/// Requires GPIO to be enabled in configuration.
async fn get_power_save(State((_, _, gpio)): State<CtcState>) -> Result<String, ApiError> {
    debug!("get_power_save: START");

    // Check if GPIO is available
    let controller = gpio.as_ref().ok_or_else(|| {
        error!("get_power_save: GPIO not available - powersave control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    // Read mode from GPIO
    let mode = controller.read_mode().map_err(|e| {
        error!("get_power_save: GPIO read error - {e}");
        ApiError::InternalError
    })?;

    let active = matches!(mode, SmartGridMode::Blocking);

    debug!("get_power_save: Current mode={mode}, powersave active={active}");

    Ok(format!(
        "{{\"powersave\": {active}, \"mode\": \"{mode}\"}}\n"
    ))
}
