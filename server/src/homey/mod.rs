//! Homey REST API client for controlling the Cirkulationspump smart plug.
//!
//! Only the two operations the [`SmartGrid`](crate::smartgrid) actor and
//! reconciliation [`poller`] actually need are exposed: read and write the
//! pump's `onoff` capability. Everything else in the Homey API surface is
//! deliberately out of scope.
//!
//! The Personal Access Token lives in [`HomeyClient`] and is never logged —
//! `Debug` redacts it. All requests carry `Authorization: Bearer <pat>`.

use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::HomeyConfig;

pub mod cache;
pub mod poller;

#[cfg(test)]
pub mod test_support;

#[derive(Debug, Error)]
pub enum HomeyError {
    #[error("Homey HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Homey returned status {0}")]
    Status(StatusCode),
    #[error("Homey response missing onoff capability for device {0}")]
    MissingCapability(String),
}

/// Thin REST client for the one Homey device we care about.
///
/// Cheap to clone — `reqwest::Client` is internally reference-counted.
#[derive(Clone)]
pub struct HomeyClient {
    http: reqwest::Client,
    base_url: Arc<str>,
    token: Arc<str>,
    pump_device_id: Arc<str>,
}

impl std::fmt::Debug for HomeyClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeyClient")
            .field("http", &self.http)
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .field("pump_device_id", &self.pump_device_id)
            .finish()
    }
}

impl HomeyClient {
    /// Build a client from a validated [`HomeyConfig`].
    ///
    /// # Errors
    /// Returns an error if the configured token is missing or the underlying
    /// `reqwest::Client` cannot be built.
    pub fn new(cfg: &HomeyConfig) -> Result<Self, String> {
        let token = cfg
            .token
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "homey.token is required when homey.enabled = true".to_string())?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| format!("Failed to build reqwest client: {e}"))?;
        Ok(Self {
            http,
            base_url: trim_trailing_slash(&cfg.url).into(),
            token: token.into(),
            pump_device_id: cfg.pump_device_id.as_str().into(),
        })
    }

    /// Set the pump's `onoff` capability.
    ///
    /// Homey path: `PUT /api/manager/devices/device/:id/capability/onoff`
    /// Body: `{"value": <bool>}`.
    ///
    /// # Errors
    /// Returns `HomeyError::Http` on transport failure, `HomeyError::Status`
    /// on non-2xx responses.
    pub async fn set_pump_onoff(&self, on: bool) -> Result<(), HomeyError> {
        let url = format!(
            "{}/api/manager/devices/device/{}/capability/onoff",
            self.base_url, self.pump_device_id
        );
        let resp = self
            .http
            .put(&url)
            .bearer_auth(self.token.as_ref())
            .json(&SetCapabilityBody { value: on })
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(
                "Homey PUT {url} returned {status}: {body}",
                body = body.chars().take(300).collect::<String>()
            );
            return Err(HomeyError::Status(status));
        }
        Ok(())
    }

    /// Read the pump's `onoff` capability value.
    ///
    /// Homey path: `GET /api/manager/devices/device/:id` — extract
    /// `capabilitiesObj.onoff.value`.
    ///
    /// # Errors
    /// Returns `HomeyError::Http`, `HomeyError::Status`, or
    /// `HomeyError::MissingCapability` if Homey's response shape changes.
    pub async fn get_pump_onoff(&self) -> Result<bool, HomeyError> {
        let url = format!(
            "{}/api/manager/devices/device/{}",
            self.base_url, self.pump_device_id
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.token.as_ref())
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(HomeyError::Status(resp.status()));
        }
        let body: DeviceResponse = resp.json().await?;
        body.capabilities_obj
            .onoff
            .map(|c| c.value)
            .ok_or_else(|| HomeyError::MissingCapability(self.pump_device_id.to_string()))
    }
}

fn trim_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

#[derive(Serialize)]
struct SetCapabilityBody {
    value: bool,
}

#[derive(Deserialize)]
struct DeviceResponse {
    #[serde(rename = "capabilitiesObj")]
    capabilities_obj: CapabilitiesObj,
}

#[derive(Deserialize)]
struct CapabilitiesObj {
    onoff: Option<OnoffCapability>,
}

#[derive(Deserialize)]
struct OnoffCapability {
    value: bool,
}

#[cfg(test)]
mod tests {
    use super::test_support::{MockState, SharedMock, make_client, spawn_mock};
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[tokio::test(flavor = "current_thread")]
    async fn set_pump_onoff_sends_correct_request() {
        let state: SharedMock = Arc::default();
        let addr = spawn_mock(state.clone()).await;
        let client = make_client(addr);

        client.set_pump_onoff(false).await.unwrap();

        let s = state.lock().unwrap();
        assert!(!s.pump_on);
        assert_eq!(
            s.last_set_authorization.as_deref(),
            Some("Bearer test-pat"),
            "missing or wrong Authorization header"
        );
        assert_eq!(s.last_set_body, Some(json!({"value": false})));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_pump_onoff_reads_capability_value() {
        let state: SharedMock = Arc::new(Mutex::new(MockState {
            pump_on: true,
            ..MockState::default()
        }));
        let addr = spawn_mock(state).await;
        let client = make_client(addr);

        assert!(client.get_pump_onoff().await.unwrap());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_pump_onoff_propagates_server_error() {
        let state: SharedMock = Arc::new(Mutex::new(MockState {
            get_returns_error: true,
            ..MockState::default()
        }));
        let addr = spawn_mock(state).await;
        let client = make_client(addr);

        match client.get_pump_onoff().await {
            Err(HomeyError::Status(s)) => assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR),
            other => panic!("expected Status(500), got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn round_trip_set_then_get() {
        let state: SharedMock = Arc::default();
        let addr = spawn_mock(state).await;
        let client = make_client(addr);

        client.set_pump_onoff(true).await.unwrap();
        assert!(client.get_pump_onoff().await.unwrap());
        client.set_pump_onoff(false).await.unwrap();
        assert!(!client.get_pump_onoff().await.unwrap());
    }

    #[test]
    fn debug_redacts_token() {
        let c = HomeyClient {
            http: reqwest::Client::new(),
            base_url: "http://x".into(),
            token: "super-secret-pat".into(),
            pump_device_id: "dev".into(),
        };
        let s = format!("{c:?}");
        assert!(s.contains("<redacted>"), "Debug output: {s}");
        assert!(
            !s.contains("super-secret-pat"),
            "token leaked in Debug: {s}"
        );
    }

    #[test]
    fn new_requires_non_empty_token() {
        let cfg = HomeyConfig {
            enabled: true,
            url: "http://x".into(),
            token: None,
            pump_device_id: "dev".into(),
            poll_interval_secs: 60,
        };
        let err = HomeyClient::new(&cfg).unwrap_err();
        assert!(err.contains("token"), "unexpected: {err}");
    }

    #[test]
    fn new_trims_trailing_slash_in_url() {
        let cfg = HomeyConfig {
            enabled: true,
            url: "http://homey.local/".into(),
            token: Some("t".into()),
            pump_device_id: "dev".into(),
            poll_interval_secs: 60,
        };
        let c = HomeyClient::new(&cfg).unwrap();
        assert_eq!(c.base_url.as_ref(), "http://homey.local");
    }
}
