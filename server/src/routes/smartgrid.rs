//! `SmartGrid` control API endpoints
//!
//! These endpoints provide control over the `SmartGrid` functionality using GPIO relays.
//! `SmartGrid` modes are set by controlling the K24 (Smart A) and K25 (Smart B) GPIO pins
//! which correspond to the CTC heat pump's external smart grid input terminals.
//!
//! All side effects are dispatched through [`SmartGridHandle`], an mpsc
//! sender backed by the [`crate::smartgrid::actor`] task. Commands are
//! processed serially in that task, so concurrent POSTs cannot interleave
//! the bump → cancel → set → schedule sequence.

use std::str::FromStr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};

use std::time::{Duration, SystemTime};

use crate::config::SmartGridConfig;
use crate::dhw::actor::ModbusWriter;
use crate::energy::price::PriceState;
use crate::energy::tibber::parse_iso8601;
use crate::error::ApiError;
use crate::smartgrid::heatup::{REG_DHW_UPPER, WarmByCommand, estimate_heatup};
use crate::smartgrid::{SmartGridError, SmartGridHandle, SmartGridMode};

/// State for `SmartGrid` routes.
#[derive(Clone)]
pub struct SmartGridState {
    handle: Option<SmartGridHandle>,
    price_state: PriceState,
    config: SmartGridConfig,
    /// Local timezone for parsing `HH:MM` warm-by deadlines.
    tz: chrono_tz::Tz,
    /// Scaled Modbus reader for the warm-by schedule-time temp read + preview.
    /// `None` only in tests that don't exercise warm-by.
    modbus: Option<Arc<dyn ModbusWriter>>,
}

pub fn routes(
    handle: Option<SmartGridHandle>,
    price_state: PriceState,
    config: SmartGridConfig,
    _request_timeout_secs: u64,
    tz: chrono_tz::Tz,
    modbus: Arc<dyn ModbusWriter>,
) -> Router {
    let state = SmartGridState {
        handle,
        price_state,
        config,
        tz,
        modbus: Some(modbus),
    };

    Router::new()
        .route("/api/v1/smartgrid", get(get_smartgrid))
        .route("/api/v1/smartgrid", post(set_smartgrid))
        .route(
            "/api/v1/smartgrid/proposed_resume",
            get(get_proposed_resume),
        )
        .route(
            "/api/v1/smartgrid/resume_candidates",
            get(get_resume_candidates),
        )
        .route(
            "/api/v1/smartgrid/scheduled_resume",
            delete(delete_scheduled_resume),
        )
        .route("/api/v1/smartgrid/warm_by", post(set_warm_by))
        .route(
            "/api/v1/smartgrid/warm_by_preview",
            get(get_warm_by_preview),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct SmartGridQuery {
    mode: String,
    #[serde(default)]
    schedule_resume: bool,
    /// Optional explicit resume instant (ISO 8601). When present with
    /// `schedule_resume=true`, schedules the auto-flip at exactly this time
    /// instead of the cheapest-run auto-pick. Must be in the future.
    #[serde(default)]
    resume_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct SmartGridResponse {
    smartgrid_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    changed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduled_resume_at: Option<String>,
    /// Configured run length the Blocking-resume scheduler aims for.
    /// Exposed so the dashboard can render an overlay band on the price
    /// chart whose width matches the scheduled run.
    run_minutes: u16,
}

#[derive(Debug, Serialize)]
struct SetSmartGridResponse {
    smartgrid_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheduled_resume_at: Option<String>,
    run_minutes: u16,
}

#[derive(Debug, Serialize)]
struct ProposedResumeResponse {
    starts_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spot_sek: Option<f64>,
    window_hours: u64,
    /// Length of the contiguous run whose start the scheduler chose.
    run_minutes: u16,
}

#[derive(Debug, Serialize)]
struct ResumeCandidatesResponse {
    candidates: Vec<crate::energy::price::ResumeCandidate>,
}

/// Set `SmartGrid` mode via GPIO.
///
/// `POST /api/v1/smartgrid?mode=blocking&schedule_resume=true`
///
/// Valid modes: normal, blocking, lowprice, overcapacity
///
/// When `mode=blocking&schedule_resume=true`, the server picks the start of
/// the cheapest contiguous `auto_resume_min_duration_minutes` run inside
/// the configured horizon and schedules an automatic flip back to Normal
/// at that time. See `compute_resume_target` in `smartgrid::actor` for the
/// full per-mode logic.
async fn set_smartgrid(
    State(state): State<SmartGridState>,
    Query(query): Query<SmartGridQuery>,
) -> Result<String, ApiError> {
    debug!(
        "set_smartgrid: START - mode={} schedule_resume={}",
        query.mode, query.schedule_resume
    );

    let handle = state.handle.as_ref().ok_or_else(|| {
        error!("set_smartgrid: GPIO not available - SmartGrid control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    let mode = SmartGridMode::from_str(&query.mode).map_err(|e| {
        error!("set_smartgrid: Invalid mode '{}': {}", query.mode, e);
        ApiError::BadRequest
    })?;

    let resume_at = match query.resume_at.as_deref() {
        Some(s) => {
            let parsed = DateTime::parse_from_rfc3339(s).map_err(|e| {
                error!("set_smartgrid: bad resume_at '{s}': {e}");
                ApiError::BadRequest
            })?;
            let when = std::time::SystemTime::from(parsed.with_timezone(&Utc));
            // Allow a 60s grace: the cheapest slot can start at the next
            // 15-min boundary, only seconds out, so dialog dwell-time must not
            // turn a valid pick into a 400. A just-passed pick schedules an
            // effectively-immediate resume (the actor treats a non-future
            // fires_at as fire-now). Reject only clearly-stale timestamps.
            let grace = std::time::Duration::from_mins(1);
            if when + grace <= std::time::SystemTime::now() {
                error!("set_smartgrid: resume_at '{s}' is more than 60s in the past");
                return Err(ApiError::BadRequest);
            }
            Some(when)
        }
        None => None,
    };

    let fires_at = handle
        .set_mode(mode, query.schedule_resume, resume_at)
        .await
        .map_err(map_smartgrid_error)?;

    let response = SetSmartGridResponse {
        smartgrid_mode: mode.to_string(),
        scheduled_resume_at: fires_at.map(format_system_time),
        run_minutes: state.config.auto_resume_min_duration_minutes,
    };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("set_smartgrid: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Get current `SmartGrid` mode from GPIO.
///
/// GET /api/v1/smartgrid
async fn get_smartgrid(State(state): State<SmartGridState>) -> Result<String, ApiError> {
    debug!("get_smartgrid: START");

    let handle = state.handle.as_ref().ok_or_else(|| {
        error!("get_smartgrid: GPIO not available - SmartGrid control requires GPIO");
        ApiError::ServiceUnavailable
    })?;

    let mode = handle.read_mode().await.map_err(map_smartgrid_error)?;
    let changed_at = handle
        .mode_changed_at()
        .await
        .map_err(map_smartgrid_error)?;
    let scheduled_resume_at = handle
        .scheduled_resume_at()
        .await
        .map_err(map_smartgrid_error)?;

    let response = SmartGridResponse {
        smartgrid_mode: mode.to_string(),
        changed_at: changed_at.map(format_system_time),
        scheduled_resume_at: scheduled_resume_at.map(format_system_time),
        run_minutes: state.config.auto_resume_min_duration_minutes,
    };

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("get_smartgrid: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Preview the slot the Blocking-resume scheduler *would* pick if asked,
/// without performing any side effect. Drives the dashboard confirmation
/// dialog and chart-overlay placement.
///
/// Uses the same `cheapest_run_within` → `cheapest_within` fallback chain
/// the actor uses, so the preview matches the actual schedule. `spot_sek`
/// is the price of the run-start slot itself (not the run's weighted
/// average) — kept for backwards compatibility with the existing dialog.
async fn get_proposed_resume(State(state): State<SmartGridState>) -> Result<String, ApiError> {
    let window =
        std::time::Duration::from_secs(state.config.auto_resume_window_hours.saturating_mul(3600));
    let run_duration = std::time::Duration::from_secs(
        u64::from(state.config.auto_resume_min_duration_minutes).saturating_mul(60),
    );
    let slot = state
        .price_state
        .cheapest_run_within(window, run_duration)
        .or_else(|| state.price_state.cheapest_within(window));

    let response = ProposedResumeResponse {
        starts_at: slot.as_ref().map(|p| p.starts_at.clone()),
        spot_sek: slot.as_ref().map(|p| p.spot_sek),
        window_hours: state.config.auto_resume_window_hours,
        run_minutes: state.config.auto_resume_min_duration_minutes,
    };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("get_proposed_resume: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Ranked, non-overlapping cheap resume runs in the configured window.
/// Read-only; drives the dashboard resume-slot picker. Empty array (200)
/// when no price data — an empty list is a valid "nothing to pick" state.
///
/// `GET /api/v1/smartgrid/resume_candidates`
async fn get_resume_candidates(State(state): State<SmartGridState>) -> Result<String, ApiError> {
    let window =
        std::time::Duration::from_secs(state.config.auto_resume_window_hours.saturating_mul(3600));
    let run_duration = std::time::Duration::from_secs(
        u64::from(state.config.auto_resume_min_duration_minutes).saturating_mul(60),
    );
    let candidates = state
        .price_state
        .cheapest_runs_within(window, run_duration, 6);

    let response = ResumeCandidatesResponse { candidates };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("get_resume_candidates: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Cancel any pending auto-resume without changing the current `SmartGrid` mode.
///
/// Idempotent — calling this when no schedule exists returns `204 No Content`.
async fn delete_scheduled_resume(
    State(state): State<SmartGridState>,
) -> Result<StatusCode, ApiError> {
    debug!("delete_scheduled_resume: START");

    let handle = state.handle.as_ref().ok_or_else(|| {
        error!("delete_scheduled_resume: GPIO not available");
        ApiError::ServiceUnavailable
    })?;

    handle
        .cancel_scheduled_resume()
        .await
        .map_err(map_smartgrid_error)?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct WarmByQuery {
    /// Deadline in `HH:MM` local wall-clock (e.g. `06:30`).
    warm_by: String,
    /// Optional target tank-top temperature; defaults to the configured value.
    #[serde(default)]
    target_c: Option<f32>,
}

#[derive(Debug, Serialize)]
struct SetWarmByResponse {
    smartgrid_mode: String,
    applied_blocking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    heatup_start_at: Option<String>,
    skipped: bool,
    target_c: f32,
}

#[derive(Debug, Serialize)]
struct WarmByPreviewResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    heatup_start_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    est_minutes: Option<u64>,
    would_skip: bool,
}

/// The temp-aware sizing result shared by the POST and preview handlers.
struct WarmByPlan {
    heatup_start: Option<SystemTime>,
    skipped: bool,
    est_minutes: Option<u64>,
}

/// Parse `target_c` from the query (or fall back to the configured default),
/// rejecting anything outside the accepted shower range.
fn resolve_target_c(query: Option<f32>, config: &SmartGridConfig) -> Result<f32, ApiError> {
    let target = query.unwrap_or(config.warm_by_target_temp_c);
    if (45.0..=50.0).contains(&target) {
        Ok(target)
    } else {
        error!("warm_by: target_c {target} outside accepted range [45, 50]");
        Err(ApiError::BadRequest)
    }
}

/// Resolve an `HH:MM` local-time string to the next future instant at that
/// wall-clock time. If the time has already passed today, it rolls to
/// tomorrow. DST-safe via `from_local_datetime().earliest()`. `now_utc` is a
/// parameter so the rollover logic is unit-testable.
fn next_deadline_from_hh_mm(
    s: &str,
    tz: chrono_tz::Tz,
    now_utc: DateTime<Utc>,
) -> Option<SystemTime> {
    let (h_str, m_str) = s.split_once(':')?;
    let hour: u32 = h_str.parse().ok()?;
    let minute: u32 = m_str.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    let now_local = now_utc.with_timezone(&tz);
    let today = now_local.date_naive();
    let at = |date: chrono::NaiveDate| -> Option<DateTime<chrono_tz::Tz>> {
        tz.from_local_datetime(&date.and_hms_opt(hour, minute, 0)?)
            .earliest()
    };
    let candidate = match at(today) {
        Some(dt) if dt > now_local => dt,
        _ => at(today.succ_opt()?)?,
    };
    Some(SystemTime::from(candidate.with_timezone(&Utc)))
}

/// Read the current tank temperature and size (or skip) the heat-up window,
/// picking the cheapest run that finishes by `deadline`. Shared by the POST
/// handler and the read-only preview.
async fn plan_warm_by(
    state: &SmartGridState,
    deadline: SystemTime,
    target_c: f32,
) -> Result<WarmByPlan, ApiError> {
    let modbus = state.modbus.as_ref().ok_or(ApiError::ServiceUnavailable)?;
    let current_c = modbus.read_scaled(REG_DHW_UPPER).await.map_err(|e| {
        error!("warm_by: failed to read tank temp: {e}");
        ApiError::InternalError
    })?;

    let max_lead = Duration::from_mins(u64::from(state.config.warm_by_max_lead_minutes));
    let max_duration = Duration::from_mins(u64::from(state.config.warm_by_max_duration_minutes));
    let rate = state.config.warm_by_heat_rate_c_per_min;

    match estimate_heatup(current_c, target_c, rate, max_duration) {
        // Already warm — block only, no heat-up.
        None => Ok(WarmByPlan {
            heatup_start: None,
            skipped: true,
            est_minutes: None,
        }),
        Some(dur) => {
            // Cheapest run finishing by the deadline within the lead window;
            // if none fits, start immediately so the tank still warms.
            let start = state
                .price_state
                .cheapest_run_ending_by(deadline, max_lead, dur)
                .and_then(|p| parse_iso8601(&p.starts_at).ok())
                .unwrap_or_else(SystemTime::now);
            Ok(WarmByPlan {
                heatup_start: Some(start),
                skipped: false,
                est_minutes: Some(dur.as_secs() / 60),
            })
        }
    }
}

/// Apply Blocking and schedule a warm-by heat-up.
///
/// `POST /api/v1/smartgrid/warm_by?warm_by=06:30&target_c=48`
async fn set_warm_by(
    State(state): State<SmartGridState>,
    Query(query): Query<WarmByQuery>,
) -> Result<String, ApiError> {
    let handle = state.handle.as_ref().ok_or(ApiError::ServiceUnavailable)?;
    if !state.config.warm_by_enabled {
        error!("warm_by: feature disabled in config");
        return Err(ApiError::ServiceUnavailable);
    }
    let target_c = resolve_target_c(query.target_c, &state.config)?;
    let deadline =
        next_deadline_from_hh_mm(&query.warm_by, state.tz, Utc::now()).ok_or_else(|| {
            error!("warm_by: bad HH:MM deadline '{}'", query.warm_by);
            ApiError::BadRequest
        })?;

    let plan = plan_warm_by(&state, deadline, target_c).await?;
    let cmd = WarmByCommand {
        heatup_start: plan.heatup_start,
        target_c,
        max_duration: Duration::from_mins(u64::from(state.config.warm_by_max_duration_minutes)),
    };
    let scheduled = handle
        .schedule_warm_by(cmd)
        .await
        .map_err(map_smartgrid_error)?;

    let response = SetWarmByResponse {
        smartgrid_mode: SmartGridMode::Blocking.to_string(),
        applied_blocking: true,
        heatup_start_at: scheduled.map(format_system_time),
        skipped: plan.skipped,
        target_c,
    };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("set_warm_by: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

/// Preview what a warm-by request would do without any side effect.
///
/// `GET /api/v1/smartgrid/warm_by_preview?warm_by=06:30`
async fn get_warm_by_preview(
    State(state): State<SmartGridState>,
    Query(query): Query<WarmByQuery>,
) -> Result<String, ApiError> {
    let target_c = resolve_target_c(query.target_c, &state.config)?;
    let deadline =
        next_deadline_from_hh_mm(&query.warm_by, state.tz, Utc::now()).ok_or_else(|| {
            error!("warm_by_preview: bad HH:MM deadline '{}'", query.warm_by);
            ApiError::BadRequest
        })?;

    let plan = plan_warm_by(&state, deadline, target_c).await?;
    let response = WarmByPreviewResponse {
        heatup_start_at: plan.heatup_start.map(format_system_time),
        est_minutes: plan.est_minutes,
        would_skip: plan.skipped,
    };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("get_warm_by_preview: JSON serialization error - {e}");
            ApiError::InternalError
        })
}

fn map_smartgrid_error(err: SmartGridError) -> ApiError {
    match err {
        SmartGridError::ActorGone => {
            error!("SmartGrid: actor unavailable");
            ApiError::ServiceUnavailable
        }
        e @ (SmartGridError::Apply(_) | SmartGridError::Internal(_)) => {
            error!("SmartGrid: {e}");
            ApiError::InternalError
        }
    }
}

fn format_system_time(t: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smartgrid::actor::test_support::spawn_with_test_gpio;
    use tokio_util::sync::CancellationToken;

    fn assert_float_eq(a: f64, b: f64, msg: &str) {
        assert!((a - b).abs() < 0.0001, "{msg}: expected {b}, got {a}");
    }

    fn test_config() -> SmartGridConfig {
        SmartGridConfig::default()
    }

    fn test_state(handle: Option<SmartGridHandle>, price_state: PriceState) -> SmartGridState {
        SmartGridState {
            handle,
            price_state,
            config: test_config(),
            tz: chrono_tz::Europe::Stockholm,
            modbus: None,
        }
    }

    fn create_state_without_handle() -> SmartGridState {
        test_state(None, PriceState::new("SE3".to_string()))
    }

    #[tokio::test]
    async fn test_get_smartgrid_no_gpio() {
        let state = create_state_without_handle();
        let result = get_smartgrid(State(state)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_set_smartgrid_no_gpio() {
        let state = create_state_without_handle();
        let query = SmartGridQuery {
            mode: "blocking".to_string(),
            schedule_resume: false,
            resume_at: None,
        };
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_set_smartgrid_invalid_mode() {
        let state = create_state_without_handle();
        let query = SmartGridQuery {
            mode: "invalid_mode".to_string(),
            schedule_resume: false,
            resume_at: None,
        };
        // With no GPIO, we get ServiceUnavailable before mode validation.
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_proposed_resume_no_prices() {
        let state = create_state_without_handle();
        let result = get_proposed_resume(State(state)).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(result.trim()).unwrap();
        assert!(parsed["starts_at"].is_null());
        assert_eq!(parsed["window_hours"], 12);
        assert_eq!(parsed["run_minutes"], 30);
    }

    /// The preview endpoint must use the same `cheapest_run_within` →
    /// `cheapest_within` fallback as the actor, so the dashboard's
    /// confirmation dialog and chart overlay match what will actually be
    /// scheduled.
    #[tokio::test]
    async fn test_proposed_resume_picks_contiguous_run() {
        let price_state = PriceState::new("SE3".to_string());
        // Contiguous 30-min run starting in 60 min at avg 0.10.
        let base = chrono::Utc::now() + chrono::Duration::minutes(60);
        let s1_end = base + chrono::Duration::minutes(15);
        let s2_end = s1_end + chrono::Duration::minutes(15);
        let prices = vec![
            crate::energy::price::PricePoint::from_spot(
                base.to_rfc3339(),
                s1_end.to_rfc3339(),
                0.10,
                0.0,
                0.0,
            ),
            crate::energy::price::PricePoint::from_spot(
                s1_end.to_rfc3339(),
                s2_end.to_rfc3339(),
                0.10,
                0.0,
                0.0,
            ),
        ];
        // Plus an isolated cheaper single slot — must NOT win because it
        // can't anchor a 30-min run.
        let isolated_start = chrono::Utc::now() + chrono::Duration::minutes(180);
        let isolated_end = isolated_start + chrono::Duration::minutes(15);
        let isolated = crate::energy::price::PricePoint::from_spot(
            isolated_start.to_rfc3339(),
            isolated_end.to_rfc3339(),
            0.02,
            0.0,
            0.0,
        );
        let mut today = prices;
        today.push(isolated);
        price_state.update_prices(today, vec![]);

        let state = test_state(None, price_state);
        let result = get_proposed_resume(State(state)).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(result.trim()).unwrap();
        assert_eq!(parsed["run_minutes"], 30);
        let spot = parsed["spot_sek"].as_f64().expect("spot_sek present");
        assert_float_eq(
            spot,
            0.10,
            "preview returns run-start, not isolated 0.02 slot",
        );
    }

    #[tokio::test]
    async fn test_resume_candidates_returns_ranked_runs() {
        let price_state = PriceState::new("SE3".to_string());
        let mut today = crate::energy::price::test_support::make_run(60, &[0.30, 0.30]);
        today.extend(crate::energy::price::test_support::make_run(
            180,
            &[0.10, 0.10],
        ));
        price_state.update_prices(today, vec![]);
        let state = test_state(None, price_state);

        let result = get_resume_candidates(State(state)).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(result.trim()).unwrap();
        let cands = parsed["candidates"].as_array().expect("candidates array");
        assert_eq!(cands.len(), 2);
        assert_float_eq(
            cands[0]["avg_spot_sek"].as_f64().unwrap(),
            0.10,
            "cheapest first",
        );
    }

    #[tokio::test]
    async fn test_resume_candidates_empty_without_prices() {
        let state = create_state_without_handle();
        let result = get_resume_candidates(State(state)).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(result.trim()).unwrap();
        assert_eq!(parsed["candidates"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_set_smartgrid_past_resume_at_is_bad_request() {
        let cancel = CancellationToken::new();
        let (handle, _join) =
            spawn_with_test_gpio(PriceState::new("SE3".to_string()), test_config(), cancel);
        let state = test_state(Some(handle), PriceState::new("SE3".to_string()));
        let past = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let query = SmartGridQuery {
            mode: "blocking".to_string(),
            schedule_resume: true,
            resume_at: Some(past),
        };
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest));
    }

    #[tokio::test]
    async fn test_set_smartgrid_future_resume_at_passes_validation() {
        let cancel = CancellationToken::new();
        let (handle, _join) =
            spawn_with_test_gpio(PriceState::new("SE3".to_string()), test_config(), cancel);
        let state = test_state(Some(handle), PriceState::new("SE3".to_string()));
        let future = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let query = SmartGridQuery {
            mode: "blocking".to_string(),
            schedule_resume: true,
            resume_at: Some(future),
        };
        let result = set_smartgrid(State(state), Query(query)).await;
        // Validation passed (not BadRequest). The test GPIO can't write
        // Blocking, so the error — if any — is InternalError, never BadRequest.
        if let Err(e) = result {
            assert!(
                matches!(e, ApiError::InternalError),
                "expected InternalError from test GPIO, not BadRequest"
            );
        }
    }

    #[tokio::test]
    async fn test_set_smartgrid_recent_past_resume_at_within_grace_ok() {
        let cancel = CancellationToken::new();
        let (handle, _join) =
            spawn_with_test_gpio(PriceState::new("SE3".to_string()), test_config(), cancel);
        let state = test_state(Some(handle), PriceState::new("SE3".to_string()));
        // 10s in the past — inside the 60s grace, must NOT be BadRequest.
        let recent = (Utc::now() - chrono::Duration::seconds(10)).to_rfc3339();
        let query = SmartGridQuery {
            mode: "blocking".to_string(),
            schedule_resume: true,
            resume_at: Some(recent),
        };
        let result = set_smartgrid(State(state), Query(query)).await;
        if let Err(e) = result {
            assert!(
                matches!(e, ApiError::InternalError),
                "within-grace pick must pass validation, not 400"
            );
        }
    }

    fn state_with_handle() -> (SmartGridState, CancellationToken) {
        let cancel = CancellationToken::new();
        let (handle, _join) = spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );
        let state = test_state(Some(handle), PriceState::new("SE3".to_string()));
        (state, cancel)
    }

    /// With a handle present, an unparseable mode string reaches the
    /// `SmartGridMode::from_str` step and maps to `BadRequest` — the
    /// no-handle test short-circuits at `ServiceUnavailable` before this.
    #[tokio::test]
    async fn test_set_smartgrid_invalid_mode_with_handle_is_bad_request() {
        let (state, _cancel) = state_with_handle();
        let query = SmartGridQuery {
            mode: "not_a_mode".to_string(),
            schedule_resume: false,
            resume_at: None,
        };
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest));
    }

    /// A `resume_at` that isn't valid RFC 3339 fails the
    /// `DateTime::parse_from_rfc3339` step with `BadRequest`.
    #[tokio::test]
    async fn test_set_smartgrid_unparseable_resume_at_is_bad_request() {
        let (state, _cancel) = state_with_handle();
        let query = SmartGridQuery {
            mode: "blocking".to_string(),
            schedule_resume: true,
            resume_at: Some("definitely-not-a-timestamp".to_string()),
        };
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest));
    }

    /// Happy path for `get_smartgrid` with a live handle: the test GPIO's
    /// in-memory mode is Normal, no change timestamp, no scheduled resume.
    /// Covers the success serialization branch and the `Option::None`
    /// `skip_serializing_if` arms.
    #[tokio::test]
    async fn test_get_smartgrid_with_handle_returns_normal() {
        let (state, _cancel) = state_with_handle();
        let result = get_smartgrid(State(state)).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(result.trim()).unwrap();
        assert_eq!(parsed["smartgrid_mode"], "normal");
        assert_eq!(parsed["run_minutes"], 30);
        // No mode change yet and nothing scheduled → both fields omitted.
        assert!(parsed.get("changed_at").is_none());
        assert!(parsed.get("scheduled_resume_at").is_none());
    }

    /// When the actor has shut down, the handle returns `ActorGone`, which
    /// `map_smartgrid_error` maps to `ServiceUnavailable` (not `InternalError`).
    #[tokio::test]
    async fn test_get_smartgrid_actor_gone_maps_to_service_unavailable() {
        let cancel = CancellationToken::new();
        let (handle, join) = spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );
        let state = test_state(Some(handle), PriceState::new("SE3".to_string()));
        // Tear the actor down so the next handle call sees a closed channel.
        cancel.cancel();
        let _ = join.await;
        let result = get_smartgrid(State(state)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_delete_scheduled_resume_no_gpio() {
        let state = create_state_without_handle();
        let result = delete_scheduled_resume(State(state)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_delete_scheduled_resume_with_handle_returns_no_content() {
        let cancel = CancellationToken::new();
        let (handle, _join) =
            spawn_with_test_gpio(PriceState::new("SE3".to_string()), test_config(), cancel);
        let state = test_state(Some(handle), PriceState::new("SE3".to_string()));
        let status = delete_scheduled_resume(State(state.clone())).await.unwrap();
        assert_eq!(status, StatusCode::NO_CONTENT);

        let again = delete_scheduled_resume(State(state)).await.unwrap();
        assert_eq!(again, StatusCode::NO_CONTENT);
    }

    // ---- warm-by ----

    use crate::smartgrid::actor::test_support::spawn_accepting_test_gpio_with_modbus;
    use chrono::TimeZone;

    /// Modbus fake that returns a fixed tank temperature for any read.
    struct FakeTemp(f32);
    #[async_trait::async_trait]
    impl ModbusWriter for FakeTemp {
        async fn write_scaled(&self, _addr: u16, _v: f32) -> Result<(), String> {
            Ok(())
        }
        async fn read_scaled(&self, _addr: u16) -> Result<f32, String> {
            Ok(self.0)
        }
    }

    fn test_state_with_modbus(
        handle: Option<SmartGridHandle>,
        price_state: PriceState,
        modbus: Arc<dyn ModbusWriter>,
    ) -> SmartGridState {
        SmartGridState {
            handle,
            price_state,
            config: test_config(),
            tz: chrono_tz::Europe::Stockholm,
            modbus: Some(modbus),
        }
    }

    #[test]
    fn hh_mm_today_when_still_ahead() {
        // 04:00Z = 05:00 local (winter, UTC+1); 06:30 is later today.
        let now = Utc.with_ymd_and_hms(2026, 1, 15, 4, 0, 0).unwrap();
        let got = next_deadline_from_hh_mm("06:30", chrono_tz::Europe::Stockholm, now).unwrap();
        let expected = SystemTime::from(Utc.with_ymd_and_hms(2026, 1, 15, 5, 30, 0).unwrap());
        assert_eq!(got, expected);
    }

    #[test]
    fn hh_mm_rolls_to_tomorrow_when_past() {
        // 10:00Z = 11:00 local; 06:30 already passed → tomorrow.
        let now = Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap();
        let got = next_deadline_from_hh_mm("06:30", chrono_tz::Europe::Stockholm, now).unwrap();
        let expected = SystemTime::from(Utc.with_ymd_and_hms(2026, 1, 16, 5, 30, 0).unwrap());
        assert_eq!(got, expected);
    }

    #[test]
    fn hh_mm_rejects_bad_input() {
        let tz = chrono_tz::Europe::Stockholm;
        let now = Utc.with_ymd_and_hms(2026, 1, 15, 4, 0, 0).unwrap();
        assert!(next_deadline_from_hh_mm("0630", tz, now).is_none());
        assert!(next_deadline_from_hh_mm("25:00", tz, now).is_none());
        assert!(next_deadline_from_hh_mm("06:99", tz, now).is_none());
        assert!(next_deadline_from_hh_mm("ab:cd", tz, now).is_none());
    }

    #[test]
    fn resolve_target_defaults_and_validates() {
        let cfg = test_config();
        assert!((resolve_target_c(None, &cfg).unwrap() - 48.0).abs() < f32::EPSILON);
        assert!((resolve_target_c(Some(46.0), &cfg).unwrap() - 46.0).abs() < f32::EPSILON);
        assert!(matches!(
            resolve_target_c(Some(60.0), &cfg),
            Err(ApiError::BadRequest)
        ));
        assert!(matches!(
            resolve_target_c(Some(40.0), &cfg),
            Err(ApiError::BadRequest)
        ));
    }

    #[tokio::test]
    async fn warm_by_no_gpio_is_service_unavailable() {
        let state = create_state_without_handle();
        let query = WarmByQuery {
            warm_by: "06:30".to_string(),
            target_c: None,
        };
        let result = set_warm_by(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn warm_by_bad_deadline_is_bad_request() {
        let cancel = CancellationToken::new();
        let (handle, _join) = spawn_accepting_test_gpio_with_modbus(
            PriceState::new("SE3".to_string()),
            test_config(),
            None,
            cancel,
        );
        let state = test_state(Some(handle), PriceState::new("SE3".to_string()));
        let query = WarmByQuery {
            warm_by: "not-a-time".to_string(),
            target_c: None,
        };
        let result = set_warm_by(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest));
    }

    #[tokio::test]
    async fn warm_by_preview_skips_when_tank_already_warm() {
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeTemp(50.0));
        let state = test_state_with_modbus(None, PriceState::new("SE3".to_string()), modbus);
        let query = WarmByQuery {
            warm_by: "23:59".to_string(),
            target_c: None,
        };
        let body = get_warm_by_preview(State(state), Query(query))
            .await
            .unwrap();
        assert!(body.contains("\"would_skip\":true"), "body: {body}");
        assert!(!body.contains("heatup_start_at"), "skip → no start: {body}");
    }

    #[tokio::test]
    async fn warm_by_preview_plans_heatup_when_cold() {
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeTemp(20.0));
        // No price data → immediate-start fallback, so a start is still produced.
        let state = test_state_with_modbus(None, PriceState::new("SE3".to_string()), modbus);
        let query = WarmByQuery {
            warm_by: "23:59".to_string(),
            target_c: None,
        };
        let body = get_warm_by_preview(State(state), Query(query))
            .await
            .unwrap();
        assert!(body.contains("\"would_skip\":false"), "body: {body}");
        assert!(body.contains("heatup_start_at"), "cold → has start: {body}");
        assert!(body.contains("est_minutes"), "cold → has estimate: {body}");
    }

    #[tokio::test]
    async fn warm_by_post_skip_path_blocks_without_heatup() {
        let cancel = CancellationToken::new();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeTemp(50.0));
        let (handle, _join) = spawn_accepting_test_gpio_with_modbus(
            PriceState::new("SE3".to_string()),
            test_config(),
            Some(modbus.clone()),
            cancel,
        );
        let state =
            test_state_with_modbus(Some(handle), PriceState::new("SE3".to_string()), modbus);
        let query = WarmByQuery {
            warm_by: "23:59".to_string(),
            target_c: None,
        };
        let body = set_warm_by(State(state), Query(query)).await.unwrap();
        assert!(body.contains("\"applied_blocking\":true"), "body: {body}");
        assert!(body.contains("\"skipped\":true"), "body: {body}");
        assert!(!body.contains("heatup_start_at"), "skip → no start: {body}");
    }

    #[tokio::test]
    async fn warm_by_preview_no_modbus_is_service_unavailable() {
        // create_state_without_handle has modbus None → temp read impossible.
        let state = create_state_without_handle();
        let query = WarmByQuery {
            warm_by: "23:59".to_string(),
            target_c: None,
        };
        let result = get_warm_by_preview(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }
}
