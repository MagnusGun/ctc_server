//! GPIO-based `SmartGrid` relay control
//!
//! Controls K24/K25 terminals via GPIO pins connected to relay board.
//! Tracks current mode in memory to avoid reading GPIO state (which would
//! change pin direction from output to input).

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use gpiocdev::line::Value;
use gpiocdev::request::Request;
use tracing::{debug, error};

use super::mode::SmartGridMode;

/// GPIO controller for `SmartGrid` relays
#[derive(Clone)]
pub struct GpioController {
    gpio_k24: u32,
    gpio_k25: u32,
    active_low: bool,
    /// Current mode stored in memory (avoids reading GPIO which changes pin direction)
    current_mode: Arc<Mutex<SmartGridMode>>,
    /// Timestamp when mode was last changed (None if never changed since startup)
    mode_changed_at: Arc<Mutex<Option<SystemTime>>>,
}

impl GpioController {
    /// Create a new GPIO controller
    ///
    /// # Arguments
    /// * `gpio_k24` - GPIO pin number for K24 (Smart A) terminal
    /// * `gpio_k25` - GPIO pin number for K25 (Smart B) terminal
    /// * `active_low` - True if relay board uses active-low logic (LOW = relay ON)
    #[must_use]
    pub fn new(gpio_k24: u32, gpio_k25: u32, active_low: bool) -> Self {
        debug!(
            "GpioController created: K24=GPIO{}, K25=GPIO{}, active_low={}",
            gpio_k24, gpio_k25, active_low
        );
        Self {
            gpio_k24,
            gpio_k25,
            active_low,
            current_mode: Arc::new(Mutex::new(SmartGridMode::Normal)),
            mode_changed_at: Arc::new(Mutex::new(None)),
        }
    }

    /// Read current `SmartGrid` mode from memory
    ///
    /// Returns the last mode set via `set_mode()`. We track mode in memory
    /// rather than reading GPIO because reading would require changing pin
    /// direction from output to input, which loses the output state.
    ///
    /// # Errors
    /// Returns error if the mutex is poisoned
    pub fn read_mode(&self) -> Result<SmartGridMode, String> {
        let mode = *self
            .current_mode
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;
        debug!("GPIO read_mode: {} (from memory)", mode);
        Ok(mode)
    }

    /// Get timestamp when mode was last changed
    ///
    /// Returns `None` if mode has never been changed since server startup.
    ///
    /// # Errors
    /// Returns error if the mutex is poisoned
    pub fn mode_changed_at(&self) -> Result<Option<SystemTime>, String> {
        let timestamp = *self
            .mode_changed_at
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;
        Ok(timestamp)
    }

    /// Set `SmartGrid` mode by controlling GPIO relays
    ///
    /// # Errors
    /// Returns error if GPIO cannot be set or mutex is poisoned
    pub fn set_mode(&self, mode: SmartGridMode) -> Result<(), String> {
        let (k24_closed, k25_closed) = mode.terminal_states();

        debug!(
            "GPIO set: {} -> K24={}, K25={}",
            mode,
            if k24_closed { "closed" } else { "open" },
            if k25_closed { "closed" } else { "open" }
        );

        self.set_terminal(self.gpio_k24, k24_closed)?;
        self.set_terminal(self.gpio_k25, k25_closed)?;

        // Check if mode actually changed before updating timestamp
        let current = *self
            .current_mode
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;

        if current != mode {
            *self
                .mode_changed_at
                .lock()
                .map_err(|e| format!("Lock poisoned: {e}"))? = Some(SystemTime::now());
        }

        // Store the mode in memory for read_mode()
        *self
            .current_mode
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))? = mode;

        Ok(())
    }

    /// Set a terminal state by controlling GPIO
    fn set_terminal(&self, gpio: u32, closed: bool) -> Result<(), String> {
        // Convert desired terminal state to GPIO level
        // Active-low: closed -> LOW, open -> HIGH
        // Active-high: closed -> HIGH, open -> LOW
        let gpio_high = if self.active_low { !closed } else { closed };
        let value = if gpio_high {
            Value::Active
        } else {
            Value::Inactive
        };

        let req = Request::builder()
            .on_chip("/dev/gpiochip0")
            .with_line(gpio)
            .as_output(value)
            .request()
            .map_err(|e| {
                error!("Failed to request GPIO {} for output: {}", gpio, e);
                format!("Failed to set GPIO {gpio}: {e}")
            })?;

        req.set_value(gpio, value).map_err(|e| {
            error!("Failed to write GPIO {}: {}", gpio, e);
            format!("Failed to write GPIO {gpio}: {e}")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpio_controller_creation() {
        let controller = GpioController::new(20, 21, true);
        assert_eq!(controller.gpio_k24, 20);
        assert_eq!(controller.gpio_k25, 21);
        assert!(controller.active_low);
    }

    #[test]
    fn test_gpio_controller_clone() {
        let controller = GpioController::new(20, 21, false);
        let cloned = controller.clone();
        assert_eq!(cloned.gpio_k24, controller.gpio_k24);
        assert_eq!(cloned.gpio_k25, controller.gpio_k25);
        assert_eq!(cloned.active_low, controller.active_low);
    }

    #[test]
    fn test_gpio_controller_initial_mode_is_normal() {
        let controller = GpioController::new(20, 21, false);
        // read_mode() returns from memory, initialized to Normal
        let mode = controller.read_mode().unwrap();
        assert!(matches!(mode, SmartGridMode::Normal));
    }

    #[test]
    fn test_gpio_controller_clone_shares_mode() {
        let controller = GpioController::new(20, 21, false);
        let cloned = controller.clone();
        // Clones share the same Arc<Mutex<SmartGridMode>>
        // Initial mode should be Normal for both
        assert!(matches!(
            controller.read_mode().unwrap(),
            SmartGridMode::Normal
        ));
        assert!(matches!(cloned.read_mode().unwrap(), SmartGridMode::Normal));
    }
}
