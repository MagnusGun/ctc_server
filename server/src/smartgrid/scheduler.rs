//! Auto-resume scheduling for `SmartGrid` non-Normal modes.
//!
//! When the user puts the heater into a non-Normal mode (typically via the
//! dashboard or the API), `apply_mode` consults the electricity price feed
//! and — if asked — schedules a one-shot task that flips the heater back to
//! `Normal` at a per-mode target time inside the configured horizon:
//!
//! - **Blocking** (defer heating): resume at the start of the cheapest
//!   15-minute slot — i.e., let the heater catch up when prices are best.
//! - **`LowPrice` / `Overcapacity`** (buffer extra heat now while cheap):
//!   resume at the moment the cheap window ends — i.e., stop boosting
//!   when prices return to normal.
//!
//! Cancellation: any subsequent mode change first calls
//! `cancel_scheduled_resume()`, so a stale task can never fire after the user
//! has changed their mind. The `DELETE /api/v1/smartgrid/scheduled_resume`
//! endpoint clears the schedule without touching the mode.

use std::time::{Duration, SystemTime};

use chrono::DateTime;
use tracing::{error, info, warn};

use crate::config::SmartGridConfig;
use crate::energy::price::PriceState;
use crate::smartgrid::{GpioController, SmartGridMode};

/// Errors `apply_mode` can surface to its caller.
#[derive(Debug)]
pub enum ApplyModeError {
    Gpio(String),
}

impl std::fmt::Display for ApplyModeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gpio(e) => write!(f, "GPIO error: {e}"),
        }
    }
}

/// Apply a `SmartGrid` mode and, when entering a non-Normal mode with a
/// resume requested, schedule the auto-resume task.
///
/// Per-mode resume target:
/// - `Blocking` → start of the cheapest slot inside the window.
/// - `LowPrice` / `Overcapacity` → start of the first non-cheap slot, or end
///   of the window if the cheap run extends past it.
/// - `Normal` → never schedules.
///
/// Returns the scheduled `fires_at` so handlers can include it in their JSON
/// response, or `None` when no schedule was set (mode was Normal, resume not
/// requested, feature disabled, or no usable price data inside the window).
pub fn apply_mode(
    gpio: &GpioController,
    mode: SmartGridMode,
    schedule_resume: bool,
    price_state: &PriceState,
    config: &SmartGridConfig,
) -> Result<Option<SystemTime>, ApplyModeError> {
    // Always cancel any prior schedule before mutating: a manual change
    // overrides any pending auto-flip.
    gpio.cancel_scheduled_resume();
    gpio.set_mode(mode).map_err(ApplyModeError::Gpio)?;

    if mode == SmartGridMode::Normal || !schedule_resume || !config.auto_resume_enabled {
        return Ok(None);
    }

    let window = Duration::from_secs(config.auto_resume_window_hours.saturating_mul(3600));
    let fires_at = match mode {
        SmartGridMode::Blocking => price_state
            .cheapest_within(window)
            .and_then(|slot| parse_rfc3339_to_system_time(&slot.starts_at)),
        SmartGridMode::LowPrice | SmartGridMode::Overcapacity => {
            price_state.cheap_window_end(window)
        }
        SmartGridMode::Normal => unreachable!("returned above"),
    };

    let Some(fires_at) = fires_at else {
        warn!(
            "Auto-resume: no resume target for mode {mode} within {}h — heater stays in {mode}",
            config.auto_resume_window_hours
        );
        return Ok(None);
    };

    let gpio_for_task = gpio.clone();
    let handle = tokio::spawn(run_resume_task(gpio_for_task, fires_at)).abort_handle();
    gpio.set_scheduled_resume(fires_at, handle);

    info!("Auto-resume scheduled for {:?} (mode={mode})", fires_at);
    Ok(Some(fires_at))
}

/// Wait until `fires_at`, then flip the heater back to `Normal`.
async fn run_resume_task(gpio: GpioController, fires_at: SystemTime) {
    if let Ok(delay) = fires_at.duration_since(SystemTime::now()) {
        tokio::time::sleep(delay).await;
    }
    if let Err(e) = gpio.set_mode(SmartGridMode::Normal) {
        error!("Auto-resume: failed to set Normal: {e}");
        return;
    }
    gpio.clear_resume_if_matches(fires_at);
    info!("Auto-resume fired — heater set back to Normal");
}

fn parse_rfc3339_to_system_time(s: &str) -> Option<SystemTime> {
    DateTime::parse_from_rfc3339(s).ok().map(SystemTime::from)
}
