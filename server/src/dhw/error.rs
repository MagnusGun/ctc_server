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
