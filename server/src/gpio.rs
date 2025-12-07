//! GPIO-based `SmartGrid` relay control
//!
//! Controls K24/K25 terminals via GPIO pins connected to relay board.
//! Supports both reading current state and setting new states.

use gpiocdev::line::Value;
use gpiocdev::request::Request;
use tracing::{debug, error};

use crate::modbus::SmartGridMode;

/// GPIO controller for `SmartGrid` relays
#[derive(Clone)]
pub struct GpioController {
    gpio_k24: u32,
    gpio_k25: u32,
    active_low: bool,
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
        }
    }

    /// Read current `SmartGrid` mode from GPIO state
    ///
    /// # Errors
    /// Returns error if GPIO cannot be read
    pub fn read_mode(&self) -> Result<SmartGridMode, String> {
        let k24_closed = self.is_terminal_closed(self.gpio_k24)?;
        let k25_closed = self.is_terminal_closed(self.gpio_k25)?;

        let mode = SmartGridMode::from_terminals(k24_closed, k25_closed);
        debug!(
            "GPIO read: K24={}, K25={} -> {}",
            if k24_closed { "closed" } else { "open" },
            if k25_closed { "closed" } else { "open" },
            mode
        );

        Ok(mode)
    }

    /// Set `SmartGrid` mode by controlling GPIO relays
    ///
    /// # Errors
    /// Returns error if GPIO cannot be set
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

        Ok(())
    }

    /// Check if a terminal is closed by reading GPIO state
    fn is_terminal_closed(&self, gpio: u32) -> Result<bool, String> {
        let req = Request::builder()
            .on_chip("/dev/gpiochip0")
            .with_line(gpio)
            .as_input()
            .request()
            .map_err(|e| {
                error!("Failed to request GPIO {} for reading: {}", gpio, e);
                format!("Failed to request GPIO {gpio}: {e}")
            })?;

        let value = req.value(gpio).map_err(|e| {
            error!("Failed to read GPIO {}: {}", gpio, e);
            format!("Failed to read GPIO {gpio}: {e}")
        })?;

        let gpio_high = value == Value::Active;

        // Active-low: LOW = relay ON = terminal closed
        // Active-high: HIGH = relay ON = terminal closed
        Ok(if self.active_low {
            !gpio_high
        } else {
            gpio_high
        })
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
}
