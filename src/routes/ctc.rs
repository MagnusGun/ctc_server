use ctc_server::modbus::bms_parameters::{get_ctc_parameter_by_id, get_custom_ctc_parameter_by_addr, CTC_VACCATION_DAYS, HEATSYSTEM_ROOM_SETTEMP};
use axum::{extract::{Query, State}, http::StatusCode, routing::{get, post}, Router};
use serde::Deserialize;
use tracing::{debug, error};

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
        Some(true) => &get_custom_ctc_parameter_by_addr(params.addr),
        _ => get_ctc_parameter_by_id(params.addr)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, format!("get_ctc_data: No CTC parameter found with address: {}", params.addr)))?,
    };
 
    // Create a oneshot channel for THIS REQUEST
    debug!("get_ctc_data: Found CTC parameter: {param:?} creating oneshot channel");
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // Send the operation and response channel to the actor
    debug!("get_ctc_data: sending request for parameter: {param:?}");
    tx.send((ParameterOperation::Read(*param), response_tx)).await.unwrap();
    
    // Wait for response on THIS request's channel
    match response_rx.await {
        Ok(Ok(rsp)) => {
            debug!("get_ctc_data: Got response for parameter {}: {rsp}", params.addr);
            Ok(format!("{{\"ctc_data\": {rsp}}}\n"))
        },
        Ok(Err(e)) => {
            error!("get_ctc_data: Error reading CTC parameter: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        },
        Err(e) => {
            error!("get_ctc_data: Failed to receive response: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
        }
    }
}

async fn post_ctc_data(State(tx): State<ModbusSender>, Query(params): Query<CtcParams>) -> Result<String, (StatusCode, String)> {
    debug!("post_ctc_data: received a request to write CTC parameter with address: {}", params.addr);
    let param = get_ctc_parameter_by_id(params.addr)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, format!("post_ctc_data: No CTC parameter found with address: {}", params.addr)))?;

    if let Some(value) = params.value {
        // Create a oneshot channel for THIS REQUEST
        debug!("post_ctc_data: Found CTC parameter: {param:?} creating oneshot channel");
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        // Send the operation and response channel to the actor
        debug!("post_ctc_data: sending request to write value {value} for parameter: {param:?}");
        tx.send((ParameterOperation::Write(*param, value), response_tx)).await.unwrap();
        
        // Wait for response on THIS request's channel
        match response_rx.await {
            Ok(Ok(_)) => {
                debug!("post_ctc_data: Successfully wrote value {value} for parameter {}", params.addr);
                Ok(format!("{{\"ctc_data\": {value}}}\n"))
            },
            Ok(Err(e)) => {
                error!("post_ctc_data: Error writing CTC parameter: {e}");
                Err((StatusCode::INTERNAL_SERVER_ERROR, e))
            },
            Err(e) => {
                error!("post_ctc_data: Failed to receive response: {e}");
                Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
            }
        }
    } else {
        Err((StatusCode::BAD_REQUEST, "post_ctc_data: No value provided to write".to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct PowerSave {
    active: bool,
}

async fn set_power_save(State(state): State<ModbusSender>, Query(params): Query<PowerSave>) -> Result<String, (StatusCode, String)>{
    debug!("set_power_save: {params:?}");
    if params.active {


        //update the Room Temperature Setpoint to 15 degrees
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        // Send the operation and response channel to the actor
        state.send((ParameterOperation::Write(HEATSYSTEM_ROOM_SETTEMP, 15_f32), response_tx)).await.unwrap();

        // Wait for response on THIS request's channel
        match response_rx.await {
            Ok(Ok(response)) => {
                debug!("Successfully set room temperature to {response} degrees");
            },
            Ok(Err(e)) => {
                error!("Error setting room temperature: {e}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, e));
            },
            Err(e) => {
                error!("Failed to receive response: {e}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()));
            }
        }

        // Uppdate the Vacation Days to 2
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        // Send the operation and response channel to the actor
        state.send((ParameterOperation::Write(CTC_VACCATION_DAYS, 2_f32), response_tx)).await.unwrap(); 

        // Wait for response on THIS request's channel
        match response_rx.await {
            Ok(Ok(response)) => {
                debug!("Successfully set vacation days to {response}");
            },
            Ok(Err(e)) => {
                error!("Error setting vacation days: {e}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, e));
            },
            Err(e) => {
                error!("Failed to receive response: {e}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()));
            }
        }   
    } else {
        //update the Room Temperature Setpoint to 21.5 degrees
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        // Send the operation and response channel to the actor
        state.send((ParameterOperation::Write(HEATSYSTEM_ROOM_SETTEMP, 21.5_f32), response_tx)).await.unwrap();

        // Wait for response on THIS request's channel
        match response_rx.await {
            Ok(Ok(response )) => {
                debug!("Successfully set room temperature to {response} degrees");
            },
            Ok(Err(e)) => {
                error!("Error setting room temperature: {e}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, e));
            },
            Err(e) => {
                error!("Failed to receive response: {e}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()));
            }
        }   

        // Update the Vacation Days to 0
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        // Send the operation and response channel to the actor
        state.send((ParameterOperation::Write(CTC_VACCATION_DAYS, 0_f32), response_tx)).await.unwrap();
        
        // Wait for response on THIS request's channel
        match response_rx.await {
            Ok(Ok(response)) => {
                debug!("Successfully set vacation days to {response}");
            },
            Ok(Err(e)) => {
                error!("Error setting vacation days: {e}"); 
                return Err((StatusCode::INTERNAL_SERVER_ERROR, e));
            },
            Err(e) => {
                error!("Failed to receive response: {e}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()));
            }
        }
    }

    let pow_state = params.active;
    Ok(format!("{{\"powersave\": {pow_state}}}\n"))
}

async fn get_power_save(State(tx): State<ModbusSender>) -> Result<String, (StatusCode, String)>{
    debug!("get_power_save");
    // Create a oneshot channel for THIS REQUEST
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(HEATSYSTEM_ROOM_SETTEMP), response_tx)).await.unwrap();
    
    // Wait for response on THIS request's channel
    let room_setpoint = match response_rx.await {
        Ok(Ok(rsp)) => {
            debug!("get_power_save: room_setpoint: {rsp}");
            rsp
        },
        Ok(Err(e)) => {
            error!("Error reading room setpoint: {e}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e));
        },
        Err(e) => {
            error!("Failed to receive response for room setpoint: {e}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response for room setpoint".to_string()));
        }
    };

    // Create a oneshot channel for THIS REQUEST
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(CTC_VACCATION_DAYS), response_tx)).await.unwrap();

    // Wait for response on THIS request's channel
    let vaccation_days = match response_rx.await {
        Ok(Ok(rsp)) => {
            debug!("get_power_save: vaccation_days: {rsp}");
            rsp
        },
        Ok(Err(e)) => {
            error!("Error reading vacation days: {e}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e));
        },
        Err(e) => {
            error!("Failed to receive response for vacation days: {e}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response for vacation days".to_string()));
        }
    };
    Ok(format!("{{\"room_temp_setpoint\": {room_setpoint}, \"vaccation_days\": {vaccation_days}}}\n"))
}