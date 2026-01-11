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
    /// Price fetch interval in minutes (align with 15-min price periods)
    pub fetch_interval_mins: u64,
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

        // Add environment variables with CTC_ prefix
        // Example: CTC_SERVER_PORT=8080, CTC_SERIAL_BAUD_RATE=19200
        builder = builder.add_source(
            Environment::with_prefix("CTC")
                .separator("_")
                .try_parsing(true),
        );

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
            .set_default("price.fetch_interval_mins", 15)?;

        builder.build()?.try_deserialize()
    }
}

impl SerialConfig {
    /// Convert parity string to `tokio_serial::Parity`
    ///
    /// # Panics
    /// Panics if `parity` string is not "none", "even", or "odd"
    pub fn get_parity(&self) -> Parity {
        match self.parity.to_lowercase().as_str() {
            "none" => Parity::None,
            "even" => Parity::Even,
            "odd" => Parity::Odd,
            _ => panic!(
                "Invalid parity value: {}. Must be 'none', 'even', or 'odd'",
                self.parity
            ),
        }
    }

    /// Convert `data_bits` u8 to `tokio_serial::DataBits`
    ///
    /// # Panics
    /// Panics if `data_bits` is not 5, 6, 7, or 8
    pub fn get_data_bits(&self) -> DataBits {
        match self.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            8 => DataBits::Eight,
            _ => panic!(
                "Invalid data_bits value: {}. Must be 5, 6, 7, or 8",
                self.data_bits
            ),
        }
    }

    /// Convert `stop_bits` u8 to `tokio_serial::StopBits`
    ///
    /// # Panics
    /// Panics if `stop_bits` is not 1 or 2
    pub fn get_stop_bits(&self) -> StopBits {
        match self.stop_bits {
            1 => StopBits::One,
            2 => StopBits::Two,
            _ => panic!(
                "Invalid stop_bits value: {}. Must be 1 or 2",
                self.stop_bits
            ),
        }
    }

    /// Convert `flow_control` string to `tokio_serial::FlowControl`
    ///
    /// # Panics
    /// Panics if `flow_control` is not "none", "software", or "hardware"
    pub fn get_flow_control(&self) -> FlowControl {
        match self.flow_control.to_lowercase().as_str() {
            "none" => FlowControl::None,
            "software" => FlowControl::Software,
            "hardware" => FlowControl::Hardware,
            _ => panic!(
                "Invalid flow_control value: {}. Must be 'none', 'software', or 'hardware'",
                self.flow_control
            ),
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

        assert!(matches!(config.serial.get_parity(), Parity::Even));
        assert!(matches!(config.serial.get_data_bits(), DataBits::Eight));
        assert!(matches!(config.serial.get_stop_bits(), StopBits::One));
        assert!(matches!(
            config.serial.get_flow_control(),
            FlowControl::Hardware
        ));
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
            .set_default("price.fetch_interval_mins", 15)
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
}
