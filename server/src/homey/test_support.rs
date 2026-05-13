//! Shared in-process Homey mock for tests.
//!
//! Used by `homey::mod`, `homey::poller`, and `smartgrid::actor` so each
//! suite doesn't reimplement the same axum routes + `MockState`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, put},
};
use serde_json::json;
use tokio::net::TcpListener;

use super::HomeyClient;
use crate::config::HomeyConfig;

#[derive(Default)]
pub struct MockState {
    pub pump_on: bool,
    pub set_calls: Vec<bool>,
    pub get_calls: u32,
    pub last_set_authorization: Option<String>,
    pub last_set_body: Option<serde_json::Value>,
    pub get_returns_error: bool,
}

pub type SharedMock = Arc<Mutex<MockState>>;

async fn put_onoff(
    State(state): State<SharedMock>,
    headers: HeaderMap,
    Path((_id, _cap)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut s = state.lock().unwrap();
    s.last_set_authorization = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    s.last_set_body = Some(body.clone());
    if let Some(v) = body.get("value").and_then(serde_json::Value::as_bool) {
        s.pump_on = v;
        s.set_calls.push(v);
    }
    Json(json!({}))
}

async fn get_device(
    State(state): State<SharedMock>,
    Path(_id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let mut s = state.lock().unwrap();
    s.get_calls += 1;
    if s.get_returns_error {
        return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(Json(json!({
        "id": "dev-xyz",
        "capabilitiesObj": { "onoff": { "value": s.pump_on } }
    })))
}

/// Bind a mock Homey HTTP server on `127.0.0.1:0` and return its address.
/// The server lives as long as the spawned task; one per test.
pub async fn spawn_mock(state: SharedMock) -> SocketAddr {
    let app = Router::new()
        .route(
            "/api/manager/devices/device/{id}/capability/{cap}",
            put(put_onoff),
        )
        .route("/api/manager/devices/device/{id}", get(get_device))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// Build a `HomeyClient` pointing at the given mock address with a fixed
/// test PAT and device id.
pub fn make_client(addr: SocketAddr) -> HomeyClient {
    HomeyClient::new(&HomeyConfig {
        enabled: true,
        url: format!("http://{addr}"),
        token: Some("test-pat".into()),
        pump_device_id: "dev-xyz".into(),
        poll_interval_secs: 60,
    })
    .unwrap()
}
