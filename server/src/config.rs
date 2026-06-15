//! Configuration management for CTC server
//!
//! This module provides configuration structures and loading logic for the CTC server.
//! Configuration can be loaded from multiple sources with the following priority:
//! 1. CLI arguments (highest priority)
//! 2. Environment variables (`CTC_SERVER_PORT`, `CTC_SERIAL_BAUD_RATE`, etc.)
//! 3. Configuration file (`config.toml`)
//! 4. Hard-coded defaults (lowest priority)

use config::{Config as ConfigBuilder, ConfigError, Environment, File};
use serde::Deserialize;
use std::path::Path;
use std::str::FromStr;
use tokio_serial::{DataBits, FlowControl, Parity, StopBits};

/// Root configuration structure
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub serial: SerialConfig,
    #[serde(default)]
    pub modbus: ModbusConfig,
    #[serde(default)]
    pub temperature_validation: TemperatureValidationConfig,
    #[serde(default)]
    pub gpio: GpioConfig,
    #[serde(default)]
    pub tibber: TibberConfig,
    #[serde(default)]
    pub price: PriceConfig,
    #[serde(default)]
    pub heatpump_stats: HeatPumpStatsConfig,
    #[serde(default)]
    pub smartgrid: SmartGridConfig,
    #[serde(default)]
    pub homey: HomeyConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub dhw: DhwConfig,
    /// IANA timezone used for local-time conversions (e.g. daily-stats keying,
    /// price-fetch schedule). The Göteborg Energi tariff calendar is always
    /// Swedish and ignores this setting.
    #[serde(default = "default_tz")]
    pub tz: String,
}

fn default_tz() -> String {
    "Europe/Stockholm".to_string()
}

/// Embedded redb store configuration
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    /// Path to the redb database file. Sensor cache, heatpump stats, and
    /// trend history all live here.
    pub db_path: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            db_path: "./data/ctc.redb".to_string(),
        }
    }
}

/// HTTP server configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Host address to bind to (e.g., "0.0.0.0" or "127.0.0.1")
    pub host: String,
    /// Port number to listen on
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 3000,
        }
    }
}

/// Serial port configuration for Modbus RTU
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SerialConfig {
    /// Default serial port path if not provided via CLI
    pub default_port: String,
    /// Baud rate (typically 9600 for CTC systems)
    pub baud_rate: u32,
    /// Data bits (7 or 8)
    pub data_bits: u8,
    /// Parity: "none", "even", or "odd"
    pub parity: String,
    /// Stop bits (1 or 2)
    pub stop_bits: u8,
    /// Flow control: "none", "software", or "hardware"
    pub flow_control: String,
    /// Timeout in seconds for serial operations
    pub timeout_secs: u64,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            default_port: "/dev/ttyAMA4".to_string(),
            baud_rate: 9600,
            data_bits: 8,
            parity: "even".to_string(),
            stop_bits: 1,
            flow_control: "hardware".to_string(),
            timeout_secs: 1,
        }
    }
}

/// Modbus protocol configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModbusConfig {
    /// Modbus slave ID (typically 1 for CTC systems)
    pub slave_id: u8,
    /// Channel buffer size for actor message queue
    pub channel_buffer_size: usize,
    /// Operation timeout in seconds (timeout for individual Modbus read/write operations)
    pub operation_timeout_secs: u64,
    /// Maximum number of retry attempts for failed operations
    pub max_retries: u32,
    /// Initial delay in milliseconds before first retry
    pub initial_retry_delay_ms: u64,
    /// Exponential backoff multiplier for retry delays
    pub backoff_multiplier: f64,
    /// Number of consecutive failures before logging critical warning
    pub max_consecutive_failures: u32,
    /// Request timeout in seconds (timeout for HTTP handlers waiting for actor response)
    /// Should be higher than `operation_timeout_secs` to allow for retries
    pub request_timeout_secs: u64,
    /// Minimum delay in milliseconds between consecutive Modbus transactions.
    /// Modbus RTU spec requires ≥3.5 character times (≈3.6 ms @ 9600 baud),
    /// but CTC firmware needs more post-response settle time: at 10 ms, prod
    /// observed ~3 timeouts/hour with the timing-out attempt landing exactly
    /// at the gap floor. 25 ms eliminates that class of retry; bump higher
    /// (50+) only if a noisier bus shows residual timeouts.
    pub inter_request_gap_ms: u64,
}

impl Default for ModbusConfig {
    fn default() -> Self {
        Self {
            slave_id: 1,
            channel_buffer_size: 32,
            operation_timeout_secs: 1,
            max_retries: 2,
            initial_retry_delay_ms: 100,
            backoff_multiplier: 2.0,
            max_consecutive_failures: 5,
            request_timeout_secs: 10,
            inter_request_gap_ms: 25,
        }
    }
}

/// Temperature validation configuration for API endpoints
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TemperatureValidationConfig {
    /// Minimum allowed room temperature setpoint (°C)
    pub min: f32,
    /// Maximum allowed room temperature setpoint (°C)
    pub max: f32,
}

impl Default for TemperatureValidationConfig {
    fn default() -> Self {
        Self {
            min: 5.0,
            max: 30.0,
        }
    }
}

/// GPIO relay configuration for `SmartGrid` control
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GpioConfig {
    /// Enable GPIO-based `SmartGrid` control
    pub enabled: bool,
    /// GPIO pin for K24 (Smart A) terminal
    pub gpio_k24: u32,
    /// GPIO pin for K25 (Smart B) terminal
    pub gpio_k25: u32,
    /// True if relay board uses active-low logic (LOW = relay ON)
    pub active_low: bool,
}

impl Default for GpioConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gpio_k24: 20,
            gpio_k25: 21,
            active_low: false,
        }
    }
}

/// Tibber API configuration for energy consumption data
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TibberConfig {
    /// Enable Tibber API integration
    pub enabled: bool,
    /// API settings (token)
    pub api: TibberApi,
}

/// Tibber API credentials
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TibberApi {
    /// Tibber API token (get from <https://developer.tibber.com>)
    /// Set via `CTC_TIBBER_API_TOKEN` environment variable
    pub token: Option<String>,
}

/// Electricity price configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PriceConfig {
    /// Enable price tracking
    pub enabled: bool,
    /// Price zone: SE1 (Luleå), SE2 (Sundsvall), SE3 (Stockholm), SE4 (Malmö)
    pub zone: String,
    /// Hour (Swedish local time, 0-23) at which to fetch the daily spot prices.
    /// elprisetjustnu.se publishes once per day around 13:00 local, so the
    /// default 14 leaves a 1-hour cushion. Set to 15 or 16 if the publisher
    /// is consistently late, or earlier (e.g. 13) on days when you're willing
    /// to retry until the data lands.
    pub fetch_hour_local: u32,
}

impl Default for PriceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            zone: "SE3".to_string(),
            fetch_hour_local: 14,
        }
    }
}

/// Heat pump statistics tracking configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HeatPumpStatsConfig {
    /// Enable heat pump statistics tracking
    pub enabled: bool,
    /// Polling interval in seconds (how often to read heat pump status)
    pub poll_interval_secs: u64,
    /// Optional path to a JSON file used to persist accumulators and history
    /// across restarts. `None` (or empty) disables persistence.
    pub persist_path: Option<String>,
}

impl Default for HeatPumpStatsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 10,
            persist_path: None,
        }
    }
}

/// `SmartGrid` behavioural configuration.
///
/// The `auto_resume_*` prefix is intentional: all three fields belong to the
/// auto-resume scheduler. Flattening or renaming would either lose the
/// grouping in the TOML file or require a nested table — both worse than
/// the prefix repetition. Hence `#[allow(clippy::struct_field_names)]`.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SmartGridConfig {
    /// Enable auto-resume to Normal after a manually-triggered Blocking
    pub auto_resume_enabled: bool,
    /// How far ahead the cheapest-slot scan looks (hours)
    pub auto_resume_window_hours: u64,
    /// Minimum contiguous run length the Blocking-resume scan looks for, in
    /// minutes. Picking a single 15-min slot can land the heater in a brief
    /// dip that is over before recovery heating completes; widening this to
    /// 30 min (default) selects the start of the cheapest 30-min contiguous
    /// stretch instead. Clamped to `[15, 240]`.
    pub auto_resume_min_duration_minutes: u16,
    /// Enable the "Block + warm-by deadline" one-shot heat-up scheduler.
    pub warm_by_enabled: bool,
    /// Target hot-water tank-top temperature (`dhw_upper`, °C) the warm-by
    /// heat-up aims for by the deadline. Clamped to `[45, 50]`.
    pub warm_by_target_temp_c: f32,
    /// Estimated tank heat-up rate (°C/min) used to size the heat-up window
    /// from the current temperature. Clamped to `[0.05, 5.0]`.
    pub warm_by_heat_rate_c_per_min: f32,
    /// Assumed standby cooldown rate (°C/min) used to place the heat-up window
    /// so the tank is still at target at the deadline. A learned value will
    /// replace this constant in a later phase. Clamped to `[0.0, 2.0]`.
    pub warm_by_cooldown_c_per_min: f32,
    /// How far before the deadline the cheapest-window scan may start, in
    /// minutes. Bounds the window to `[deadline - max_lead, deadline]`.
    /// Clamped to `[30, 240]`.
    pub warm_by_max_lead_minutes: u16,
    /// Safety cap on how long the warm-by heat-up may run before forcing a
    /// re-block, in minutes. Clamped to `[15, 240]`.
    pub warm_by_max_duration_minutes: u16,
}

impl Default for SmartGridConfig {
    fn default() -> Self {
        Self {
            auto_resume_enabled: true,
            auto_resume_window_hours: 12,
            auto_resume_min_duration_minutes: 30,
            warm_by_enabled: true,
            warm_by_target_temp_c: 48.0,
            warm_by_heat_rate_c_per_min: 0.4,
            warm_by_cooldown_c_per_min: 0.05,
            warm_by_max_lead_minutes: 90,
            warm_by_max_duration_minutes: 90,
        }
    }
}

/// Homey REST API integration for controlling the Cirkulationspump smart plug.
///
/// When `enabled = true`, the `SmartGrid` actor pushes the pump on/off via the
/// Homey REST API on every mode change (`Blocking` → off, anything else → on),
/// and a reconciliation poller corrects drift every `poll_interval_secs`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HomeyConfig {
    /// Enable Homey REST integration
    pub enabled: bool,
    /// Homey Pro LAN URL (no trailing slash), e.g. `http://192.168.10.10`
    pub url: String,
    /// Personal Access Token. Required scopes:
    /// `homey.device.control`, `homey.device.readonly`.
    /// Set via `CTC_HOMEY_TOKEN` env var — never commit.
    pub token: Option<String>,
    /// Device id of the smart plug acting as the pump switch.
    pub pump_device_id: String,
    /// Reconciliation poll interval in seconds. `0` disables the poller
    /// (push-only mode — drift will not be corrected).
    pub poll_interval_secs: u64,
}

impl Default for HomeyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            token: None,
            pump_device_id: String::new(),
            poll_interval_secs: 60,
        }
    }
}

/// Domestic-hot-water (DHW) boost controller configuration.
///
/// Consumed by the DHW actor (added in later tasks). Defaults come from
/// `Default::default()` so missing fields fall back to the documented values.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DhwConfig {
    /// Shower preset duration in minutes (heater-side timer matched by watcher).
    pub shower_duration_minutes: u32,
    /// Bath slider upper bound (hours). Range \[0.5, `bath_max_hours`\] in 0.5 steps.
    pub bath_max_hours: f32,
    /// Cancel Bath if `CTC_ROOM_TEMP` drops below this (°C).
    pub boost_room_temp_bail_c: f32,
    /// Spot price ceiling (SEK/kWh) for Bath immersion gate, centre value.
    pub immersion_allow_price_sek_per_kwh: f32,
    /// Hysteresis around the immersion gate (SEK/kWh).
    pub immersion_hysteresis_sek_per_kwh: f32,
    /// Power cap written to 61591 while the immersion gate is engaged (kW).
    pub immersion_kw_when_allowed: f32,
    /// `61636` value written while a Bath is active (°C).
    pub immersion_engage_temp_c: f32,
    /// Path to the persistence JSON. `None` = no persistence.
    pub persist_path: Option<std::path::PathBuf>,
}

impl Default for DhwConfig {
    fn default() -> Self {
        Self {
            shower_duration_minutes: 30,
            bath_max_hours: 2.0,
            boost_room_temp_bail_c: 17.0,
            immersion_allow_price_sek_per_kwh: 0.50,
            immersion_hysteresis_sek_per_kwh: 0.05,
            immersion_kw_when_allowed: 3.0,
            immersion_engage_temp_c: 50.0,
            persist_path: None,
        }
    }
}

impl Config {
    /// Load configuration from file, environment variables, and defaults
    ///
    /// # Arguments
    /// * `config_path` - Optional path to configuration file
    ///
    /// # Returns
    /// Configuration struct or error if loading fails
    pub fn load(config_path: Option<&str>) -> Result<Self, ConfigError> {
        Self::load_with_env(config_path, |k| std::env::var(k).ok())
    }

    /// Same as [`Self::load`] but with an injectable env lookup. Lets tests
    /// stub the few env vars that the `config` crate's `Environment` source
    /// cannot route correctly because field names contain underscores
    /// (`storage.db_path`, `heatpump_stats.persist_path`).
    fn load_with_env<F>(config_path: Option<&str>, get_env: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut builder = ConfigBuilder::builder();

        // Load from config file if it exists
        if let Some(path) = config_path {
            if Path::new(path).exists() {
                builder = builder.add_source(File::with_name(path));
            }
        } else {
            // Try default config.toml in current directory
            if Path::new("config.toml").exists() {
                builder = builder.add_source(File::with_name("config.toml"));
            }
        }

        // Add environment variables with CTC_ prefix.
        // Works for single-word field names (CTC_SERVER_PORT -> server.port).
        // Multi-word field names (e.g. `serial.baud_rate`, `storage.db_path`)
        // do not round-trip through this source because `separator("_")` would
        // map `BAUD_RATE` to `baud.rate`, not `baud_rate`. The two path vars
        // we ship in docker-compose are read explicitly below.
        builder = builder.add_source(
            Environment::with_prefix("CTC")
                .separator("_")
                .try_parsing(true),
        );

        // Explicit overrides for keys whose field names contain underscores.
        // `config::Environment` with `separator("_")` would map e.g.
        // CTC_SERIAL_BAUD_RATE -> serial.baud.rate (not serial.baud_rate),
        // so each underscored field is routed manually here.
        for (env_key, cfg_key) in [
            ("CTC_STORAGE_DB_PATH", "storage.db_path"),
            (
                "CTC_HEATPUMP_STATS_PERSIST_PATH",
                "heatpump_stats.persist_path",
            ),
            ("CTC_SERIAL_DEFAULT_PORT", "serial.default_port"),
            ("CTC_SERIAL_BAUD_RATE", "serial.baud_rate"),
            ("CTC_SERIAL_DATA_BITS", "serial.data_bits"),
            ("CTC_SERIAL_STOP_BITS", "serial.stop_bits"),
            ("CTC_SERIAL_FLOW_CONTROL", "serial.flow_control"),
            ("CTC_SERIAL_TIMEOUT_SECS", "serial.timeout_secs"),
            (
                "CTC_MODBUS_CHANNEL_BUFFER_SIZE",
                "modbus.channel_buffer_size",
            ),
            ("CTC_HOMEY_ENABLED", "homey.enabled"),
            ("CTC_HOMEY_URL", "homey.url"),
            ("CTC_HOMEY_TOKEN", "homey.token"),
            ("CTC_HOMEY_PUMP_DEVICE_ID", "homey.pump_device_id"),
            ("CTC_HOMEY_POLL_INTERVAL_SECS", "homey.poll_interval_secs"),
        ] {
            if let Some(v) = get_env(env_key) {
                builder = builder.set_override(cfg_key, v)?;
            }
        }

        let mut cfg: Self = builder.build()?.try_deserialize()?;
        // Tibber/elpris only publish today + tomorrow, so a scan window
        // beyond 48 h cannot find additional slots. Clamp to avoid the
        // `.saturating_mul(3600)` overflow path on misconfig.
        cfg.smartgrid.auto_resume_window_hours =
            cfg.smartgrid.auto_resume_window_hours.clamp(1, 48);
        // Anything below 15 min would degenerate to the previous single-slot
        // behaviour; anything above 4 h is longer than any plausible recovery
        // cycle and would forbid valid runs from being selected.
        cfg.smartgrid.auto_resume_min_duration_minutes = cfg
            .smartgrid
            .auto_resume_min_duration_minutes
            .clamp(15, 240);
        // Warm-by knobs: clamp to sane ranges so a config typo cannot produce
        // a div-by-zero (heat rate), an unreachable target, or an unbounded
        // heat-up. Mirrors the auto_resume clamps above.
        cfg.smartgrid.warm_by_target_temp_c = cfg.smartgrid.warm_by_target_temp_c.clamp(45.0, 50.0);
        cfg.smartgrid.warm_by_heat_rate_c_per_min =
            cfg.smartgrid.warm_by_heat_rate_c_per_min.clamp(0.05, 5.0);
        cfg.smartgrid.warm_by_cooldown_c_per_min =
            cfg.smartgrid.warm_by_cooldown_c_per_min.clamp(0.0, 2.0);
        cfg.smartgrid.warm_by_max_lead_minutes =
            cfg.smartgrid.warm_by_max_lead_minutes.clamp(30, 240);
        cfg.smartgrid.warm_by_max_duration_minutes =
            cfg.smartgrid.warm_by_max_duration_minutes.clamp(15, 240);
        // Hour-of-day is 0..=23. Anything else is a config bug; clamp so a
        // typo doesn't cause the scheduler to skip days.
        if cfg.price.fetch_hour_local > 23 {
            return Err(ConfigError::Message(format!(
                "Invalid price.fetch_hour_local: {} (must be 0..=23)",
                cfg.price.fetch_hour_local
            )));
        }
        // Validate the timezone string by parsing it. Surface a clear error
        // instead of letting `parsed_tz()` panic at first use.
        if chrono_tz::Tz::from_str(&cfg.tz).is_err() {
            return Err(ConfigError::Message(format!(
                "Invalid tz: '{}' is not a valid IANA timezone (e.g. 'Europe/Stockholm', 'America/New_York')",
                cfg.tz
            )));
        }
        // When Homey is enabled, every connection parameter must be present —
        // surface the misconfig at startup rather than at the first failed
        // request. When disabled, the fields are unused.
        if cfg.homey.enabled {
            if cfg.homey.url.is_empty() {
                return Err(ConfigError::Message(
                    "homey.enabled = true but homey.url is empty (set CTC_HOMEY_URL)".into(),
                ));
            }
            if cfg.homey.token.as_deref().is_none_or(str::is_empty) {
                return Err(ConfigError::Message(
                    "homey.enabled = true but homey.token is empty (set CTC_HOMEY_TOKEN)".into(),
                ));
            }
            if cfg.homey.pump_device_id.is_empty() {
                return Err(ConfigError::Message(
                    "homey.enabled = true but homey.pump_device_id is empty (set CTC_HOMEY_PUMP_DEVICE_ID)".into(),
                ));
            }
        }

        // Explicit env override for the DHW persistence path. The implicit
        // `Environment` source above splits on every `_`, so it cannot route
        // `CTC_DHW_PERSIST_PATH` to `dhw.persist_path` reliably; read it
        // directly here instead.
        if let Ok(path) = std::env::var("CTC_DHW_PERSIST_PATH")
            && !path.is_empty()
        {
            cfg.dhw.persist_path = Some(std::path::PathBuf::from(path));
        }

        Ok(cfg)
    }

    /// Return the parsed timezone. Safe to `expect` because `load_with_env`
    /// validates the string before returning the `Config`.
    #[must_use]
    pub fn parsed_tz(&self) -> chrono_tz::Tz {
        chrono_tz::Tz::from_str(&self.tz).expect("tz validated at load")
    }
}

impl SerialConfig {
    /// Convert parity string to `tokio_serial::Parity`.
    ///
    /// # Errors
    /// Returns `ConfigError::Message` if `parity` is not "none", "even", or "odd".
    pub fn get_parity(&self) -> Result<Parity, ConfigError> {
        match self.parity.to_lowercase().as_str() {
            "none" => Ok(Parity::None),
            "even" => Ok(Parity::Even),
            "odd" => Ok(Parity::Odd),
            _ => Err(ConfigError::Message(format!(
                "Invalid parity value: {}. Must be 'none', 'even', or 'odd'",
                self.parity
            ))),
        }
    }

    /// Convert `data_bits` u8 to `tokio_serial::DataBits`.
    ///
    /// # Errors
    /// Returns `ConfigError::Message` if `data_bits` is not 5, 6, 7, or 8.
    pub fn get_data_bits(&self) -> Result<DataBits, ConfigError> {
        match self.data_bits {
            5 => Ok(DataBits::Five),
            6 => Ok(DataBits::Six),
            7 => Ok(DataBits::Seven),
            8 => Ok(DataBits::Eight),
            _ => Err(ConfigError::Message(format!(
                "Invalid data_bits value: {}. Must be 5, 6, 7, or 8",
                self.data_bits
            ))),
        }
    }

    /// Convert `stop_bits` u8 to `tokio_serial::StopBits`.
    ///
    /// # Errors
    /// Returns `ConfigError::Message` if `stop_bits` is not 1 or 2.
    pub fn get_stop_bits(&self) -> Result<StopBits, ConfigError> {
        match self.stop_bits {
            1 => Ok(StopBits::One),
            2 => Ok(StopBits::Two),
            _ => Err(ConfigError::Message(format!(
                "Invalid stop_bits value: {}. Must be 1 or 2",
                self.stop_bits
            ))),
        }
    }

    /// Convert `flow_control` string to `tokio_serial::FlowControl`.
    ///
    /// # Errors
    /// Returns `ConfigError::Message` if `flow_control` is not "none", "software", or "hardware".
    pub fn get_flow_control(&self) -> Result<FlowControl, ConfigError> {
        match self.flow_control.to_lowercase().as_str() {
            "none" => Ok(FlowControl::None),
            "software" => Ok(FlowControl::Software),
            "hardware" => Ok(FlowControl::Hardware),
            _ => Err(ConfigError::Message(format!(
                "Invalid flow_control value: {}. Must be 'none', 'software', or 'hardware'",
                self.flow_control
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = Config::load(None).expect("Failed to load default config");

        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.serial.default_port, "/dev/ttyAMA4");
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.modbus.slave_id, 1);
    }

    #[test]
    fn test_serial_config_conversions() {
        let config = Config::load(None).expect("Failed to load default config");

        assert!(matches!(config.serial.get_parity().unwrap(), Parity::Even));
        assert!(matches!(
            config.serial.get_data_bits().unwrap(),
            DataBits::Eight
        ));
        assert!(matches!(
            config.serial.get_stop_bits().unwrap(),
            StopBits::One
        ));
        assert!(matches!(
            config.serial.get_flow_control().unwrap(),
            FlowControl::Hardware
        ));
    }

    #[test]
    fn test_serial_config_invalid_values_return_errors() {
        let bad_parity = SerialConfig {
            default_port: String::new(),
            baud_rate: 9600,
            data_bits: 8,
            parity: "bogus".to_string(),
            stop_bits: 1,
            flow_control: "none".to_string(),
            timeout_secs: 1,
        };
        assert!(bad_parity.get_parity().is_err());

        let bad_data_bits = SerialConfig {
            default_port: String::new(),
            baud_rate: 9600,
            data_bits: 4,
            parity: "none".to_string(),
            stop_bits: 1,
            flow_control: "none".to_string(),
            timeout_secs: 1,
        };
        assert!(bad_data_bits.get_data_bits().is_err());

        let bad_stop_bits = SerialConfig {
            default_port: String::new(),
            baud_rate: 9600,
            data_bits: 8,
            parity: "none".to_string(),
            stop_bits: 3,
            flow_control: "none".to_string(),
            timeout_secs: 1,
        };
        assert!(bad_stop_bits.get_stop_bits().is_err());

        let bad_flow = SerialConfig {
            default_port: String::new(),
            baud_rate: 9600,
            data_bits: 8,
            parity: "none".to_string(),
            stop_bits: 1,
            flow_control: "bogus".to_string(),
            timeout_secs: 1,
        };
        assert!(bad_flow.get_flow_control().is_err());
    }

    #[test]
    fn test_partial_config() {
        // Test that the config system works when only some values are specified
        // This simulates a user having a config.toml with only port = 8080.
        // `#[serde(default)]` on every substruct lets every other field fall
        // back to `Default::default()` during deserialization.
        let builder = ConfigBuilder::builder()
            .set_override("server.port", 8080)
            .unwrap();

        let config: Config = builder.build().unwrap().try_deserialize().unwrap();

        // Verify the overridden value
        assert_eq!(config.server.port, 8080);

        // Verify defaults still work
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.serial.baud_rate, 9600);
        assert_eq!(config.modbus.slave_id, 1);
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = Config::load(None).expect("Failed to load default config");

        assert_eq!(config.modbus.operation_timeout_secs, 1);
        assert_eq!(config.modbus.max_retries, 2);
        assert_eq!(config.modbus.initial_retry_delay_ms, 100);
        assert!((config.modbus.backoff_multiplier - 2.0).abs() < f64::EPSILON);
        assert_eq!(config.modbus.max_consecutive_failures, 5);
    }

    #[test]
    fn test_retry_backoff_multiplier_is_positive() {
        let config = Config::load(None).expect("Failed to load default config");
        assert!(config.modbus.backoff_multiplier > 0.0);
    }

    #[test]
    fn test_max_retries_reasonable() {
        let config = Config::load(None).expect("Failed to load default config");
        // Should be between 0 and 10
        assert!(config.modbus.max_retries <= 10);
    }

    #[test]
    fn test_initial_retry_delay_positive() {
        let config = Config::load(None).expect("Failed to load default config");
        assert!(config.modbus.initial_retry_delay_ms > 0);
    }

    #[test]
    fn test_max_consecutive_failures_reasonable() {
        let config = Config::load(None).expect("Failed to load default config");
        assert!(config.modbus.max_consecutive_failures > 0);
        assert!(config.modbus.max_consecutive_failures < 100);
    }

    #[test]
    fn explicit_env_path_overrides_take_effect() {
        let env = |k: &str| -> Option<String> {
            match k {
                "CTC_STORAGE_DB_PATH" => Some("/custom/ctc.redb".to_string()),
                "CTC_HEATPUMP_STATS_PERSIST_PATH" => Some("/custom/stats.json".to_string()),
                _ => None,
            }
        };
        let cfg = Config::load_with_env(None, env).expect("load");
        assert_eq!(cfg.storage.db_path, "/custom/ctc.redb");
        assert_eq!(
            cfg.heatpump_stats.persist_path.as_deref(),
            Some("/custom/stats.json")
        );
    }

    #[test]
    fn explicit_env_path_overrides_absent_use_defaults() {
        let cfg = Config::load_with_env(None, |_| None).expect("load");
        assert_eq!(cfg.storage.db_path, "./data/ctc.redb");
        assert!(cfg.heatpump_stats.persist_path.is_none());
    }

    #[test]
    fn homey_disabled_by_default() {
        let cfg = Config::load(None).expect("load");
        assert!(!cfg.homey.enabled);
        assert!(cfg.homey.url.is_empty());
        assert!(cfg.homey.token.is_none());
        assert!(cfg.homey.pump_device_id.is_empty());
        assert_eq!(cfg.homey.poll_interval_secs, 60);
    }

    #[test]
    fn homey_env_vars_route_correctly() {
        let env = |k: &str| -> Option<String> {
            match k {
                "CTC_HOMEY_ENABLED" => Some("true".into()),
                "CTC_HOMEY_URL" => Some("http://homey.local".into()),
                "CTC_HOMEY_TOKEN" => Some("pat-abc".into()),
                "CTC_HOMEY_PUMP_DEVICE_ID" => Some("dev-xyz".into()),
                "CTC_HOMEY_POLL_INTERVAL_SECS" => Some("30".into()),
                _ => None,
            }
        };
        let cfg = Config::load_with_env(None, env).expect("load");
        assert!(cfg.homey.enabled);
        assert_eq!(cfg.homey.url, "http://homey.local");
        assert_eq!(cfg.homey.token.as_deref(), Some("pat-abc"));
        assert_eq!(cfg.homey.pump_device_id, "dev-xyz");
        assert_eq!(cfg.homey.poll_interval_secs, 30);
    }

    #[test]
    fn homey_enabled_requires_url() {
        let env = |k: &str| -> Option<String> {
            match k {
                "CTC_HOMEY_ENABLED" => Some("true".into()),
                "CTC_HOMEY_TOKEN" => Some("pat-abc".into()),
                "CTC_HOMEY_PUMP_DEVICE_ID" => Some("dev-xyz".into()),
                _ => None,
            }
        };
        let err = Config::load_with_env(None, env).unwrap_err();
        assert!(
            err.to_string().contains("homey.url"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn homey_enabled_requires_token() {
        let env = |k: &str| -> Option<String> {
            match k {
                "CTC_HOMEY_ENABLED" => Some("true".into()),
                "CTC_HOMEY_URL" => Some("http://homey.local".into()),
                "CTC_HOMEY_PUMP_DEVICE_ID" => Some("dev-xyz".into()),
                _ => None,
            }
        };
        let err = Config::load_with_env(None, env).unwrap_err();
        assert!(
            err.to_string().contains("homey.token"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn homey_enabled_requires_pump_device_id() {
        let env = |k: &str| -> Option<String> {
            match k {
                "CTC_HOMEY_ENABLED" => Some("true".into()),
                "CTC_HOMEY_URL" => Some("http://homey.local".into()),
                "CTC_HOMEY_TOKEN" => Some("pat-abc".into()),
                _ => None,
            }
        };
        let err = Config::load_with_env(None, env).unwrap_err();
        assert!(
            err.to_string().contains("homey.pump_device_id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn homey_disabled_accepts_empty_fields() {
        // Defaults: enabled=false, all fields empty/None. Must still validate.
        let cfg = Config::load_with_env(None, |_| None).expect("load");
        assert!(!cfg.homey.enabled);
    }

    #[test]
    fn dhw_config_defaults() {
        let cfg: DhwConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.shower_duration_minutes, 30);
        assert!((cfg.bath_max_hours - 2.0).abs() < f32::EPSILON);
        assert!((cfg.boost_room_temp_bail_c - 17.0).abs() < f32::EPSILON);
        assert!((cfg.immersion_allow_price_sek_per_kwh - 0.50).abs() < f32::EPSILON);
        assert!((cfg.immersion_hysteresis_sek_per_kwh - 0.05).abs() < f32::EPSILON);
        assert!((cfg.immersion_kw_when_allowed - 3.0).abs() < f32::EPSILON);
        assert!((cfg.immersion_engage_temp_c - 50.0).abs() < f32::EPSILON);
        assert!(cfg.persist_path.is_none());
    }

    #[test]
    fn dhw_config_overrides_parse() {
        let cfg: DhwConfig = toml::from_str(
            r#"
            shower_duration_minutes = 20
            bath_max_hours = 3.0
            boost_room_temp_bail_c = 18.5
            immersion_allow_price_sek_per_kwh = 0.40
            immersion_hysteresis_sek_per_kwh = 0.10
            immersion_kw_when_allowed = 5.5
            immersion_engage_temp_c = 45.0
            persist_path = "/var/lib/ctc/dhw.json"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.shower_duration_minutes, 20);
        assert!((cfg.bath_max_hours - 3.0).abs() < f32::EPSILON);
        assert!((cfg.immersion_kw_when_allowed - 5.5).abs() < f32::EPSILON);
        assert_eq!(
            cfg.persist_path.as_deref(),
            Some(std::path::Path::new("/var/lib/ctc/dhw.json"))
        );
    }
}
