use crate::config::PowerSaveConfig;
use crate::error::ApiError;
use crate::modbus::bms_parameters::{
    CTC_VACCATION_DAYS, HEATSYSTEM_ROOM_SETTEMP, get_ctc_parameter_by_id,
    get_custom_ctc_parameter_by_addr,
};
use axum::{
    Router,
    extract::{Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use tracing::debug;

use crate::helpers::{read_parameter, read_parameter_value, write_parameter};
use crate::routes::ctc_actor::ModbusSender;

pub fn routes(sender: ModbusSender, power_save_config: PowerSaveConfig) -> Router {
    Router::new()
        .route("/api/v1/ctc", get(get_ctc_data))
        .route("/api/v1/ctc", post(post_ctc_data))
        .route("/api/v1/ctc/powersave", post(set_power_save))
        .route("/api/v1/ctc/powersave", get(get_power_save))
        .with_state((sender, power_save_config))
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
    State((tx, _)): State<(ModbusSender, PowerSaveConfig)>,
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
    read_parameter(&tx, param, "ctc_data", "get_ctc_data").await
}

async fn post_ctc_data(
    State((tx, _)): State<(ModbusSender, PowerSaveConfig)>,
    Query(params): Query<CtcParams>,
) -> Result<String, ApiError> {
    debug!(
        "post_ctc_data: received a request to write CTC parameter with address: {}",
        params.addr
    );
    let param = *get_ctc_parameter_by_id(params.addr).ok_or(ApiError::BadRequest)?;

    let value = params.value.ok_or(ApiError::BadRequest)?;

    debug!("post_ctc_data: Found CTC parameter: {param:?}");
    write_parameter(&tx, param, value, "ctc_data", "post_ctc_data").await
}

#[derive(Debug, Deserialize)]
struct PowerSave {
    active: bool,
}

async fn set_power_save(
    State((tx, config)): State<(ModbusSender, PowerSaveConfig)>,
    Query(params): Query<PowerSave>,
) -> Result<String, ApiError> {
    debug!("set_power_save: START - {params:?}");

    let (room_temp, vacation_days) = if params.active {
        (config.low_temp, config.low_days)
    } else {
        (config.high_temp, config.high_days)
    };

    debug!(
        "set_power_save: Target values - room_temp={}, vacation_days={}",
        room_temp, vacation_days
    );

    // Update room temperature setpoint
    debug!("set_power_save: Calling write_parameter for HEATSYSTEM_ROOM_SETTEMP");
    match write_parameter(
        &tx,
        HEATSYSTEM_ROOM_SETTEMP,
        room_temp,
        "room_temperature_setpoint",
        "set_power_save",
    )
    .await
    {
        Ok(result) => {
            debug!(
                "set_power_save: HEATSYSTEM_ROOM_SETTEMP write succeeded: {}",
                result
            );
        }
        Err(e) => {
            debug!(
                "set_power_save: HEATSYSTEM_ROOM_SETTEMP write FAILED: {:?}",
                e
            );
            return Err(e);
        }
    }

    // Update vacation days
    debug!("set_power_save: Calling write_parameter for CTC_VACCATION_DAYS");
    match write_parameter(
        &tx,
        CTC_VACCATION_DAYS,
        vacation_days,
        "vacation_days",
        "set_power_save",
    )
    .await
    {
        Ok(result) => {
            debug!(
                "set_power_save: CTC_VACCATION_DAYS write succeeded: {}",
                result
            );
        }
        Err(e) => {
            debug!("set_power_save: CTC_VACCATION_DAYS write FAILED: {:?}", e);
            return Err(e);
        }
    }

    debug!("set_power_save: SUCCESS - Both writes completed");
    Ok(format!("{{\"powersave\": {}}}\n", params.active))
}

async fn get_power_save(
    State((tx, _)): State<(ModbusSender, PowerSaveConfig)>,
) -> Result<String, ApiError> {
    debug!("get_power_save");

    let room_setpoint =
        read_parameter_value(&tx, HEATSYSTEM_ROOM_SETTEMP, "get_power_save:room_setpoint").await?;
    let vaccation_days =
        read_parameter_value(&tx, CTC_VACCATION_DAYS, "get_power_save:vacation_days").await?;

    Ok(format!(
        "{{\"room_temp_setpoint\": {room_setpoint}, \"vaccation_days\": {vaccation_days}}}\n"
    ))
}
