//! GPIO-based `SmartGrid` relay control
//!
//! Controls K24/K25 terminals via GPIO pins connected to relay board.
//! Tracks current mode in memory to avoid reading GPIO state (which would
//! change pin direction from output to input).
//!
//! The line request FD is acquired once at construction and held for the
//! lifetime of the controller. Dropping the `Request` would close the FD and
//! the kernel would release the lines back to high-Z input, so e.g. the
//! `Blocking` relay would drop out the instant `set_mode` returned. Holding
//! the request keeps the lines driven.

use std::sync::Arc;
use std::time::SystemTime;

use gpiocdev::line::{Value, Values};
use gpiocdev::request::Request;
use tracing::{debug, error, info};

use super::mode::SmartGridMode;

/// GPIO controller for `SmartGrid` relays.
///
/// Owned by [`super::actor::SmartGridActor`] and never shared: every command
/// arrives via the actor's mpsc channel, so the actor task is the only
/// caller. That's why mode bookkeeping is held as plain fields rather than
/// behind `Arc<Mutex<…>>` — Rust's `&mut self` borrow on the actor side gives
/// us all the mutual exclusion we need.
pub struct GpioController {
    gpio_k24: u32,
    gpio_k25: u32,
    active_low: bool,
    /// Persistent line request. Holding this keeps the K24 + K25 lines
    /// acquired (output direction, last-written value driven). `None` only
    /// in unit tests that exercise bookkeeping without hardware.
    request: Option<Arc<Request>>,
    /// Current mode stored in memory (avoids reading GPIO which changes pin direction)
    current_mode: SmartGridMode,
    /// Timestamp when mode was last changed (None if never changed since startup)
    mode_changed_at: Option<SystemTime>,
    /// Monotonic counter bumped on every manual mode change. Snapshotted by
    /// the resume timer; re-checked inside `set_mode_if_not_superseded` so a
    /// manual override that races a waking timer is detected.
    /// Belt-and-suspenders given the actor already serialises commands.
    mode_generation: u64,
    /// Test-only seam: when true, `set_mode` updates in-memory bookkeeping
    /// without issuing the hardware ioctl, letting tests drive the actor's
    /// post-set scheduling paths. Always false in production
    /// (`request` is `Some`, so this flag is never consulted there).
    #[cfg(test)]
    test_accept_writes: bool,
}

impl GpioController {
    /// Create a new GPIO controller and acquire the K24/K25 lines.
    ///
    /// Both lines are requested as outputs in a single kernel `Request`,
    /// initialised to the levels representing `SmartGridMode::Normal`
    /// (both terminals open). The request FD is held for the lifetime of
    /// the controller; dropping it would release the lines back to the
    /// kernel default.
    ///
    /// # Arguments
    /// * `gpio_k24` - GPIO pin number for K24 (Smart A) terminal
    /// * `gpio_k25` - GPIO pin number for K25 (Smart B) terminal
    /// * `active_low` - True if relay board uses active-low logic (LOW = relay ON)
    ///
    /// # Errors
    /// Returns an error string if `/dev/gpiochip0` is unavailable or the
    /// requested lines are already claimed by another process.
    pub fn new(gpio_k24: u32, gpio_k25: u32, active_low: bool) -> Result<Self, String> {
        debug!(
            "GpioController acquiring: K24=GPIO{}, K25=GPIO{}, active_low={}",
            gpio_k24, gpio_k25, active_low
        );

        // Normal mode = both terminals open. Active-low → open is HIGH;
        // active-high → open is LOW. Both lines share this initial level.
        let initial_value = if active_low {
            Value::Active
        } else {
            Value::Inactive
        };

        let request = Request::builder()
            .on_chip("/dev/gpiochip0")
            .with_lines(&[gpio_k24, gpio_k25])
            .as_output(initial_value)
            .request()
            .map_err(|e| {
                error!("Failed to acquire GPIO lines K24={gpio_k24}/K25={gpio_k25}: {e}");
                format!("Failed to acquire GPIO lines K24={gpio_k24}/K25={gpio_k25}: {e}")
            })?;

        Ok(Self {
            gpio_k24,
            gpio_k25,
            active_low,
            request: Some(Arc::new(request)),
            current_mode: SmartGridMode::Normal,
            mode_changed_at: None,
            mode_generation: 0,
            #[cfg(test)]
            test_accept_writes: false,
        })
    }

    /// Test-only constructor that skips hardware acquisition. Bookkeeping
    /// methods (`read_mode`, `mode_changed_at`, …) work normally; `set_mode`
    /// errors because no line request is held.
    #[cfg(test)]
    pub fn new_for_test(gpio_k24: u32, gpio_k25: u32, active_low: bool) -> Self {
        Self {
            gpio_k24,
            gpio_k25,
            active_low,
            request: None,
            current_mode: SmartGridMode::Normal,
            mode_changed_at: None,
            mode_generation: 0,
            test_accept_writes: false,
        }
    }

    /// Test-only constructor whose `set_mode` succeeds in memory without any
    /// hardware ioctl. Lets tests drive the actor's post-set scheduling,
    /// resume-timer, and cancellation paths. Production never uses this.
    #[cfg(test)]
    pub fn new_for_test_accepting(gpio_k24: u32, gpio_k25: u32, active_low: bool) -> Self {
        Self {
            gpio_k24,
            gpio_k25,
            active_low,
            request: None,
            current_mode: SmartGridMode::Normal,
            mode_changed_at: None,
            mode_generation: 0,
            test_accept_writes: true,
        }
    }

    /// Snapshot the current mode-generation counter. The scheduled resume task
    /// calls this when it spawns; if the value has changed by the time the
    /// task is about to mutate state, the task is considered superseded by a
    /// manual override and must not run.
    #[must_use]
    pub fn mode_generation(&self) -> u64 {
        self.mode_generation
    }

    /// Bump the mode-generation counter. Called by `apply_mode` before any
    /// state mutation so that an in-flight resume task (whose `AbortHandle`
    /// may not have pre-empted it because it has already passed its `.await`
    /// point) can detect supersession and bail.
    pub fn bump_mode_generation(&mut self) {
        self.mode_generation += 1;
    }

    /// Apply `Normal` mode from the scheduled resume task only if no manual
    /// override has happened since `expected_generation` was captured.
    ///
    /// Returns `Ok(true)` if the mode was applied, `Ok(false)` if the task
    /// was superseded and no mutation happened.
    ///
    /// # Errors
    /// Returns error if GPIO cannot be set.
    pub fn set_mode_if_not_superseded(
        &mut self,
        mode: SmartGridMode,
        expected_generation: u64,
    ) -> Result<bool, String> {
        if self.mode_generation != expected_generation {
            info!(
                "Auto-resume superseded by manual mode change — skipping flip to {}",
                mode
            );
            return Ok(false);
        }
        self.set_mode(mode).map(|()| true)
    }

    /// Read current `SmartGrid` mode from memory
    ///
    /// Returns the last mode set via `set_mode()`. We track mode in memory
    /// rather than reading GPIO because reading would require changing pin
    /// direction from output to input, which loses the output state.
    #[must_use]
    pub fn read_mode(&self) -> SmartGridMode {
        debug!("GPIO read_mode: {} (from memory)", self.current_mode);
        self.current_mode
    }

    /// Get timestamp when mode was last changed
    ///
    /// Returns `None` if mode has never been changed since server startup.
    #[must_use]
    pub fn mode_changed_at(&self) -> Option<SystemTime> {
        self.mode_changed_at
    }

    /// Set `SmartGrid` mode by driving the held K24/K25 line request.
    ///
    /// Both lines are written in a single `set_values` ioctl so transitions
    /// are atomic — there's no transient intermediate state visible to the
    /// heat pump between K24 and K25 updates.
    ///
    /// # Errors
    /// Returns error if GPIO cannot be set or the controller was built
    /// without a hardware request (test-only).
    pub fn set_mode(&mut self, mode: SmartGridMode) -> Result<(), String> {
        let (k24_closed, k25_closed) = mode.terminal_states();

        debug!(
            "GPIO set: {} -> K24={}, K25={}",
            mode,
            if k24_closed { "closed" } else { "open" },
            if k25_closed { "closed" } else { "open" }
        );

        // Test seam: a test controller built via `new_for_test_accepting`
        // skips the ioctl and just updates bookkeeping below. Production
        // always holds a `request`, so this branch is dead there.
        #[cfg(test)]
        let skip_hardware = self.test_accept_writes;
        #[cfg(not(test))]
        let skip_hardware = false;

        if !skip_hardware {
            let request = self
                .request
                .as_ref()
                .ok_or_else(|| "GPIO request not initialised (test-only controller)".to_string())?;

            let mut values = Values::default();
            values
                .set(self.gpio_k24, self.gpio_level(k24_closed))
                .set(self.gpio_k25, self.gpio_level(k25_closed));

            request.set_values(&values).map_err(|e| {
                error!(
                    "Failed to write GPIO K24={}/K25={}: {e}",
                    self.gpio_k24, self.gpio_k25
                );
                format!(
                    "Failed to write GPIO K24={}/K25={}: {e}",
                    self.gpio_k24, self.gpio_k25
                )
            })?;
        }

        if self.current_mode != mode {
            self.mode_changed_at = Some(SystemTime::now());
        }
        self.current_mode = mode;
        Ok(())
    }

    /// Convert a logical terminal state ("closed" / "open") to the GPIO
    /// level required by the relay board:
    ///   active-low  → closed = LOW,  open = HIGH
    ///   active-high → closed = HIGH, open = LOW
    fn gpio_level(&self, closed: bool) -> Value {
        let gpio_high = if self.active_low { !closed } else { closed };
        if gpio_high {
            Value::Active
        } else {
            Value::Inactive
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpio_controller_creation() {
        let controller = GpioController::new_for_test(20, 21, true);
        assert_eq!(controller.gpio_k24, 20);
        assert_eq!(controller.gpio_k25, 21);
        assert!(controller.active_low);
    }

    #[test]
    fn test_gpio_controller_initial_mode_is_normal() {
        let controller = GpioController::new_for_test(20, 21, false);
        // read_mode() returns from memory, initialized to Normal
        assert!(matches!(controller.read_mode(), SmartGridMode::Normal));
    }

    #[test]
    fn test_set_mode_without_hardware_errors() {
        let mut controller = GpioController::new_for_test(20, 21, false);
        let err = controller.set_mode(SmartGridMode::Blocking).unwrap_err();
        assert!(err.contains("not initialised"));
    }

    #[test]
    fn test_accepting_controller_set_mode_updates_bookkeeping() {
        // The accepting seam must behave like a successful hardware write:
        // mode is recorded and the changed-at timestamp is stamped.
        let mut controller = GpioController::new_for_test_accepting(20, 21, false);
        assert!(matches!(controller.read_mode(), SmartGridMode::Normal));
        assert!(controller.mode_changed_at().is_none());

        controller.set_mode(SmartGridMode::Blocking).unwrap();
        assert!(matches!(controller.read_mode(), SmartGridMode::Blocking));
        assert!(controller.mode_changed_at().is_some());
    }

    #[test]
    fn test_accepting_controller_same_mode_keeps_timestamp() {
        // Re-applying the current mode must not refresh mode_changed_at
        // (mirrors the production guard `if self.current_mode != mode`).
        let mut controller = GpioController::new_for_test_accepting(20, 21, false);
        controller.set_mode(SmartGridMode::Blocking).unwrap();
        let first = controller.mode_changed_at().unwrap();
        controller.set_mode(SmartGridMode::Blocking).unwrap();
        assert_eq!(controller.mode_changed_at().unwrap(), first);
    }
}
