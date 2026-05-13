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
    pub server: ServerConfig,
    pub serial: SerialConfig,
    pub modbus: ModbusConfig,
    pub temperature_validation: TemperatureValidationConfig,
    pub gpio: GpioConfig,
    pub tibber: TibberConfig,
    pub price: PriceConfig,
    pub heatpump_stats: HeatPumpStatsConfig,
    pub smartgrid: SmartGridConfig,
    pub storage: StorageConfig,
    /// IANA timezone used for local-time conversions (e.g. daily-stats keying,
    /// price-fetch schedule). The Göteborg Energi tariff calendar is always
    /// Swedish and ignores this setting.
    pub tz: String,
}

/// Embedded redb store configuration
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    /// Path to the redb database file. Sensor cache, heatpump stats, and
    /// trend history all live here.
    pub db_path: String,
}

/// HTTP server configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Host address to bind to (e.g., "0.0.0.0" or "127.0.0.1")
    pub host: String,
    /// Port number to listen on
    pub port: u16,
}

/// Serial port configuration for Modbus RTU
#[derive(Debug, Clone, Deserialize)]
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

/// Modbus protocol configuration
#[derive(Debug, Clone, Deserialize)]
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
    /// Modbus RTU spec requires ≥3.5 character times (≈3.6 ms @ 9600 baud);
    /// CTC firmware typically needs more breathing room between back-to-back
    /// reads. 10 ms is a safe default; raise to 20-50 ms on noisy buses.
    pub inter_request_gap_ms: u64,
}

/// Temperature validation configuration for API endpoints
#[derive(Debug, Clone, Deserialize)]
pub struct TemperatureValidationConfig {
    /// Minimum allowed room temperature setpoint (°C)
    pub min: f32,
    /// Maximum allowed room temperature setpoint (°C)
    pub max: f32,
}

/// GPIO relay configuration for `SmartGrid` control
#[derive(Debug, Clone, Deserialize)]
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

/// Tibber API configuration for energy consumption data
#[derive(Debug, Clone, Deserialize)]
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

/// Heat pump statistics tracking configuration
#[derive(Debug, Clone, Deserialize)]
pub struct HeatPumpStatsConfig {
    /// Enable heat pump statistics tracking
    pub enabled: bool,
    /// Polling interval in seconds (how often to read heat pump status)
    pub poll_interval_secs: u64,
    /// Optional path to a JSON file used to persist accumulators and history
    /// across restarts. `None` (or empty) disables persistence.
    #[serde(default)]
    pub persist_path: Option<String>,
}

/// `SmartGrid` behavioural configuration
#[derive(Debug, Clone, Deserialize)]
pub struct SmartGridConfig {
    /// Enable auto-resume to Normal after a manually-triggered Blocking
    pub auto_resume_enabled: bool,
    /// How far ahead the cheapest-slot scan looks (hours)
    pub auto_resume_window_hours: u64,
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
        ] {
            if let Some(v) = get_env(env_key) {
                builder = builder.set_override(cfg_key, v)?;
            }
        }

        // Set default values
        builder = builder
            // Server defaults
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 3000)?
            // Serial defaults
            .set_default("serial.default_port", "/dev/ttyAMA4")?
            .set_default("serial.baud_rate", 9600)?
            .set_default("serial.data_bits", 8)?
            .set_default("serial.parity", "even")?
            .set_default("serial.stop_bits", 1)?
            .set_default("serial.flow_control", "hardware")?
            .set_default("serial.timeout_secs", 1)?
            // Modbus defaults
            .set_default("modbus.slave_id", 1)?
            .set_default("modbus.channel_buffer_size", 32)?
            .set_default("modbus.operation_timeout_secs", 1)?
            .set_default("modbus.max_retries", 2)?
            .set_default("modbus.initial_retry_delay_ms", 100)?
            .set_default("modbus.backoff_multiplier", 2.0)?
            .set_default("modbus.max_consecutive_failures", 5)?
            .set_default("modbus.request_timeout_secs", 10)?
            .set_default("modbus.inter_request_gap_ms", 10)?
            // Temperature validation defaults
            .set_default("temperature_validation.min", 5.0)?
            .set_default("temperature_validation.max", 30.0)?
            // GPIO defaults
            .set_default("gpio.enabled", true)?
            .set_default("gpio.gpio_k24", 20)?
            .set_default("gpio.gpio_k25", 21)?
            .set_default("gpio.active_low", false)?
            // Tibber defaults
            .set_default("tibber.enabled", false)?
            .set_default("tibber.api.token", None::<String>)?
            // Price defaults
            .set_default("price.enabled", true)?
            .set_default("price.zone", "SE3")?
            .set_default("price.fetch_hour_local", 14)?
            // Heat pump stats defaults
            .set_default("heatpump_stats.enabled", true)?
            .set_default("heatpump_stats.poll_interval_secs", 10)?
            .set_default("heatpump_stats.persist_path", None::<String>)?
            // SmartGrid defaults
            .set_default("smartgrid.auto_resume_enabled", true)?
            .set_default("smartgrid.auto_resume_window_hours", 8)?
            // Storage defaults — CTC_DB_PATH env var overrides
            .set_default("storage.db_path", "./data/ctc.redb")?
            // Timezone default — Stockholm preserves prior hardcoded behaviour.
            .set_default("tz", "Europe/Stockholm")?;

        let mut cfg: Self = builder.build()?.try_deserialize()?;
        // Tibber/elpris only publish today + tomorrow, so a scan window
        // beyond 48 h cannot find additional slots. Clamp to avoid the
        // `.saturating_mul(3600)` overflow path on misconfig.
        cfg.smartgrid.auto_resume_window_hours =
            cfg.smartgrid.auto_resume_window_hours.clamp(1, 48);
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
        // This simulates a user having a config.toml with only port = 8080
        let builder = ConfigBuilder::builder()
            .set_default("server.host", "0.0.0.0")
            .unwrap()
            .set_default("server.port", 3000)
            .unwrap()
            .set_override("server.port", 8080)
            .unwrap()
            .set_default("serial.default_port", "/dev/ttyAMA4")
            .unwrap()
            .set_default("serial.baud_rate", 9600)
            .unwrap()
            .set_default("serial.data_bits", 8)
            .unwrap()
            .set_default("serial.parity", "even")
            .unwrap()
            .set_default("serial.stop_bits", 1)
            .unwrap()
            .set_default("serial.flow_control", "hardware")
            .unwrap()
            .set_default("serial.timeout_secs", 1)
            .unwrap()
            .set_default("modbus.slave_id", 1)
            .unwrap()
            .set_default("modbus.channel_buffer_size", 24)
            .unwrap()
            .set_default("modbus.operation_timeout_secs", 5)
            .unwrap()
            .set_default("modbus.max_retries", 2)
            .unwrap()
            .set_default("modbus.initial_retry_delay_ms", 100)
            .unwrap()
            .set_default("modbus.request_timeout_secs", 10)
            .unwrap()
            .set_default("modbus.backoff_multiplier", 2.0)
            .unwrap()
            .set_default("modbus.max_consecutive_failures", 5)
            .unwrap()
            .set_default("modbus.inter_request_gap_ms", 10)
            .unwrap()
            .set_default("temperature_validation.min", 5.0)
            .unwrap()
            .set_default("temperature_validation.max", 30.0)
            .unwrap()
            .set_default("gpio.enabled", true)
            .unwrap()
            .set_default("gpio.gpio_k24", 20)
            .unwrap()
            .set_default("gpio.gpio_k25", 21)
            .unwrap()
            .set_default("gpio.active_low", false)
            .unwrap()
            .set_default("tibber.enabled", false)
            .unwrap()
            .set_default("tibber.api.token", None::<String>)
            .unwrap()
            .set_default("price.enabled", true)
            .unwrap()
            .set_default("price.zone", "SE3")
            .unwrap()
            .set_default("price.fetch_hour_local", 14)
            .unwrap()
            .set_default("heatpump_stats.enabled", true)
            .unwrap()
            .set_default("heatpump_stats.poll_interval_secs", 10)
            .unwrap()
            .set_default("heatpump_stats.persist_path", None::<String>)
            .unwrap()
            .set_default("smartgrid.auto_resume_enabled", true)
            .unwrap()
            .set_default("smartgrid.auto_resume_window_hours", 8)
            .unwrap()
            .set_default("storage.db_path", "./data/ctc.redb")
            .unwrap()
            .set_default("tz", "Europe/Stockholm")
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
}
