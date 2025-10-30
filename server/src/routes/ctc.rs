use crate::modbus::bms_parameters::{get_ctc_parameter_by_id, get_custom_ctc_parameter_by_addr, CTC_VACCATION_DAYS, HEATSYSTEM_ROOM_SETTEMP};
use axum::{extract::{Query, State}, http::StatusCode, routing::{get, post}, Router};
use serde::Deserialize;
use tracing::debug;

use crate::helpers::{read_parameter, read_parameter_value, write_parameter};
use crate::routes::ctc_actor::{ModbusSender, ParameterOperation};

pub fn routes(sender: tokio::sync::mpsc::Sender<(ParameterOperation, tokio::sync::oneshot::Sender<Result<f32,String>>)>) 
    -> Router {
    Router::new()
        .route("/api/v1/ctc", get(get_ctc_data))
        .route("/api/v1/ctc", post(post_ctc_data)) 
        .route("/api/v1/ctc/powersave", post(set_power_save))
        .route("/api/v1/ctc/powersave", get(get_power_save))
        .with_state(sender)
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
async fn get_ctc_data(State(tx): State<ModbusSender>, Query(params): Query<CtcParams>) -> Result<String, (StatusCode, String)>{
    debug!("get_ctc_data: reveived a request for reading CTC parameter with address: {}", params.addr);

    let param = match params.custom {
        Some(true) => get_custom_ctc_parameter_by_addr(params.addr, params.factor),
        _ => *get_ctc_parameter_by_id(params.addr)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, format!("get_ctc_data: No CTC parameter found with address: {}", params.addr)))?,
    };

    debug!("get_ctc_data: Found CTC parameter: {param:?}");
    read_parameter(&tx, param, "ctc_data", "get_ctc_data").await
}

async fn post_ctc_data(State(tx): State<ModbusSender>, Query(params): Query<CtcParams>) -> Result<String, (StatusCode, String)> {
    debug!("post_ctc_data: received a request to write CTC parameter with address: {}", params.addr);
    let param = *get_ctc_parameter_by_id(params.addr)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, format!("post_ctc_data: No CTC parameter found with address: {}", params.addr)))?;

    let value = params.value
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "post_ctc_data: No value provided to write".to_string()))?;

    debug!("post_ctc_data: Found CTC parameter: {param:?}");
    write_parameter(&tx, param, value, "ctc_data", "post_ctc_data").await
}

#[derive(Debug, Deserialize)]
struct PowerSave {
    active: bool,
}

async fn set_power_save(State(state): State<ModbusSender>, Query(params): Query<PowerSave>) -> Result<String, (StatusCode, String)>{
    debug!("set_power_save: {params:?}");

    let (room_temp, vacation_days) = if params.active {
        (15.0, 2.0)
    } else {
        (21.5, 0.0)
    };

    // Update room temperature setpoint
    write_parameter(&state, HEATSYSTEM_ROOM_SETTEMP, room_temp, "room_temperature_setpoint", "set_power_save").await?;

    // Update vacation days
    write_parameter(&state, CTC_VACCATION_DAYS, vacation_days, "vacation_days", "set_power_save").await?;

    Ok(format!("{{\"powersave\": {}}}\n", params.active))
}

async fn get_power_save(State(tx): State<ModbusSender>) -> Result<String, (StatusCode, String)>{
    debug!("get_power_save");

    let room_setpoint = read_parameter_value(&tx, HEATSYSTEM_ROOM_SETTEMP, "get_power_save:room_setpoint").await?;
    let vaccation_days = read_parameter_value(&tx, CTC_VACCATION_DAYS, "get_power_save:vacation_days").await?;

    Ok(format!("{{\"room_temp_setpoint\": {room_setpoint}, \"vaccation_days\": {vaccation_days}}}\n"))
}