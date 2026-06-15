//! Domestic-hot-water HTTP API.
//!
//! Endpoints (Task 10):
//! * `GET    /api/v1/dhw/state`                              — snapshot
//! * `POST   /api/v1/dhw/comfort?level=economy|normal|komfort` — set comfort
//! * `POST   /api/v1/dhw/boost?preset=shower`                — UC-A boost
//! * `POST   /api/v1/dhw/boost?preset=bath&hours=N`          — UC-B boost
//! * `DELETE /api/v1/dhw/boost`                              — cancel/no-op
//!
//! All side effects are forwarded to the [`DhwActor`](crate::dhw::DhwActor)
//! over an mpsc channel; the routes themselves are stateless apart from a
//! cloned [`DhwHandle`]. The router is mounted by `main.rs` once the
//! actor is up. Tests here drive a stand-alone `DhwHandle` backed by a
//! lightweight inline actor task.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::Deserialize;

use crate::dhw::DhwHandle;
use crate::dhw::error::{ComfortLevel, DhwError, StartReport};
use crate::dhw::state::DhwSnapshot;

#[derive(Clone)]
pub struct DhwRouterState {
    pub handle: DhwHandle,
}

pub fn routes(state: DhwRouterState) -> Router {
    Router::new()
        .route("/api/v1/dhw/state", get(get_state))
        .route("/api/v1/dhw/comfort", post(set_comfort))
        .route("/api/v1/dhw/boost", post(start_boost))
        .route("/api/v1/dhw/boost", delete(cancel_boost))
        .with_state(state)
}

async fn get_state(State(s): State<DhwRouterState>) -> Json<DhwSnapshot> {
    Json(s.handle.snapshot().await)
}

#[derive(Deserialize)]
struct ComfortQ {
    level: String,
}

async fn set_comfort(
    State(s): State<DhwRouterState>,
    Query(q): Query<ComfortQ>,
) -> Result<StatusCode, axum::response::Response> {
    let level = ComfortLevel::from_query(&q.level).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "level must be economy|normal|komfort",
        )
            .into_response()
    })?;
    s.handle.set_comfort(level).await.map_err(into_resp)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct BoostQ {
    preset: String,
    hours: Option<f32>,
}

async fn start_boost(
    State(s): State<DhwRouterState>,
    Query(q): Query<BoostQ>,
) -> Result<Json<StartReport>, axum::response::Response> {
    let result = match q.preset.as_str() {
        "shower" => s.handle.start_shower().await,
        "bath" => {
            let hours = q.hours.ok_or_else(|| {
                (StatusCode::BAD_REQUEST, "hours required for bath").into_response()
            })?;
            s.handle.start_bath(hours).await
        }
        _ => {
            return Err((StatusCode::BAD_REQUEST, "preset must be shower|bath").into_response());
        }
    };
    result.map(Json).map_err(into_resp)
}

async fn cancel_boost(
    State(s): State<DhwRouterState>,
) -> Result<StatusCode, axum::response::Response> {
    s.handle.cancel().await.map_err(into_resp)?;
    Ok(StatusCode::NO_CONTENT)
}

fn into_resp(e: DhwError) -> axum::response::Response {
    let (code, body) = e.into_response();
    (code, body).into_response()
}

#[cfg(test)]
mod tests {
    //! Stand-alone route tests. We don't spin up a full `DhwActor` (that
    //! would require Modbus + SG fakes, a `Store`, a `PriceState`, etc.).
    //! Instead we wrap a fresh mpsc sender in a `DhwHandle` via the
    //! test-only `DhwHandle::from_sender`, and run a tiny inline task that
    //! responds to the four `DhwCmd` variants the routes exercise.

    use super::*;
    use crate::dhw::actor::DhwCmd;
    use crate::dhw::error::ComfortLevel;
    use crate::dhw::state::{BoostPreset, DhwBoostSnapshot, DhwSnapshot};
    use axum::body::to_bytes;
    use axum::extract::Query;
    use tokio::sync::mpsc;

    /// Spawn a lightweight inline "actor" that replies to commands using
    /// the closure-driven `outcomes` description. Returns a `DhwHandle`
    /// wired to it.
    fn spawn_fake(
        snap: DhwSnapshot,
        comfort_outcome: Result<(), DhwError>,
        shower_outcome: Result<StartReport, DhwError>,
        bath_outcome: Result<StartReport, DhwError>,
        cancel_outcome: Result<bool, DhwError>,
    ) -> DhwHandle {
        let (tx, mut rx) = mpsc::channel::<DhwCmd>(8);
        tokio::spawn(async move {
            // The fake responds once per outcome and then keeps the channel
            // open by ignoring further messages until the sender is dropped.
            let mut snap = Some(snap);
            let mut comfort = Some(comfort_outcome);
            let mut shower = Some(shower_outcome);
            let mut bath = Some(bath_outcome);
            let mut cancel = Some(cancel_outcome);
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    DhwCmd::Snapshot { respond_to } => {
                        let s = snap.clone().expect("snapshot already consumed");
                        snap = Some(s.clone());
                        let _ = respond_to.send(s);
                    }
                    DhwCmd::SetComfort { respond_to, .. } => {
                        let r = comfort.take().expect("comfort already consumed");
                        let _ = respond_to.send(r);
                    }
                    DhwCmd::StartShower { respond_to } => {
                        let r = shower.take().expect("shower already consumed");
                        let _ = respond_to.send(r);
                    }
                    DhwCmd::StartBath { respond_to, .. } => {
                        let r = bath.take().expect("bath already consumed");
                        let _ = respond_to.send(r);
                    }
                    DhwCmd::Cancel { respond_to } => {
                        let r = cancel.take().expect("cancel already consumed");
                        let _ = respond_to.send(r);
                    }
                    DhwCmd::ShutdownSave { respond_to } => {
                        let _ = respond_to.send(Ok(()));
                    }
                }
            }
        });
        DhwHandle::from_sender(tx)
    }

    fn sample_snapshot() -> DhwSnapshot {
        DhwSnapshot {
            comfort_level: ComfortLevel::Normal,
            boost: Some(DhwBoostSnapshot {
                preset: BoostPreset::Shower,
                started_at: chrono::Utc::now(),
                scheduled_end: chrono::Utc::now() + chrono::Duration::minutes(30),
                elapsed_s: 0,
                remaining_s: 1800,
                immersion_engaged: false,
            }),
        }
    }

    async fn read_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn get_state_returns_snapshot() {
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Ok(StartReport::AlreadyAtTarget {
                dhw_c: 0.0,
                target_c: 0.0,
            }),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let json = get_state(State(state)).await;
        let v = serde_json::to_value(&json.0).unwrap();
        assert_eq!(v["comfort_level"], "normal");
        assert!(v["boost"].is_object());
    }

    #[tokio::test]
    async fn post_comfort_normal_returns_204() {
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Ok(StartReport::AlreadyAtTarget {
                dhw_c: 0.0,
                target_c: 0.0,
            }),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = set_comfort(
            State(state),
            Query(ComfortQ {
                level: "normal".into(),
            }),
        )
        .await;
        match result {
            Ok(code) => assert_eq!(code, StatusCode::NO_CONTENT),
            Err(resp) => panic!("expected 204, got {}", resp.status()),
        }
    }

    #[tokio::test]
    async fn post_comfort_invalid_level_returns_400() {
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Ok(StartReport::AlreadyAtTarget {
                dhw_c: 0.0,
                target_c: 0.0,
            }),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = set_comfort(
            State(state),
            Query(ComfortQ {
                level: "wat".into(),
            }),
        )
        .await;
        match result {
            Err(resp) => assert_eq!(resp.status(), StatusCode::BAD_REQUEST),
            Ok(code) => panic!("expected 400, got {code}"),
        }
    }

    #[tokio::test]
    async fn post_boost_shower_returns_started_or_already_at_target() {
        let scheduled_end = chrono::Utc::now() + chrono::Duration::minutes(30);
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Ok(StartReport::Started { scheduled_end }),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = start_boost(
            State(state),
            Query(BoostQ {
                preset: "shower".into(),
                hours: None,
            }),
        )
        .await;
        match result {
            Ok(Json(report)) => {
                let v = serde_json::to_value(&report).unwrap();
                assert_eq!(v["outcome"], "started");
            }
            Err(resp) => panic!("expected 200, got {}", resp.status()),
        }
    }

    #[tokio::test]
    async fn post_boost_bath_missing_hours_returns_400() {
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Ok(StartReport::AlreadyAtTarget {
                dhw_c: 0.0,
                target_c: 0.0,
            }),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = start_boost(
            State(state),
            Query(BoostQ {
                preset: "bath".into(),
                hours: None,
            }),
        )
        .await;
        match result {
            Err(resp) => assert_eq!(resp.status(), StatusCode::BAD_REQUEST),
            Ok(_) => panic!("expected 400 for missing hours"),
        }
    }

    #[tokio::test]
    async fn post_boost_unknown_preset_returns_400() {
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Ok(StartReport::AlreadyAtTarget {
                dhw_c: 0.0,
                target_c: 0.0,
            }),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = start_boost(
            State(state),
            Query(BoostQ {
                preset: "sauna".into(),
                hours: None,
            }),
        )
        .await;
        match result {
            Err(resp) => assert_eq!(resp.status(), StatusCode::BAD_REQUEST),
            Ok(_) => panic!("expected 400 for unknown preset"),
        }
    }

    #[tokio::test]
    async fn delete_boost_returns_204_when_nothing_active() {
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Ok(StartReport::AlreadyAtTarget {
                dhw_c: 0.0,
                target_c: 0.0,
            }),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = cancel_boost(State(state)).await;
        match result {
            Ok(code) => assert_eq!(code, StatusCode::NO_CONTENT),
            Err(resp) => panic!("expected 204, got {}", resp.status()),
        }
    }

    #[tokio::test]
    async fn post_boost_bath_with_hours_returns_started() {
        // The "bath" arm with valid hours reaches start_bath and maps the
        // success report to JSON — a path the shower/missing-hours tests skip.
        let scheduled_end = chrono::Utc::now() + chrono::Duration::hours(2);
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Err(DhwError::Modbus("unused".into())),
            Ok(StartReport::Started { scheduled_end }),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = start_boost(
            State(state),
            Query(BoostQ {
                preset: "bath".into(),
                hours: Some(2.0),
            }),
        )
        .await;
        match result {
            Ok(Json(report)) => {
                let v = serde_json::to_value(&report).unwrap();
                assert_eq!(v["outcome"], "started");
            }
            Err(resp) => panic!("expected 200, got {}", resp.status()),
        }
    }

    #[tokio::test]
    async fn post_comfort_actor_error_maps_to_response() {
        // set_comfort with a valid level but a failing actor must propagate
        // the DhwError through into_resp rather than returning 204.
        let handle = spawn_fake(
            sample_snapshot(),
            Err(DhwError::Modbus("write failed".into())),
            Ok(StartReport::AlreadyAtTarget {
                dhw_c: 0.0,
                target_c: 0.0,
            }),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = set_comfort(
            State(state),
            Query(ComfortQ {
                level: "komfort".into(),
            }),
        )
        .await;
        match result {
            Err(resp) => assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR),
            Ok(code) => panic!("expected error response, got {code}"),
        }
    }

    #[tokio::test]
    async fn post_boost_shower_actor_error_maps_to_response() {
        // A failing shower start must flow through into_resp (the err arm of
        // result.map(Json).map_err(into_resp)).
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Err(DhwError::Modbus("read failed".into())),
            Err(DhwError::Modbus("unused".into())),
            Ok(false),
        );
        let state = DhwRouterState { handle };
        let result = start_boost(
            State(state),
            Query(BoostQ {
                preset: "shower".into(),
                hours: None,
            }),
        )
        .await;
        match result {
            Err(resp) => assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR),
            Ok(_) => panic!("expected error response from failing shower start"),
        }
    }

    #[tokio::test]
    async fn delete_boost_returns_409_for_active_shower() {
        let handle = spawn_fake(
            sample_snapshot(),
            Ok(()),
            Ok(StartReport::AlreadyAtTarget {
                dhw_c: 0.0,
                target_c: 0.0,
            }),
            Err(DhwError::Modbus("unused".into())),
            Err(DhwError::ShowerCannotBeCancelled),
        );
        let state = DhwRouterState { handle };
        let result = cancel_boost(State(state)).await;
        match result {
            Err(resp) => {
                assert_eq!(resp.status(), StatusCode::CONFLICT);
                let body = read_json(resp).await;
                assert_eq!(body["error"], "shower_runs_to_completion");
            }
            Ok(code) => panic!("expected 409, got {code}"),
        }
    }
}
