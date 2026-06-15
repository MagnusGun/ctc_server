//! DHW errors and small value types used at the HTTP and actor boundaries.

use axum::http::StatusCode;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComfortLevel {
    Economy,
    Normal,
    Komfort,
    Manuell,
}

impl ComfortLevel {
    /// Parse from a query-string value. Returns None for unrecognised input
    /// AND for `manuell` — Manuell is a read-only state derived from the
    /// heater (61500=3 with 61501 carrying the user's stop temp); the HTTP
    /// surface only accepts Economy/Normal/Komfort as writable choices.
    #[must_use]
    pub fn from_query(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "economy" => Some(Self::Economy),
            "normal" => Some(Self::Normal),
            "komfort" | "comfort" => Some(Self::Komfort),
            // "manuell" intentionally rejected — not a writable target.
            _ => None,
        }
    }

    /// Scaled value to write to `61500` (factor 1.0). Returns `None` for
    /// `Manuell` because it isn't a write target — callers should ensure
    /// they only call this after `from_query` accepted the input.
    #[must_use]
    pub fn as_scaled(self) -> Option<f32> {
        match self {
            Self::Economy => Some(0.0),
            Self::Normal => Some(1.0),
            Self::Komfort => Some(2.0),
            Self::Manuell => None,
        }
    }

    /// Map a raw `61500` value back into a variant. Used for read-back at
    /// startup and dashboard snapshot. Unknown raw values fall back to
    /// `Manuell` (the safest "this isn't a writable program" label).
    #[must_use]
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            0 => Self::Economy,
            1 => Self::Normal,
            2 => Self::Komfort,
            _ => Self::Manuell,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    TimerExpired,
    RoomTooCold,
    PriceLeftCheap,
    Manual,
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum StartReport {
    Started {
        scheduled_end: chrono::DateTime<chrono::Utc>,
    },
    AlreadyAtTarget {
        dhw_c: f32,
        target_c: f32,
    },
}

#[derive(Debug)]
pub enum DhwError {
    BoostAlreadyActive,
    PriceNotCheap { current_level: String },
    HoursOutOfRange { min: f32, max: f32 },
    ShowerCannotBeCancelled,
    Modbus(String),
    HomeyOverrideSendFailed,
    SmartGrid(String),
    Sensor(&'static str),
    Persistence(String),
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_level: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

impl<'a> ErrorBody<'a> {
    /// Build an `ErrorBody` with only the `error` tag populated; other
    /// fields stay `None` and are skipped during serialisation.
    fn tag(error: &'static str) -> Self {
        Self {
            error,
            field: None,
            min: None,
            max: None,
            current_level: None,
            detail: None,
        }
    }

    fn with_detail(mut self, detail: &'a str) -> Self {
        self.detail = Some(detail);
        self
    }

    fn with_current_level(mut self, level: &'a str) -> Self {
        self.current_level = Some(level);
        self
    }

    fn with_field(mut self, field: &'static str) -> Self {
        self.field = Some(field);
        self
    }

    fn with_range(mut self, min: f32, max: f32) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }
}

impl DhwError {
    /// Convert this error into an HTTP status + JSON body suitable for an
    /// Axum route handler.
    ///
    /// # Panics
    /// The internal `serde_json::to_value` call is on a pure value-type and
    /// cannot fail in practice; we `unwrap` rather than threading a `Result`
    /// through every error-conversion site.
    pub fn into_response(self) -> (StatusCode, axum::Json<serde_json::Value>) {
        let (code, body) = match &self {
            Self::BoostAlreadyActive => {
                (StatusCode::CONFLICT, ErrorBody::tag("boost_already_active"))
            }
            Self::PriceNotCheap { current_level } => (
                StatusCode::CONFLICT,
                ErrorBody::tag("price_not_cheap").with_current_level(current_level),
            ),
            Self::HoursOutOfRange { min, max } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorBody::tag("out_of_range")
                    .with_field("hours")
                    .with_range(*min, *max),
            ),
            Self::ShowerCannotBeCancelled => (
                StatusCode::CONFLICT,
                ErrorBody::tag("shower_runs_to_completion"),
            ),
            Self::Modbus(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody::tag("modbus").with_detail(e),
            ),
            Self::HomeyOverrideSendFailed => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody::tag("homey_override_unavailable"),
            ),
            Self::SmartGrid(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody::tag("smartgrid").with_detail(e),
            ),
            Self::Sensor(name) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorBody::tag("sensor_unavailable").with_field(name),
            ),
            Self::Persistence(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody::tag("persistence").with_detail(e),
            ),
        };
        (code, axum::Json(serde_json::to_value(&body).unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn assert_float_eq(a: f32, b: f32, msg: &str) {
        assert!((a - b).abs() < f32::EPSILON, "{msg}: expected {b}, got {a}");
    }

    #[test]
    fn comfort_from_query_accepts_writable() {
        assert_eq!(
            ComfortLevel::from_query("economy"),
            Some(ComfortLevel::Economy)
        );
        assert_eq!(
            ComfortLevel::from_query("normal"),
            Some(ComfortLevel::Normal)
        );
        assert_eq!(
            ComfortLevel::from_query("komfort"),
            Some(ComfortLevel::Komfort)
        );
        assert_eq!(
            ComfortLevel::from_query("comfort"),
            Some(ComfortLevel::Komfort)
        );
        // Case-insensitive.
        assert_eq!(
            ComfortLevel::from_query("ECONOMY"),
            Some(ComfortLevel::Economy)
        );
    }

    #[test]
    fn comfort_from_query_rejects_manuell_and_unknown() {
        assert_eq!(ComfortLevel::from_query("manuell"), None);
        assert_eq!(ComfortLevel::from_query("garbage"), None);
        assert_eq!(ComfortLevel::from_query(""), None);
    }

    #[test]
    fn comfort_as_scaled() {
        assert_float_eq(ComfortLevel::Economy.as_scaled().unwrap(), 0.0, "economy");
        assert_float_eq(ComfortLevel::Normal.as_scaled().unwrap(), 1.0, "normal");
        assert_float_eq(ComfortLevel::Komfort.as_scaled().unwrap(), 2.0, "komfort");
        assert_eq!(ComfortLevel::Manuell.as_scaled(), None);
    }

    #[test]
    fn comfort_from_raw() {
        assert_eq!(ComfortLevel::from_raw(0), ComfortLevel::Economy);
        assert_eq!(ComfortLevel::from_raw(1), ComfortLevel::Normal);
        assert_eq!(ComfortLevel::from_raw(2), ComfortLevel::Komfort);
        // Anything else (including 3 and negatives) falls back to Manuell.
        assert_eq!(ComfortLevel::from_raw(3), ComfortLevel::Manuell);
        assert_eq!(ComfortLevel::from_raw(-1), ComfortLevel::Manuell);
        assert_eq!(ComfortLevel::from_raw(99), ComfortLevel::Manuell);
    }

    #[test]
    fn comfort_serde_roundtrip_lowercase() {
        // serde rename_all = "lowercase"
        let json = serde_json::to_string(&ComfortLevel::Komfort).unwrap();
        assert_eq!(json, "\"komfort\"");
        let back: ComfortLevel = serde_json::from_str("\"economy\"").unwrap();
        assert_eq!(back, ComfortLevel::Economy);
    }

    #[test]
    fn start_report_serializes_outcome_tag() {
        let end = chrono::Utc.with_ymd_and_hms(2026, 1, 4, 12, 0, 0).unwrap();
        let started = StartReport::Started { scheduled_end: end };
        let v = serde_json::to_value(&started).unwrap();
        assert_eq!(v["outcome"], "started");
        assert!(v["scheduled_end"].is_string());

        let at_target = StartReport::AlreadyAtTarget {
            dhw_c: 52.5,
            target_c: 50.0,
        };
        let v = serde_json::to_value(&at_target).unwrap();
        assert_eq!(v["outcome"], "already_at_target");
        assert!((v["dhw_c"].as_f64().unwrap() - 52.5).abs() < 1e-6);
        assert!((v["target_c"].as_f64().unwrap() - 50.0).abs() < 1e-6);
    }

    fn body_of(err: DhwError) -> (StatusCode, serde_json::Value) {
        let (code, json) = err.into_response();
        (code, json.0)
    }

    #[test]
    fn into_response_boost_already_active() {
        let (code, body) = body_of(DhwError::BoostAlreadyActive);
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(body["error"], "boost_already_active");
        // Optional fields are skipped.
        assert!(body.get("field").is_none());
    }

    #[test]
    fn into_response_price_not_cheap_carries_level() {
        let (code, body) = body_of(DhwError::PriceNotCheap {
            current_level: "Expensive".to_string(),
        });
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(body["error"], "price_not_cheap");
        assert_eq!(body["current_level"], "Expensive");
    }

    #[test]
    fn into_response_hours_out_of_range_carries_field_and_range() {
        let (code, body) = body_of(DhwError::HoursOutOfRange { min: 0.5, max: 6.0 });
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"], "out_of_range");
        assert_eq!(body["field"], "hours");
        assert!((body["min"].as_f64().unwrap() - 0.5).abs() < 1e-6);
        assert!((body["max"].as_f64().unwrap() - 6.0).abs() < 1e-6);
    }

    #[test]
    fn into_response_shower_cannot_be_cancelled() {
        let (code, body) = body_of(DhwError::ShowerCannotBeCancelled);
        assert_eq!(code, StatusCode::CONFLICT);
        assert_eq!(body["error"], "shower_runs_to_completion");
    }

    #[test]
    fn into_response_modbus_carries_detail() {
        let (code, body) = body_of(DhwError::Modbus("bus timeout".to_string()));
        assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "modbus");
        assert_eq!(body["detail"], "bus timeout");
    }

    #[test]
    fn into_response_homey_override_failed() {
        let (code, body) = body_of(DhwError::HomeyOverrideSendFailed);
        assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "homey_override_unavailable");
    }

    #[test]
    fn into_response_smartgrid_carries_detail() {
        let (code, body) = body_of(DhwError::SmartGrid("gpio busy".to_string()));
        assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "smartgrid");
        assert_eq!(body["detail"], "gpio busy");
    }

    #[test]
    fn into_response_sensor_carries_field() {
        let (code, body) = body_of(DhwError::Sensor("dhw_temp"));
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"], "sensor_unavailable");
        assert_eq!(body["field"], "dhw_temp");
    }

    #[test]
    fn into_response_persistence_carries_detail() {
        let (code, body) = body_of(DhwError::Persistence("disk full".to_string()));
        assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "persistence");
        assert_eq!(body["detail"], "disk full");
    }

    #[test]
    fn cancel_reason_is_constructible_and_comparable() {
        // CancelReason is a plain marker enum used at boundaries; assert the
        // derived PartialEq distinguishes variants.
        assert_eq!(CancelReason::TimerExpired, CancelReason::TimerExpired);
        assert_ne!(CancelReason::TimerExpired, CancelReason::Manual);
        assert_ne!(CancelReason::RoomTooCold, CancelReason::PriceLeftCheap);
    }
}
