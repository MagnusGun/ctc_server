//! `SmartGrid` Relay Test Tool
//!
//! Tests GPIO relay control of CTC `SmartGrid` terminal blocks K25/K26.
//! Discovers relay mapping, measures response time when setting modes.

use clap::{Parser, Subcommand};
use gpiocdev::line::Value;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const GPIO_PIN_1: u32 = 20; // Pin 38
const GPIO_PIN_2: u32 = 21; // Pin 40
const DEFAULT_SERVER: &str = "http://localhost:3000";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const POLL_INTERVAL_MS: u64 = 100;

/// `SmartGrid` modes based on K25/K26 terminal states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmartGridMode {
    Normal,       // 0b00 - both open
    Blocking,     // 0b01 - K25 closed, K26 open
    LowPrice,     // 0b10 - K25 open, K26 closed
    Overcapacity, // 0b11 - both closed
}

impl SmartGridMode {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0b00 => Self::Normal,
            0b01 => Self::Blocking,
            0b10 => Self::LowPrice,
            0b11 => Self::Overcapacity,
            _ => unreachable!(),
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            Self::Normal => 0b00,
            Self::Blocking => 0b01,
            Self::LowPrice => 0b10,
            Self::Overcapacity => 0b11,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Blocking => "Blocking",
            Self::LowPrice => "LowPrice",
            Self::Overcapacity => "Overcapacity",
        }
    }

    /// Returns (`K25_closed`, `K26_closed`) for this mode
    fn terminal_states(self) -> (bool, bool) {
        match self {
            Self::Normal => (false, false),
            Self::Blocking => (true, false),
            Self::LowPrice => (false, true),
            Self::Overcapacity => (true, true),
        }
    }
}

impl std::fmt::Display for SmartGridMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Relay board configuration
#[derive(Debug, Clone)]
struct RelayConfig {
    /// True if relay activates on LOW signal (common for optocoupler modules)
    active_low: bool,
    /// GPIO pin that controls K25 (Smart A)
    gpio_k25: u32,
    /// GPIO pin that controls K26 (Smart B)
    gpio_k26: u32,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            active_low: false, // Active-HIGH: GPIO HIGH = relay ON = terminal closed
            gpio_k25: GPIO_PIN_1,
            gpio_k26: GPIO_PIN_2,
        }
    }
}

#[derive(Parser)]
#[command(name = "smartgrid_test")]
#[command(about = "SmartGrid relay test tool for CTC heating systems")]
struct Cli {
    /// CTC server URL
    #[arg(short, long, default_value = DEFAULT_SERVER)]
    server: String,

    /// Timeout in seconds for mode change detection
    #[arg(short, long, default_value_t = DEFAULT_TIMEOUT_SECS)]
    timeout: u64,

    #[command(subcommand)]
    command: Commands,
}

/// Relay state for CLI arguments
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum RelayState {
    On,
    Off,
}

#[derive(Subcommand)]
enum Commands {
    /// Query register 1100 and show current `SmartGrid` mode
    Status,
    /// Interactive mode to identify relay mapping and active-high/low
    Discover,
    /// Cycle all modes and measure response time for each transition
    Cycle,
    /// Manually set individual relay states
    Relay {
        /// Set K25 relay (Smart A): on = closed, off = open
        #[arg(long)]
        k25: Option<RelayState>,
        /// Set K26 relay (Smart B): on = closed, off = open
        #[arg(long)]
        k26: Option<RelayState>,
    },
    /// Set a specific `SmartGrid` mode
    Mode {
        /// Mode to set: normal, blocking, lowprice, overcapacity
        #[arg(value_parser = parse_mode)]
        mode: SmartGridMode,
    },
    /// Loop through all modes until Ctrl+C
    Loop {
        /// Delay between mode changes in seconds
        #[arg(short, long, default_value_t = 30)]
        delay: u64,
    },
}

fn parse_mode(s: &str) -> Result<SmartGridMode, String> {
    match s.to_lowercase().as_str() {
        "normal" => Ok(SmartGridMode::Normal),
        "blocking" | "block" => Ok(SmartGridMode::Blocking),
        "lowprice" | "low" => Ok(SmartGridMode::LowPrice),
        "overcapacity" | "high" | "highcap" => Ok(SmartGridMode::Overcapacity),
        _ => Err(format!(
            "Invalid mode '{s}'. Valid: normal, blocking, lowprice, overcapacity"
        )),
    }
}

#[derive(Deserialize)]
struct ApiResponse {
    ctc_data: f64,
}

/// Query `SmartGrid` mode from CTC server via generic API
fn query_smartgrid_register(server: &str) -> Result<u16, String> {
    let url = format!("{server}/api/v1/ctc?addr=1100&custom=true");
    let resp = reqwest::blocking::get(&url).map_err(|e| format!("HTTP request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server returned error: {}", resp.status()));
    }

    let api_resp: ApiResponse = resp
        .json()
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    // API returns f64 but Modbus register values are always 0-65535
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(api_resp.ctc_data as u16)
}

/// Extract `SmartGrid` mode bits (6-7) from register 1100
fn get_smartgrid_mode(server: &str) -> Result<SmartGridMode, String> {
    let raw = query_smartgrid_register(server)?;
    let bits = ((raw >> 6) & 0x03) as u8;
    Ok(SmartGridMode::from_bits(bits))
}

/// Heat pump status codes from register 62017
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum HeatPumpStatus {
    CompressorOffStartDelay = 0,
    CompressorOffReadyToStart = 1,
    CompressorWaitUntilFlow = 2,
    CompressorOnHeating = 3,
    DefrostActive = 4,
    CompressorOnCooling = 5,
    CompressorOffBlocked = 6,
    CompressorOffAlarm = 7,
    FunctionTest = 8,
    HpNotDefined = 30,
    CompressorNotEnabled = 31,
    CommunicationError = 32,
    ChargeDhw = 33,
    Unknown = 255,
}

impl From<u16> for HeatPumpStatus {
    fn from(val: u16) -> Self {
        match val {
            0 => Self::CompressorOffStartDelay,
            1 => Self::CompressorOffReadyToStart,
            2 => Self::CompressorWaitUntilFlow,
            3 => Self::CompressorOnHeating,
            4 => Self::DefrostActive,
            5 => Self::CompressorOnCooling,
            6 => Self::CompressorOffBlocked,
            7 => Self::CompressorOffAlarm,
            8 => Self::FunctionTest,
            30 => Self::HpNotDefined,
            31 => Self::CompressorNotEnabled,
            32 => Self::CommunicationError,
            33 => Self::ChargeDhw,
            _ => Self::Unknown,
        }
    }
}

impl std::fmt::Display for HeatPumpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CompressorOffStartDelay => write!(f, "Compressor off (start delay)"),
            Self::CompressorOffReadyToStart => write!(f, "Compressor off (ready to start)"),
            Self::CompressorWaitUntilFlow => write!(f, "Compressor wait until flow"),
            Self::CompressorOnHeating => write!(f, "Compressor on (heating)"),
            Self::DefrostActive => write!(f, "Defrost active"),
            Self::CompressorOnCooling => write!(f, "Compressor on (cooling)"),
            Self::CompressorOffBlocked => write!(f, "Compressor off (BLOCKED)"),
            Self::CompressorOffAlarm => write!(f, "Compressor off (alarm)"),
            Self::FunctionTest => write!(f, "Function test"),
            Self::HpNotDefined => write!(f, "HP not defined"),
            Self::CompressorNotEnabled => write!(f, "Compressor not enabled"),
            Self::CommunicationError => write!(f, "Communication error"),
            Self::ChargeDhw => write!(f, "Charge DHW"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Query heat pump status from register 62017
fn query_heatpump_status(server: &str) -> Result<HeatPumpStatus, String> {
    let url = format!("{server}/api/v1/ctc?addr=62017&custom=true");
    let resp = reqwest::blocking::get(&url).map_err(|e| format!("HTTP request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server returned error: {}", resp.status()));
    }

    let api_resp: ApiResponse = resp
        .json()
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(HeatPumpStatus::from(api_resp.ctc_data as u16))
}

/// Query `SmartGrid` mode directly from status register 62301 (`SGMode`)
/// Returns: 0=Normal, 1=Block, 2=LowPrice, 3=Overcapacity
fn query_sgmode(server: &str) -> Result<SmartGridMode, String> {
    let url = format!("{server}/api/v1/ctc?addr=62301&custom=true");
    let resp = reqwest::blocking::get(&url).map_err(|e| format!("HTTP request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Server returned error: {}", resp.status()));
    }

    let api_resp: ApiResponse = resp
        .json()
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let sgmode = api_resp.ctc_data as u8;

    // SGMode register values: 0=Normal, 1=Block, 2=LowPrice, 3=HighCap
    Ok(match sgmode {
        1 => SmartGridMode::Blocking,
        2 => SmartGridMode::LowPrice,
        3 => SmartGridMode::Overcapacity,
        _ => SmartGridMode::Normal, // 0 and unknown values treated as Normal
    })
}

/// Set a GPIO pin to the specified value
fn set_gpio(gpio: u32, high: bool) -> Result<(), String> {
    use gpiocdev::request::Request;

    let value = if high { Value::Active } else { Value::Inactive };

    let req = Request::builder()
        .on_chip("/dev/gpiochip0")
        .with_line(gpio)
        .as_output(value)
        .request()
        .map_err(|e| format!("Failed to request GPIO {gpio}: {e}"))?;

    req.set_value(gpio, value)
        .map_err(|e| format!("Failed to set GPIO {gpio}: {e}"))?;

    Ok(())
}

/// Read current GPIO output state
fn read_gpio(gpio: u32) -> Result<bool, String> {
    use gpiocdev::request::Request;

    let req = Request::builder()
        .on_chip("/dev/gpiochip0")
        .with_line(gpio)
        .as_input()
        .request()
        .map_err(|e| format!("Failed to request GPIO {gpio}: {e}"))?;

    let value = req
        .value(gpio)
        .map_err(|e| format!("Failed to read GPIO {gpio}: {e}"))?;

    Ok(value == Value::Active)
}

/// Set relays to achieve the desired `SmartGrid` mode
fn set_mode(config: &RelayConfig, mode: SmartGridMode) -> Result<(), String> {
    let (k25_closed, k26_closed) = mode.terminal_states();

    // For active-low: LOW = relay ON (closed), HIGH = relay OFF (open)
    // For active-high: HIGH = relay ON (closed), LOW = relay OFF (open)
    let gpio_k25_high = if config.active_low {
        !k25_closed
    } else {
        k25_closed
    };
    let gpio_k26_high = if config.active_low {
        !k26_closed
    } else {
        k26_closed
    };

    set_gpio(config.gpio_k25, gpio_k25_high)?;
    set_gpio(config.gpio_k26, gpio_k26_high)?;

    Ok(())
}

/// Wait for `SmartGrid` mode to change, return elapsed time
fn wait_for_mode(
    server: &str,
    target: SmartGridMode,
    timeout: Duration,
    verbose: bool,
) -> Result<Duration, String> {
    let start = Instant::now();
    let poll_interval = Duration::from_millis(POLL_INTERVAL_MS);

    loop {
        let current = get_smartgrid_mode(server)?;

        if verbose {
            println!(
                "  {:.1}s: 0x{:04X} ({})",
                start.elapsed().as_secs_f32(),
                u16::from(current.to_bits()) << 6,
                current
            );
        }

        if current == target {
            return Ok(start.elapsed());
        }

        if start.elapsed() > timeout {
            return Err(format!(
                "Timeout after {:.1}s waiting for {} mode",
                timeout.as_secs_f32(),
                target
            ));
        }

        thread::sleep(poll_interval);
    }
}

/// Status command: show current `SmartGrid` mode and GPIO states
fn cmd_status(server: &str) -> Result<(), String> {
    // Read SGMode status register (62301) - this reflects physical terminal state
    let sgmode = query_sgmode(server)?;
    println!("=== SmartGrid Status (Register 62301) ===");
    println!("SGMode: {} ({})", sgmode, sgmode.to_bits());

    // Read control register 1100 for reference
    let raw = query_smartgrid_register(server)?;
    let mode_from_ctrl = SmartGridMode::from_bits(((raw >> 6) & 0x03) as u8);
    println!("\n=== Control Register (1100) ===");
    println!("Register 1100: 0x{raw:04X}");
    println!(
        "BMS Mode: {} (bits 6-7 = 0b{:02b})",
        mode_from_ctrl,
        mode_from_ctrl.to_bits()
    );

    // Read heat pump status for reference
    let hp_status = query_heatpump_status(server)?;
    println!("\n=== Heat Pump Status (62017) ===");
    println!("HP Status: {hp_status}");

    // Try to read GPIO states
    println!("\n=== GPIO States ===");
    match (read_gpio(GPIO_PIN_1), read_gpio(GPIO_PIN_2)) {
        (Ok(g1), Ok(g2)) => {
            println!(
                "GPIO {} (Pin 38): {}",
                GPIO_PIN_1,
                if g1 { "HIGH" } else { "LOW" }
            );
            println!(
                "GPIO {} (Pin 40): {}",
                GPIO_PIN_2,
                if g2 { "HIGH" } else { "LOW" }
            );
        }
        _ => {
            println!("(Could not read GPIO states - may need root privileges)");
        }
    }

    Ok(())
}

/// Detect relay board logic level (active-high vs active-low) using `SGMode` register
fn detect_logic_level(server: &str) -> Result<Option<bool>, String> {
    println!("Step 1: Detecting relay board logic level (via SGMode register 62301)");

    // Set both GPIOs LOW first (default state = Normal for active-HIGH boards)
    println!("\nSetting GPIO {GPIO_PIN_1} = LOW, GPIO {GPIO_PIN_2} = LOW");
    set_gpio(GPIO_PIN_1, false)?;
    set_gpio(GPIO_PIN_2, false)?;
    thread::sleep(Duration::from_secs(15));

    let mode_low = query_sgmode(server)?;
    println!("  SGMode: {mode_low}");

    // Set both GPIOs HIGH
    println!("\nSetting GPIO {GPIO_PIN_1} = HIGH, GPIO {GPIO_PIN_2} = HIGH");
    set_gpio(GPIO_PIN_1, true)?;
    set_gpio(GPIO_PIN_2, true)?;
    thread::sleep(Duration::from_secs(15));

    let mode_high = query_sgmode(server)?;
    println!("  SGMode: {mode_high}");

    // Analyze results
    if mode_low == SmartGridMode::Overcapacity && mode_high == SmartGridMode::Normal {
        println!("\nResult: Relay board is ACTIVE-LOW");
        Ok(Some(true))
    } else if mode_low == SmartGridMode::Normal && mode_high == SmartGridMode::Overcapacity {
        println!("\nResult: Relay board is ACTIVE-HIGH");
        Ok(Some(false))
    } else {
        println!("\nCould not determine logic level from both-high/both-low test.");
        println!("Detected modes: LOW={mode_low}, HIGH={mode_high}");
        println!("Trying individual GPIO tests...");
        Ok(None)
    }
}

/// Test a single GPIO and determine which terminal it controls using `SGMode` register
fn test_gpio_terminal(
    server: &str,
    gpio: u32,
    other_gpio: u32,
    relay_on: bool,
) -> Result<Option<bool>, String> {
    println!(
        "\nTesting GPIO {} alone (set to {})...",
        gpio,
        if relay_on { "HIGH" } else { "LOW" }
    );
    set_gpio(gpio, relay_on)?;
    set_gpio(other_gpio, !relay_on)?;
    thread::sleep(Duration::from_secs(15));

    let mode = query_sgmode(server)?;
    println!("  SGMode: {mode}");

    match mode {
        SmartGridMode::Blocking => {
            println!("  GPIO {gpio} controls K25 (Smart A)");
            Ok(Some(true)) // true = K25
        }
        SmartGridMode::LowPrice => {
            println!("  GPIO {gpio} controls K26 (Smart B)");
            Ok(Some(false)) // false = K26
        }
        _ => {
            println!("  Could not determine terminal for GPIO {gpio}");
            Ok(None)
        }
    }
}

/// Print discovery summary
fn print_discovery_summary(
    detected_active_low: Option<bool>,
    gpio_for_k25: Option<u32>,
    gpio_for_k26: Option<u32>,
) {
    println!("\n=== Discovery Complete ===");

    if let Some(active_low) = detected_active_low {
        println!(
            "Relay board: {}",
            if active_low {
                "ACTIVE-LOW"
            } else {
                "ACTIVE-HIGH"
            }
        );
    } else {
        println!("Relay board logic: UNDETERMINED");
    }

    if let (Some(k25), Some(k26)) = (gpio_for_k25, gpio_for_k26) {
        println!(
            "GPIO {} (Pin {}) -> K25 (Smart A)",
            k25,
            if k25 == 20 { 38 } else { 40 }
        );
        println!(
            "GPIO {} (Pin {}) -> K26 (Smart B)",
            k26,
            if k26 == 20 { 38 } else { 40 }
        );
    } else {
        println!("\nWarning: Could not fully determine GPIO mapping.");
        println!("Please check:");
        println!("  - Relay wiring to CTC terminals K25/K26");
        println!("  - Power to relay board");
        println!("  - CTC SmartGrid feature is enabled");
    }
}

/// Discover command: determine relay mapping and active-high/low
fn cmd_discover(server: &str, _timeout: Duration) -> Result<(), String> {
    println!("=== SmartGrid Relay Discovery ===\n");
    println!("Using SGMode register (62301) for detection:");
    println!("  0=Normal, 1=Blocking, 2=LowPrice, 3=Overcapacity\n");

    // Set GPIOs to LOW first (Normal mode for active-HIGH boards)
    println!("Setting GPIOs to LOW (default Normal state)...");
    set_gpio(GPIO_PIN_1, false)?;
    set_gpio(GPIO_PIN_2, false)?;
    thread::sleep(Duration::from_secs(15));

    // Read initial state
    let initial_mode = query_sgmode(server)?;
    println!("Initial SGMode: {initial_mode}\n");

    // Detect logic level
    let detected_active_low = detect_logic_level(server)?;

    // Map GPIOs to terminals
    println!("\nStep 2: Mapping GPIOs to CTC terminals");

    let active_low = detected_active_low.unwrap_or(false); // Default to active-HIGH
    let relay_on = !active_low;

    let mut gpio_for_k25: Option<u32> = None;
    let mut gpio_for_k26: Option<u32> = None;

    // Test GPIO_PIN_1
    if let Some(is_k25) = test_gpio_terminal(server, GPIO_PIN_1, GPIO_PIN_2, relay_on)? {
        if is_k25 {
            gpio_for_k25 = Some(GPIO_PIN_1);
        } else {
            gpio_for_k26 = Some(GPIO_PIN_1);
        }
    }

    // Test GPIO_PIN_2
    if let Some(is_k25) = test_gpio_terminal(server, GPIO_PIN_2, GPIO_PIN_1, relay_on)? {
        if is_k25 {
            gpio_for_k25 = Some(GPIO_PIN_2);
        } else {
            gpio_for_k26 = Some(GPIO_PIN_2);
        }
    }

    // Reset to Normal mode (GPIOs LOW for active-HIGH board)
    println!("\nResetting to Normal mode (GPIOs LOW)...");
    set_gpio(GPIO_PIN_1, false)?;
    set_gpio(GPIO_PIN_2, false)?;

    print_discovery_summary(detected_active_low, gpio_for_k25, gpio_for_k26);

    Ok(())
}

/// Cycle command: test all mode transitions and measure timing
fn cmd_cycle(server: &str, timeout: Duration) -> Result<(), String> {
    println!("Testing all SmartGrid mode transitions:\n");

    // Use default config (can be enhanced to load discovered config)
    let config = RelayConfig::default();

    // First ensure we're in Normal mode
    set_mode(&config, SmartGridMode::Normal)?;
    thread::sleep(Duration::from_secs(2));

    let initial = get_smartgrid_mode(server)?;
    if initial != SmartGridMode::Normal {
        println!("Warning: Could not set Normal mode (current: {initial})");
        println!("Proceeding anyway...\n");
    }

    let transitions = [
        (SmartGridMode::Normal, SmartGridMode::Blocking),
        (SmartGridMode::Blocking, SmartGridMode::LowPrice),
        (SmartGridMode::LowPrice, SmartGridMode::Overcapacity),
        (SmartGridMode::Overcapacity, SmartGridMode::Normal),
    ];

    let mut times: Vec<Duration> = Vec::new();
    let mut all_success = true;

    for (from, to) in transitions {
        print!("{from} -> {to}: ");

        // Set the target mode
        set_mode(&config, to)?;

        // Wait for CTC to report the new mode
        match wait_for_mode(server, to, timeout, false) {
            Ok(elapsed) => {
                println!("{:.2}s OK", elapsed.as_secs_f32());
                times.push(elapsed);
            }
            Err(e) => {
                println!("FAILED ({e})");
                all_success = false;
            }
        }

        // Small delay between transitions
        thread::sleep(Duration::from_millis(500));
    }

    println!();

    if !times.is_empty() {
        // At most 4 transitions, precision loss is negligible
        #[allow(clippy::cast_precision_loss)]
        let avg: f32 = times
            .iter()
            .map(std::time::Duration::as_secs_f32)
            .sum::<f32>()
            / times.len() as f32;
        println!("Average response time: {avg:.2}s");
    }

    if all_success {
        println!("All transitions successful.");
    } else {
        println!("Some transitions failed.");
    }

    Ok(())
}

/// Relay command: manually set individual relay states
fn cmd_relay(server: &str, k25: Option<RelayState>, k26: Option<RelayState>) -> Result<(), String> {
    if k25.is_none() && k26.is_none() {
        return Err("At least one of --k25 or --k26 must be specified".to_string());
    }

    // Active-HIGH: GPIO HIGH = relay ON = terminal closed
    if let Some(state) = k25 {
        let high = state == RelayState::On;
        set_gpio(GPIO_PIN_1, high)?;
        println!(
            "K25 (GPIO {}): {} -> terminal {}",
            GPIO_PIN_1,
            if high { "HIGH" } else { "LOW" },
            if high { "CLOSED" } else { "OPEN" }
        );
    }

    if let Some(state) = k26 {
        let high = state == RelayState::On;
        set_gpio(GPIO_PIN_2, high)?;
        println!(
            "K26 (GPIO {}): {} -> terminal {}",
            GPIO_PIN_2,
            if high { "HIGH" } else { "LOW" },
            if high { "CLOSED" } else { "OPEN" }
        );
    }

    // Wait a moment and show current SGMode
    thread::sleep(Duration::from_secs(2));
    let sgmode = query_sgmode(server)?;
    println!("\nSGMode (62301): {sgmode}");

    Ok(())
}

/// Mode command: set a specific `SmartGrid` mode
fn cmd_mode_set(server: &str, mode: SmartGridMode) -> Result<(), String> {
    let (k25_closed, k26_closed) = mode.terminal_states();

    // Active-HIGH: GPIO HIGH = relay ON = terminal closed
    set_gpio(GPIO_PIN_1, k25_closed)?;
    set_gpio(GPIO_PIN_2, k26_closed)?;

    println!(
        "Setting mode: {} (K25={}, K26={})",
        mode,
        if k25_closed { "CLOSED" } else { "OPEN" },
        if k26_closed { "CLOSED" } else { "OPEN" }
    );

    // Wait and verify
    thread::sleep(Duration::from_secs(2));
    let sgmode = query_sgmode(server)?;
    println!("SGMode (62301): {sgmode}");

    if sgmode == mode {
        println!("Mode set successfully!");
    } else {
        println!("Warning: Expected {mode}, got {sgmode}");
    }

    Ok(())
}

/// Loop command: cycle through all modes until Ctrl+C
fn cmd_loop(server: &str, delay_secs: u64) -> Result<(), String> {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // Set up Ctrl+C handler
    ctrlc::set_handler(move || {
        println!("\n\nCtrl+C received, resetting to Normal mode...");
        r.store(false, Ordering::SeqCst);
    })
    .map_err(|e| format!("Failed to set Ctrl+C handler: {e}"))?;

    let modes = [
        SmartGridMode::Normal,
        SmartGridMode::Blocking,
        SmartGridMode::LowPrice,
        SmartGridMode::Overcapacity,
    ];

    println!("Looping through SmartGrid modes ({delay_secs}s delay)");
    println!("Press Ctrl+C to stop and reset to Normal mode\n");

    let mut cycle = 0u32;
    while running.load(Ordering::SeqCst) {
        for mode in &modes {
            if !running.load(Ordering::SeqCst) {
                break;
            }

            cycle += 1;
            let (k25_closed, k26_closed) = mode.terminal_states();

            // Set the mode
            set_gpio(GPIO_PIN_1, k25_closed)?;
            set_gpio(GPIO_PIN_2, k26_closed)?;

            println!(
                "[{}] {} (K25={}, K26={})",
                cycle,
                mode,
                if k25_closed { "ON" } else { "OFF" },
                if k26_closed { "ON" } else { "OFF" }
            );

            // Wait a moment for CTC to update
            thread::sleep(Duration::from_secs(2));

            // Read and display SGMode
            match query_sgmode(server) {
                Ok(sgmode) => {
                    print!("    SGMode: {sgmode}");
                    if sgmode == *mode {
                        println!(" ✓");
                    } else {
                        println!(" (expected {mode})");
                    }
                }
                Err(e) => println!("    SGMode: Error - {e}"),
            }

            // Wait for the remaining delay
            if delay_secs > 2 {
                for _ in 0..(delay_secs - 2) {
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    // Reset to Normal mode on exit
    set_gpio(GPIO_PIN_1, false)?;
    set_gpio(GPIO_PIN_2, false)?;
    println!("Reset to Normal mode (both GPIOs LOW)");

    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let timeout = Duration::from_secs(cli.timeout);

    let result = match cli.command {
        Commands::Status => cmd_status(&cli.server),
        Commands::Discover => cmd_discover(&cli.server, timeout),
        Commands::Cycle => cmd_cycle(&cli.server, timeout),
        Commands::Relay { k25, k26 } => cmd_relay(&cli.server, k25, k26),
        Commands::Mode { mode } => cmd_mode_set(&cli.server, mode),
        Commands::Loop { delay } => cmd_loop(&cli.server, delay),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
