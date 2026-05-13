//! Trend series endpoint.
//!
//! `GET /api/v1/heatpump/series?sensor=<slug>&hours=<N>` returns an array of
//! `{t, v}` pairs from the in-memory ring inside [`Store`]. Used by the
//! dashboard's trend modal (Step 8) and the stats-modal charts (Step 10).

use crate::error::ApiError;
use crate::routes::series_window;
use crate::storage::{Sensor, Store};

#[cfg(test)]
use std::time::SystemTime;
use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use serde::{Deserialize, Serialize};

pub fn routes(store: Store) -> Router {
    Router::new()
        .route("/api/v1/heatpump/series", get(get_series))
        .with_state(store)
}

#[derive(Debug, Deserialize)]
struct SeriesQuery {
    sensor: String,
    hours: Option<u32>,
}

#[derive(Debug, Serialize)]
struct SeriesPoint {
    t: i64,
    v: f32,
}

async fn get_series(
    State(store): State<Store>,
    Query(q): Query<SeriesQuery>,
) -> Result<axum::Json<Vec<SeriesPoint>>, ApiError> {
    let sensor = Sensor::from_slug(&q.sensor).ok_or_else(|| {
        // The response body stays empty (consistent with other BadRequest
        // mappings, which keep internals hidden), but log the offending slug
        // so operators can diagnose typos quickly.
        tracing::warn!("get_series: unknown sensor slug \"{}\"", q.sensor);
        ApiError::BadRequest
    })?;
    let hours = q.hours.unwrap_or(24).clamp(1, 24);
    let (from, _, to) = series_window(hours)?;

    let points = store
        .series_range(sensor, from, to)
        .into_iter()
        .map(|(t, v)| SeriesPoint { t, v })
        .collect();
    Ok(axum::Json(points))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Query, State};

    fn tmp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn unknown_slug_returns_bad_request() {
        let (_dir, store) = tmp_store();
        let q = Query(SeriesQuery {
            sensor: "nope".into(),
            hours: None,
        });
        let r = get_series(State(store), q).await;
        assert!(matches!(r, Err(ApiError::BadRequest)));
    }

    #[tokio::test]
    async fn known_slug_with_no_samples_returns_empty() {
        let (_dir, store) = tmp_store();
        let q = Query(SeriesQuery {
            sensor: "room".into(),
            hours: None,
        });
        let r = get_series(State(store), q).await.unwrap();
        assert!(r.0.is_empty());
    }

    #[tokio::test]
    async fn returns_recorded_samples() {
        let (_dir, store) = tmp_store();
        let now = SystemTime::now();
        store.record_sample(Sensor::Room, now, 21.5).unwrap();
        let q = Query(SeriesQuery {
            sensor: "room".into(),
            hours: Some(1),
        });
        let r = get_series(State(store), q).await.unwrap();
        assert_eq!(r.0.len(), 1);
        assert!((r.0[0].v - 21.5).abs() < f32::EPSILON);
    }
}
