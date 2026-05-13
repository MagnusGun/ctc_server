//! Step-response API endpoint.
//!
//! `GET /api/v1/heatpump/step_response?limit=N` returns the most recent
//! captured flow → return propagation events (newest first). Each event
//! includes the raw sample timeline so the dashboard can render the
//! response curve directly without server-side smoothing.

use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::storage::{StepEventBlob, Store};

#[derive(Debug, Deserialize)]
struct StepResponseQuery {
    limit: Option<usize>,
}

pub fn routes(store: Store) -> Router {
    Router::new()
        .route("/api/v1/heatpump/step_response", get(get_step_response))
        .with_state(store)
}

async fn get_step_response(
    State(store): State<Store>,
    Query(q): Query<StepResponseQuery>,
) -> Result<axum::Json<Vec<StepEventBlob>>, ApiError> {
    let limit = q.limit.unwrap_or(6).clamp(1, 50);
    Ok(axum::Json(store.recent_step_events(limit)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();
        (dir, store)
    }

    fn make_event(seed: u64) -> StepEventBlob {
        StepEventBlob {
            started_at: seed,
            flow_before: 30.0,
            flow_after: 32.0,
            return_before: 25.0,
            samples: vec![(0, 30.0, 25.0), (60, 31.0, 25.5)],
        }
    }

    #[tokio::test]
    async fn empty_store_returns_empty_array() {
        let (_dir, store) = tmp_store();
        let r = get_step_response(State(store), Query(StepResponseQuery { limit: None }))
            .await
            .expect("get_step_response");
        assert!(r.0.is_empty());
    }

    #[tokio::test]
    async fn default_limit_is_six() {
        let (_dir, store) = tmp_store();
        for i in 0..10 {
            store.record_step_event(make_event(i));
        }
        let r = get_step_response(State(store), Query(StepResponseQuery { limit: None }))
            .await
            .expect("get_step_response");
        assert_eq!(r.0.len(), 6, "default limit should return 6 newest events");
    }

    #[tokio::test]
    async fn limit_zero_is_clamped_to_one() {
        let (_dir, store) = tmp_store();
        for i in 0..3 {
            store.record_step_event(make_event(i));
        }
        let r = get_step_response(State(store), Query(StepResponseQuery { limit: Some(0) }))
            .await
            .expect("get_step_response");
        assert_eq!(r.0.len(), 1, "limit=0 should clamp to 1");
    }

    #[tokio::test]
    async fn limit_exceeds_available_returns_all_available() {
        let (_dir, store) = tmp_store();
        for i in 0..3 {
            store.record_step_event(make_event(i));
        }
        let r = get_step_response(State(store), Query(StepResponseQuery { limit: Some(10) }))
            .await
            .expect("get_step_response");
        assert_eq!(r.0.len(), 3);
    }

    #[tokio::test]
    async fn limit_huge_is_clamped_to_fifty() {
        let (_dir, store) = tmp_store();
        // Seed 60 events; cache caps at MAX_STEP_EVENTS (50) so we already
        // have fewer than 60 retained — but the clamp still applies.
        for i in 0..60 {
            store.record_step_event(make_event(i));
        }
        let r = get_step_response(
            State(store),
            Query(StepResponseQuery {
                limit: Some(1_000_000),
            }),
        )
        .await
        .expect("get_step_response");
        assert!(r.0.len() <= 50, "limit should be clamped to <= 50");
    }
}
