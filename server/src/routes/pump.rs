//! Cirkulationspump status endpoint.
//!
//! `GET /api/v1/pump` exposes the last-known on/off state of the smart plug
//! the [`SmartGrid`](crate::smartgrid) actor drives. The state comes from
//! [`HomeyPumpCache`], which is kept fresh by the reconciliation poller in
//! [`crate::homey::poller`] and by every successful actor push.
//!
//! When `[homey].enabled = false`, the route returns 503 so the dashboard
//! can hide the pump badge instead of showing stale defaults.

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Serialize;

use crate::error::ApiError;
use crate::homey::cache::HomeyPumpCache;

#[derive(Clone)]
pub struct PumpRouteState {
    pub cache: Option<Arc<HomeyPumpCache>>,
}

#[derive(Serialize)]
struct PumpResponse {
    /// Last-known plug state. `None` means we have never observed it yet.
    on: Option<bool>,
    /// `true` when the most recent Homey call failed; `on` may still be
    /// useful but is no longer authoritative.
    stale: bool,
    /// Unix seconds when the cache last received a fresh observation.
    /// Stable between observations so the dashboard's JSON-dedup skips
    /// re-renders when state is unchanged.
    last_observed_unix_secs: Option<u64>,
}

pub fn routes(cache: Option<Arc<HomeyPumpCache>>) -> Router {
    Router::new()
        .route("/api/v1/pump", get(get_pump))
        .with_state(PumpRouteState { cache })
}

async fn get_pump(State(state): State<PumpRouteState>) -> Result<impl IntoResponse, ApiError> {
    let cache = state.cache.ok_or(ApiError::ServiceUnavailable)?;
    let snap = cache.read().await;
    Ok(Json(PumpResponse {
        on: snap.actual,
        stale: snap.stale,
        last_observed_unix_secs: snap.last_observed_unix_secs,
    }))
}

#[cfg(test)]
mod tests {
    //! Handler-level tests: invoke `get_pump` directly with synthesized
    //! `State`, then inspect the `Response` parts. Avoids pulling in
    //! `tower::ServiceExt` / `http_body_util` just for these checks.

    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    async fn call(state: PumpRouteState) -> axum::response::Response {
        match get_pump(State(state)).await {
            Ok(resp) => resp.into_response(),
            Err(err) => err.into_response(),
        }
    }

    async fn read_body(resp: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn returns_503_when_disabled() {
        let resp = call(PumpRouteState { cache: None }).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn returns_empty_state_when_cache_never_written() {
        let resp = call(PumpRouteState {
            cache: Some(Arc::new(HomeyPumpCache::new())),
        })
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body["on"], serde_json::Value::Null);
        assert_eq!(body["stale"], false);
        assert_eq!(body["last_observed_unix_secs"], serde_json::Value::Null);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn returns_actual_and_freshness() {
        let cache = Arc::new(HomeyPumpCache::new());
        cache.write_fresh(true).await;
        let resp = call(PumpRouteState { cache: Some(cache) }).await;
        let body = read_body(resp).await;
        assert_eq!(body["on"], true);
        assert_eq!(body["stale"], false);
        assert!(body["last_observed_unix_secs"].is_u64());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reports_stale_after_mark() {
        let cache = Arc::new(HomeyPumpCache::new());
        cache.write_fresh(false).await;
        cache.mark_stale().await;
        let resp = call(PumpRouteState { cache: Some(cache) }).await;
        let body = read_body(resp).await;
        assert_eq!(body["on"], false);
        assert_eq!(body["stale"], true);
    }
}
