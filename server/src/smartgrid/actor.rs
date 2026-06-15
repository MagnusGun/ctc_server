//! `SmartGrid` actor.
//!
//! Owns the [`GpioController`] and all in-memory mode bookkeeping (current
//! mode, last-changed timestamp, pending auto-resume) in a single async
//! task. Every external request — read or write — is funneled through one
//! mpsc channel, so commands run serially. That gives mutual exclusion for
//! the `bump → cancel → set → schedule` sequence by construction; no
//! [`tokio::sync::Mutex`] needed.
//!
//! ### Shutdown
//! The actor selects on both `rx.recv()` and `cancel.cancelled()`. On
//! cancellation it aborts any pending resume timer and exits. Its
//! `JoinHandle` is in `main`'s `background_tasks` vector so the
//! graceful-shutdown 5 s window waits for it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::SmartGridConfig;
use crate::dhw::actor::ModbusWriter;
use crate::energy::price::PriceState;
use crate::energy::tibber::parse_iso8601;
use crate::homey::HomeyClient;
use crate::homey::cache::HomeyPumpCache;

use super::gpio::GpioController;
use super::heatup::WarmByCommand;
use super::heatup_watcher::run_heatup_watcher;
use super::mode::SmartGridMode;

/// Side-channel for slaving the Cirkulationspump to `SmartGrid` mode via Homey.
///
/// Optional: when the user has not configured `[homey].enabled = true`, the
/// actor stores `None` and the pump-push helpers are zero-cost no-ops.
#[derive(Clone)]
pub struct HomeyHooks {
    pub client: HomeyClient,
    pub cache: Arc<HomeyPumpCache>,
    /// Latest desired pump state (`true` = on). Read by the reconciliation
    /// poller so it always uses the current mode's intent, not whatever it
    /// captured at startup.
    pub desired_tx: watch::Sender<bool>,
    /// Boost-priority override lane. `Some(v)` masks `desired_tx`; `None`
    /// defers to SG-derived intent. Written by `DhwActor` (in Task 11) when
    /// a Bath boost wants to force pump off regardless of SG mode.
    pub boost_override_tx: watch::Sender<Option<bool>>,
}

/// Compute the pump's desired state from the `SmartGrid` mode.
///
/// Pump OFF only when actively blocking; in every other mode it should run.
#[must_use]
pub fn pump_on_for(mode: SmartGridMode) -> bool {
    !matches!(mode, SmartGridMode::Blocking)
}

/// Resolve the pump target from boost-override and SG-derived intent.
///
/// Boost-override wins when `Some(_)`; otherwise SG applies. Pure helper so
/// both the poller (`tick`) and `push_pump_to_homey` use the same rule.
#[must_use]
pub fn resolve_pump_target(boost_override: Option<bool>, sg_on: bool) -> bool {
    boost_override.unwrap_or(sg_on)
}

/// Receiver-friendly wrapper used by the Homey reconciler poller.
#[must_use]
pub fn reconciler_target(
    boost_rx: &tokio::sync::watch::Receiver<Option<bool>>,
    sg_rx: &tokio::sync::watch::Receiver<bool>,
) -> bool {
    resolve_pump_target(*boost_rx.borrow(), *sg_rx.borrow())
}

/// Fire-and-forget: push the pump state implied by `mode` to Homey, and
/// publish the desired state synchronously so the reconciliation poller
/// observes it immediately. On push failure the cache is marked stale; the
/// poller's next tick retries.
///
/// Honors an active boost-override lane: when DHW has set
/// `boost_override_tx = Some(v)` (Bath active), this push uses `v` as the
/// target — same resolution rule the reconciler poller uses. Without this
/// the eager SG-driven push would race the override and briefly flip the
/// pump to the SG-derived state until the next poller tick reconciles.
pub fn push_pump_to_homey(hooks: Option<&HomeyHooks>, mode: SmartGridMode) {
    let Some(hooks) = hooks else { return };
    let sg_on = pump_on_for(mode);
    let _ = hooks.desired_tx.send(sg_on);
    let target = resolve_pump_target(*hooks.boost_override_tx.borrow(), sg_on);
    let client = hooks.client.clone();
    let cache = hooks.cache.clone();
    tokio::spawn(async move {
        match client.set_pump_onoff(target).await {
            Ok(()) => cache.write_fresh(target).await,
            Err(e) => {
                tracing::warn!("Homey pump push failed: {e}");
                cache.mark_stale().await;
            }
        }
    });
}

/// Errors a `SetMode` command can surface.
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

impl std::error::Error for ApplyModeError {}

/// Commands the actor accepts. Replies are returned via `oneshot::Sender`
/// to preserve the request → response shape route handlers expect.
pub enum SmartGridCmd {
    SetMode {
        mode: SmartGridMode,
        schedule_resume: bool,
        /// When `Some` and scheduling applies, use this exact instant instead
        /// of `compute_resume_target`'s auto-pick. From the dashboard picker.
        resume_at: Option<SystemTime>,
        respond_to: oneshot::Sender<Result<Option<SystemTime>, ApplyModeError>>,
    },
    ReadMode {
        respond_to: oneshot::Sender<Result<SmartGridMode, String>>,
    },
    ModeChangedAt {
        respond_to: oneshot::Sender<Result<Option<SystemTime>, String>>,
    },
    ScheduledResumeAt {
        respond_to: oneshot::Sender<Option<SystemTime>>,
    },
    CancelScheduledResume {
        respond_to: oneshot::Sender<()>,
    },
    /// Internal: fired by the actor's own timer task when an auto-resume
    /// reaches its `fires_at`. The actor's generation check rejects stale
    /// fires that lost a race against a manual override.
    ResumeFire {
        fires_at: SystemTime,
        generation: u64,
    },
    /// Schedule a "Block + warm-by deadline" one-shot heat-up. Blocks
    /// immediately; the route has already resolved the temp-aware
    /// `heatup_start` (or `None` to skip the heat-up and just block).
    ScheduleWarmBy {
        cmd: WarmByCommand,
        respond_to: oneshot::Sender<Result<Option<SystemTime>, ApplyModeError>>,
    },
    /// Internal: phase-A timer reached `heatup_start` — flip to Normal and
    /// start the phase-B watcher. Generation-guarded like `ResumeFire`.
    HeatupStartFire {
        generation: u64,
    },
    /// Internal: phase-B watcher saw the compressor finish — re-block.
    /// Generation-guarded.
    HeatupDoneFire {
        generation: u64,
    },
}

/// Cheap-clone handle that route handlers use to send commands.
#[derive(Clone)]
pub struct SmartGridHandle {
    tx: mpsc::Sender<SmartGridCmd>,
}

impl SmartGridHandle {
    /// Send `SetMode` and await the reply.
    pub async fn set_mode(
        &self,
        mode: SmartGridMode,
        schedule_resume: bool,
        resume_at: Option<SystemTime>,
    ) -> Result<Option<SystemTime>, SmartGridError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(SmartGridCmd::SetMode {
                mode,
                schedule_resume,
                resume_at,
                respond_to,
            })
            .await
            .map_err(|_| SmartGridError::ActorGone)?;
        rx.await
            .map_err(|_| SmartGridError::ActorGone)?
            .map_err(SmartGridError::Apply)
    }

    pub async fn read_mode(&self) -> Result<SmartGridMode, SmartGridError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(SmartGridCmd::ReadMode { respond_to })
            .await
            .map_err(|_| SmartGridError::ActorGone)?;
        rx.await
            .map_err(|_| SmartGridError::ActorGone)?
            .map_err(SmartGridError::Internal)
    }

    pub async fn mode_changed_at(&self) -> Result<Option<SystemTime>, SmartGridError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(SmartGridCmd::ModeChangedAt { respond_to })
            .await
            .map_err(|_| SmartGridError::ActorGone)?;
        rx.await
            .map_err(|_| SmartGridError::ActorGone)?
            .map_err(SmartGridError::Internal)
    }

    pub async fn scheduled_resume_at(&self) -> Result<Option<SystemTime>, SmartGridError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(SmartGridCmd::ScheduledResumeAt { respond_to })
            .await
            .map_err(|_| SmartGridError::ActorGone)?;
        rx.await.map_err(|_| SmartGridError::ActorGone)
    }

    pub async fn cancel_scheduled_resume(&self) -> Result<(), SmartGridError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(SmartGridCmd::CancelScheduledResume { respond_to })
            .await
            .map_err(|_| SmartGridError::ActorGone)?;
        rx.await.map_err(|_| SmartGridError::ActorGone)
    }

    /// Apply Blocking now and schedule a warm-by heat-up. Returns the
    /// scheduled `heatup_start` instant, or `None` when the tank is already
    /// warm (`cmd.heatup_start` is `None`) — in that case the system is simply
    /// left blocked.
    pub async fn schedule_warm_by(
        &self,
        cmd: WarmByCommand,
    ) -> Result<Option<SystemTime>, SmartGridError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(SmartGridCmd::ScheduleWarmBy { cmd, respond_to })
            .await
            .map_err(|_| SmartGridError::ActorGone)?;
        rx.await
            .map_err(|_| SmartGridError::ActorGone)?
            .map_err(SmartGridError::Apply)
    }
}

#[derive(Debug)]
pub enum SmartGridError {
    /// The actor task is no longer running (shutdown).
    ActorGone,
    Apply(ApplyModeError),
    Internal(String),
}

impl std::fmt::Display for SmartGridError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ActorGone => write!(f, "SmartGrid actor unavailable"),
            Self::Apply(e) => write!(f, "{e}"),
            Self::Internal(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SmartGridError {}

struct ScheduledResume {
    fires_at: SystemTime,
    timer_task: AbortHandle,
}

/// A pending warm-by heat-up. The single `task` handle tracks whichever phase
/// is live: it holds the phase-A timer until `HeatupStartFire`, then is
/// **swapped** to the phase-B watcher so a later manual override aborts the
/// right task. `generation` is the snapshot taken when the heat-up was
/// scheduled; both fire handlers reject mismatches.
struct WarmBySlot {
    heatup_start: SystemTime,
    task: AbortHandle,
    generation: u64,
}

struct SmartGridActor {
    gpio: GpioController,
    scheduled_resume: Option<ScheduledResume>,
    /// Pending warm-by heat-up (phase A or B). Mutually exclusive with a plain
    /// auto-resume in practice — any mode change cancels both.
    scheduled_warmby: Option<WarmBySlot>,
    price_state: PriceState,
    config: SmartGridConfig,
    /// Cloned for the resume timer task to post back `ResumeFire`.
    self_tx: mpsc::Sender<SmartGridCmd>,
    /// Scaled Modbus reader for the warm-by phase-B watcher (tank temp +
    /// compressor status). `None` in GPIO-only deployments / tests.
    modbus: Option<Arc<dyn ModbusWriter>>,
    /// When `Some`, every successful mode write is mirrored to Homey to
    /// drive the Cirkulationspump on/off.
    homey: Option<HomeyHooks>,
}

/// Spawn the actor.
///
/// On hardware error during construction or initial mode write, returns an
/// error. The caller (main) treats that as "GPIO unavailable" and proceeds
/// without a `SmartGrid` handle — every route returns `ServiceUnavailable`.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    gpio_k24: u32,
    gpio_k25: u32,
    active_low: bool,
    initial_mode: SmartGridMode,
    price_state: PriceState,
    config: SmartGridConfig,
    modbus: Option<Arc<dyn ModbusWriter>>,
    homey: Option<HomeyHooks>,
    cancel: CancellationToken,
) -> Result<(SmartGridHandle, JoinHandle<()>), String> {
    let mut gpio = GpioController::new(gpio_k24, gpio_k25, active_low)?;
    if let Err(e) = gpio.set_mode(initial_mode) {
        return Err(format!("Initial GPIO set_mode({initial_mode}) failed: {e}"));
    }

    // Seed Homey with the initial mode's desired pump state so the first
    // poll doesn't reconcile against a stale value.
    push_pump_to_homey(homey.as_ref(), initial_mode);

    let (tx, rx) = mpsc::channel(32);
    let actor = SmartGridActor {
        gpio,
        scheduled_resume: None,
        scheduled_warmby: None,
        price_state,
        config,
        self_tx: tx.clone(),
        modbus,
        homey,
    };
    let join = tokio::spawn(actor.run(rx, cancel));
    Ok((SmartGridHandle { tx }, join))
}

/// Pick the wall-clock instant at which `mode` should auto-resume to Normal.
///
/// `Blocking` resumes at the start of the cheapest contiguous run of length
/// `auto_resume_min_duration_minutes` within the `auto_resume_window_hours`
/// horizon. If no run of that length fits (sparse price data, fragmented
/// today/tomorrow boundary), falls back to the cheapest single slot so we
/// still schedule. `LowPrice` / `Overcapacity` resume when the current cheap
/// run ends, capped by the window. `Normal` never schedules.
fn compute_resume_target(
    price_state: &PriceState,
    config: &SmartGridConfig,
    mode: SmartGridMode,
) -> Option<SystemTime> {
    let window = Duration::from_secs(config.auto_resume_window_hours.saturating_mul(3600));
    let run_duration =
        Duration::from_secs(u64::from(config.auto_resume_min_duration_minutes).saturating_mul(60));
    match mode {
        SmartGridMode::Blocking => price_state
            .cheapest_run_within(window, run_duration)
            .or_else(|| price_state.cheapest_within(window))
            .and_then(|slot| parse_iso8601(&slot.starts_at).ok()),
        SmartGridMode::LowPrice | SmartGridMode::Overcapacity => {
            price_state.cheap_window_end(window)
        }
        SmartGridMode::Normal => None,
    }
}

/// Resolve the resume instant for a scheduling mode change: an explicit
/// picker choice (`resume_at`) always wins; otherwise fall back to the
/// per-mode auto-computation. Pure (no GPIO) so the override precedence is
/// unit-testable.
fn resolve_fires_at(
    resume_at: Option<SystemTime>,
    price_state: &PriceState,
    config: &SmartGridConfig,
    mode: SmartGridMode,
) -> Option<SystemTime> {
    resume_at.or_else(|| compute_resume_target(price_state, config, mode))
}

impl SmartGridActor {
    async fn run(mut self, mut rx: mpsc::Receiver<SmartGridCmd>, cancel: CancellationToken) {
        info!("SmartGrid actor started");
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    info!("SmartGrid actor: shutdown signal received");
                    break;
                }
                cmd = rx.recv() => if let Some(c) = cmd {
                    self.handle(c);
                } else {
                    info!("SmartGrid actor: all senders dropped, exiting");
                    break;
                }
            }
        }
        // Abort any pending timers so we don't leak them past shutdown.
        self.cancel_all_schedules();
    }

    /// Abort and clear both the auto-resume and warm-by pending timers. Called
    /// before any mode change and on shutdown so a stale fire can never land
    /// after the user's intent has moved on.
    ///
    /// Bumps the mode generation FIRST: a timer task that already left its
    /// sleep and queued its `*Fire` before `abort()` landed would otherwise
    /// pass the generation guard and apply a mode after the schedule was
    /// cancelled (e.g. a bare `DELETE /scheduled_resume` could still flip to
    /// Normal, or — worse for warm-by — leave the system stuck in Normal with
    /// the slot already cleared and no watcher spawned). Bumping invalidates
    /// any in-flight fire.
    fn cancel_all_schedules(&mut self) {
        self.gpio.bump_mode_generation();
        if let Some(prev) = self.scheduled_resume.take() {
            prev.timer_task.abort();
        }
        if let Some(prev) = self.scheduled_warmby.take() {
            prev.task.abort();
        }
    }

    fn handle(&mut self, cmd: SmartGridCmd) {
        match cmd {
            SmartGridCmd::SetMode {
                mode,
                schedule_resume,
                resume_at,
                respond_to,
            } => {
                let result = self.do_set_mode(mode, schedule_resume, resume_at);
                let _ = respond_to.send(result);
            }
            SmartGridCmd::ReadMode { respond_to } => {
                let _ = respond_to.send(Ok(self.gpio.read_mode()));
            }
            SmartGridCmd::ModeChangedAt { respond_to } => {
                let _ = respond_to.send(Ok(self.gpio.mode_changed_at()));
            }
            SmartGridCmd::ScheduledResumeAt { respond_to } => {
                let _ = respond_to.send(self.pending_schedule_at());
            }
            SmartGridCmd::CancelScheduledResume { respond_to } => {
                self.cancel_all_schedules();
                let _ = respond_to.send(());
            }
            SmartGridCmd::ResumeFire {
                fires_at,
                generation,
            } => self.on_resume_fire(fires_at, generation),
            SmartGridCmd::ScheduleWarmBy { cmd, respond_to } => {
                let result = self.do_warm_by(cmd);
                let _ = respond_to.send(result);
            }
            SmartGridCmd::HeatupStartFire { generation } => self.on_heatup_start_fire(generation),
            SmartGridCmd::HeatupDoneFire { generation } => self.on_heatup_done_fire(generation),
        }
    }

    /// The next pending flip the dashboard should show: a scheduled auto-resume,
    /// or a warm-by heat-up start that is still in the future (phase A).
    fn pending_schedule_at(&self) -> Option<SystemTime> {
        if let Some(s) = self.scheduled_resume.as_ref() {
            return Some(s.fires_at);
        }
        let now = SystemTime::now();
        self.scheduled_warmby
            .as_ref()
            .filter(|w| w.heatup_start > now)
            .map(|w| w.heatup_start)
    }

    fn do_set_mode(
        &mut self,
        mode: SmartGridMode,
        schedule_resume: bool,
        resume_at: Option<SystemTime>,
    ) -> Result<Option<SystemTime>, ApplyModeError> {
        // Always cancel a prior schedule before mutating: a manual change
        // overrides any pending auto-flip or warm-by heat-up. This also bumps
        // the mode generation FIRST, so any in-flight timer that already left
        // its sleep sees a mismatched generation and bails out.
        self.cancel_all_schedules();

        self.gpio.set_mode(mode).map_err(ApplyModeError::Gpio)?;

        // Pump tracks SmartGrid mode: ON for Normal/LowPrice/Overcapacity,
        // OFF for Blocking. Fire-and-forget — must not stall the actor.
        push_pump_to_homey(self.homey.as_ref(), mode);

        if mode == SmartGridMode::Normal || !schedule_resume || !self.config.auto_resume_enabled {
            return Ok(None);
        }

        // An explicit picker choice overrides the auto-pick; otherwise fall
        // back to the cheapest-run computation.
        let fires_at = resolve_fires_at(resume_at, &self.price_state, &self.config, mode);

        let Some(fires_at) = fires_at else {
            warn!(
                "Auto-resume: no resume target for mode {mode} within {}h — heater stays in {mode}",
                self.config.auto_resume_window_hours
            );
            return Ok(None);
        };

        // Snapshot the generation now. The timer task posts it back with
        // ResumeFire so we can recognise stale fires that lost a race.
        let generation = self.gpio.mode_generation();
        let tx = self.self_tx.clone();
        let timer_task = tokio::spawn(async move {
            // tokio::time::sleep is monotonic. Re-check wall-clock in
            // ≤ 60 s chunks so NTP steps don't misfire by more than that.
            while let Ok(remaining) = fires_at.duration_since(SystemTime::now()) {
                if remaining.is_zero() {
                    break;
                }
                let step = remaining.min(Duration::from_mins(1));
                tokio::time::sleep(step).await;
            }
            // If the channel is closed (actor shut down) or this task has
            // been aborted between the sleep and here, the send is a no-op.
            let _ = tx
                .send(SmartGridCmd::ResumeFire {
                    fires_at,
                    generation,
                })
                .await;
        })
        .abort_handle();

        self.scheduled_resume = Some(ScheduledResume {
            fires_at,
            timer_task,
        });
        info!("Auto-resume scheduled for {:?} (mode={mode})", fires_at);
        Ok(Some(fires_at))
    }

    fn on_resume_fire(&mut self, fires_at: SystemTime, generation: u64) {
        // Belt-and-suspenders: even though commands are serial, the timer
        // task may have already left its sleep before the abort took effect.
        // Generation mismatch means a superseding SetMode landed between.
        if let Ok(rejected) = self
            .gpio
            .set_mode_if_not_superseded(SmartGridMode::Normal, generation)
        {
            if !rejected {
                // Superseded; the new SetMode handler already replaced the
                // scheduled-resume slot (or cleared it).
                return;
            }
            // Clear our slot if it's still pointing at this fire.
            if self
                .scheduled_resume
                .as_ref()
                .is_some_and(|s| s.fires_at == fires_at)
            {
                self.scheduled_resume = None;
            }
            // Auto-resume always targets Normal → pump comes back on.
            push_pump_to_homey(self.homey.as_ref(), SmartGridMode::Normal);
            info!("Auto-resume fired — heater set back to Normal");
        } else {
            error!("Auto-resume: failed to set Normal");
        }
    }

    /// Apply Blocking now and, unless the tank is already warm, schedule the
    /// phase-A timer that will flip to Normal at `heatup_start`.
    fn do_warm_by(&mut self, cmd: WarmByCommand) -> Result<Option<SystemTime>, ApplyModeError> {
        // Same prologue as do_set_mode: cancel prior schedules (which bumps the
        // generation, invalidating any in-flight fire), block, push pump.
        self.cancel_all_schedules();
        self.gpio
            .set_mode(SmartGridMode::Blocking)
            .map_err(ApplyModeError::Gpio)?;
        push_pump_to_homey(self.homey.as_ref(), SmartGridMode::Blocking);

        let Some(heatup_start) = cmd.heatup_start else {
            info!("Warm-by: tank already at target — staying blocked, no heat-up");
            return Ok(None);
        };

        let generation = self.gpio.mode_generation();
        let tx = self.self_tx.clone();
        let task = tokio::spawn(async move {
            // Monotonic sleep, re-checked in ≤60s chunks so an NTP step
            // misfires by at most that. Mirrors the auto-resume timer.
            while let Ok(remaining) = heatup_start.duration_since(SystemTime::now()) {
                if remaining.is_zero() {
                    break;
                }
                tokio::time::sleep(remaining.min(Duration::from_mins(1))).await;
            }
            let _ = tx.send(SmartGridCmd::HeatupStartFire { generation }).await;
        })
        .abort_handle();

        self.scheduled_warmby = Some(WarmBySlot {
            heatup_start,
            task,
            generation,
        });
        info!("Warm-by heat-up scheduled to start at {heatup_start:?}");
        Ok(Some(heatup_start))
    }

    /// Phase A fired: flip to Normal so the heat pump charges the tank, then
    /// spawn the phase-B watcher and swap the slot's handle to it.
    fn on_heatup_start_fire(&mut self, generation: u64) {
        // Without a Modbus reader we cannot watch the compressor, so we would
        // never know when to re-block — don't enter an unwatchable Normal.
        // Production always has a reader (built unconditionally in main).
        let Some(modbus) = self.modbus.clone() else {
            error!("Warm-by: no Modbus reader — cannot run heat-up, staying blocked");
            return;
        };

        match self
            .gpio
            .set_mode_if_not_superseded(SmartGridMode::Normal, generation)
        {
            Ok(true) => {}
            Ok(false) => return, // superseded by a manual change
            Err(e) => {
                error!("Warm-by: failed to set Normal at heat-up start: {e}");
                return;
            }
        }
        push_pump_to_homey(self.homey.as_ref(), SmartGridMode::Normal);

        // The slot must still be the one we scheduled (generation match).
        let Some(slot) = self
            .scheduled_warmby
            .as_mut()
            .filter(|s| s.generation == generation)
        else {
            return;
        };

        // Swap the slot's handle from the (fired) phase-A timer to the phase-B
        // watcher so a later manual override aborts the live task. The watcher
        // re-blocks only when the heat pump finishes its own cycle.
        let tx = self.self_tx.clone();
        slot.task = tokio::spawn(run_heatup_watcher(generation, modbus, tx)).abort_handle();
        info!("Warm-by heat-up started — Normal, watching for compressor finish");
    }

    /// Phase B fired: the compressor finished — re-block and stay blocked.
    fn on_heatup_done_fire(&mut self, generation: u64) {
        match self
            .gpio
            .set_mode_if_not_superseded(SmartGridMode::Blocking, generation)
        {
            Ok(true) => {}
            Ok(false) => return, // superseded by a manual change
            Err(e) => {
                error!("Warm-by: failed to re-block after heat-up: {e}");
                return;
            }
        }
        push_pump_to_homey(self.homey.as_ref(), SmartGridMode::Blocking);
        if self
            .scheduled_warmby
            .as_ref()
            .is_some_and(|s| s.generation == generation)
        {
            self.scheduled_warmby = None;
        }
        info!("Warm-by heat-up done (compressor finished) — re-blocked");
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Test-only helpers that let the smartgrid actor be exercised without
    //! real `/dev/gpiochip0` access. Mirrors `GpioController::new_for_test`.

    use super::*;

    /// Spawn the actor with a test-only `GpioController` (no hardware ioctls).
    /// The actor still serialises commands correctly; only `do_set_mode`
    /// will error out at the GPIO write step. For tests that don't care
    /// about that (concurrency, schedule, supersession), set the initial
    /// mode to Normal so no GPIO write happens during spawn.
    pub fn spawn_with_test_gpio(
        price_state: PriceState,
        config: SmartGridConfig,
        cancel: CancellationToken,
    ) -> (SmartGridHandle, JoinHandle<()>) {
        let gpio = GpioController::new_for_test(20, 21, false);
        let (tx, rx) = mpsc::channel(32);
        let actor = SmartGridActor {
            gpio,
            scheduled_resume: None,
            scheduled_warmby: None,
            price_state,
            config,
            self_tx: tx.clone(),
            modbus: None,
            homey: None,
        };
        let join = tokio::spawn(actor.run(rx, cancel));
        (SmartGridHandle { tx }, join)
    }

    /// Like [`spawn_with_test_gpio`] but the controller accepts `set_mode`
    /// writes in memory (no hardware ioctl). This unlocks the actor's
    /// post-set scheduling, resume-timer, and cancellation paths that the
    /// erroring controller can never reach.
    pub fn spawn_accepting_test_gpio(
        price_state: PriceState,
        config: SmartGridConfig,
        cancel: CancellationToken,
    ) -> (SmartGridHandle, JoinHandle<()>) {
        spawn_accepting_test_gpio_with_modbus(price_state, config, None, cancel)
    }

    /// Like [`spawn_accepting_test_gpio`] but with an injectable Modbus reader
    /// so the warm-by phase-B watcher can be driven by a fake in tests.
    pub fn spawn_accepting_test_gpio_with_modbus(
        price_state: PriceState,
        config: SmartGridConfig,
        modbus: Option<Arc<dyn ModbusWriter>>,
        cancel: CancellationToken,
    ) -> (SmartGridHandle, JoinHandle<()>) {
        let gpio = GpioController::new_for_test_accepting(20, 21, false);
        let (tx, rx) = mpsc::channel(32);
        let actor = SmartGridActor {
            gpio,
            scheduled_resume: None,
            scheduled_warmby: None,
            price_state,
            config,
            self_tx: tx.clone(),
            modbus,
            homey: None,
        };
        let join = tokio::spawn(actor.run(rx, cancel));
        (SmartGridHandle { tx }, join)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::spawn_with_test_gpio;
    use super::*;
    use crate::homey::test_support::{SharedMock, make_client, spawn_mock};
    use std::net::SocketAddr;

    fn make_hooks(addr: SocketAddr) -> (HomeyHooks, watch::Receiver<bool>) {
        let client = make_client(addr);
        let cache = Arc::new(HomeyPumpCache::new());
        let (desired_tx, desired_rx) = watch::channel(true);
        let (boost_override_tx, _boost_override_rx) = watch::channel(None::<bool>);
        (
            HomeyHooks {
                client,
                cache,
                desired_tx,
                boost_override_tx,
            },
            desired_rx,
        )
    }

    /// Wait briefly for the fire-and-forget `tokio::spawn` inside
    /// `push_pump_to_homey` to complete and the mock to record the call.
    async fn wait_for_sets(state: &SharedMock, want_len: usize) {
        for _ in 0..50 {
            if state.lock().unwrap().set_calls.len() >= want_len {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "expected at least {want_len} sets, got {:?}",
            state.lock().unwrap().set_calls
        );
    }

    fn test_config() -> SmartGridConfig {
        SmartGridConfig::default()
    }

    #[tokio::test]
    async fn cancellation_token_makes_actor_exit_quickly() {
        let cancel = CancellationToken::new();
        let (handle, join) = spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );

        cancel.cancel();
        // Should exit within 100 ms, not waiting for the 5 s shutdown timeout.
        tokio::time::timeout(Duration::from_millis(100), join)
            .await
            .expect("actor must exit within 100 ms after cancel")
            .expect("actor must not panic");
        // After exit, sends fail (actor gone).
        let err = handle.read_mode().await.unwrap_err();
        assert!(matches!(err, SmartGridError::ActorGone));
    }

    #[tokio::test]
    async fn read_mode_succeeds_via_handle() {
        let cancel = CancellationToken::new();
        let (handle, _join) = spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );
        let mode = handle.read_mode().await.expect("read");
        assert!(matches!(mode, SmartGridMode::Normal));
        cancel.cancel();
    }

    /// Regression for Critical #2 from the code review: concurrent `SetMode`
    /// calls must not interleave. Even with no real GPIO, the actor's serial
    /// command processing means both calls return their results without
    /// races.
    #[tokio::test]
    async fn concurrent_set_mode_calls_serialise() {
        let cancel = CancellationToken::new();
        let (handle, _join) = spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );

        // Both calls will fail at the GPIO write (test-only controller),
        // but the actor must process them serially without panic or hang.
        let h1 = handle.clone();
        let h2 = handle.clone();
        let t1 =
            tokio::spawn(async move { h1.set_mode(SmartGridMode::Blocking, false, None).await });
        let t2 =
            tokio::spawn(async move { h2.set_mode(SmartGridMode::LowPrice, false, None).await });

        let (r1, r2) = (t1.await.unwrap(), t2.await.unwrap());
        // Both error out at the hardware write — but the actor responded to
        // both, proving they were serialised through the channel.
        assert!(matches!(r1, Err(SmartGridError::Apply(_))));
        assert!(matches!(r2, Err(SmartGridError::Apply(_))));

        cancel.cancel();
    }

    #[tokio::test]
    async fn actor_survives_gpio_error_and_serves_subsequent_commands() {
        // The test-only GPIO controller errors on every write. A failed
        // set_mode must NOT bring the actor down — subsequent reads should
        // still succeed. This guards against a regression where an error
        // path inadvertently exits the actor loop.
        let cancel = CancellationToken::new();
        let (handle, _join) = spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );

        let err = handle
            .set_mode(SmartGridMode::Blocking, false, None)
            .await
            .unwrap_err();
        assert!(matches!(err, SmartGridError::Apply(_)));

        // Actor must still respond to follow-up commands.
        let mode = handle.read_mode().await.expect("actor still alive");
        // GPIO write failed → in-memory mode unchanged from initial Normal.
        assert!(matches!(mode, SmartGridMode::Normal));
        let resume = handle
            .scheduled_resume_at()
            .await
            .expect("actor still alive");
        assert!(resume.is_none());

        cancel.cancel();
    }

    #[tokio::test]
    async fn all_handle_methods_return_actor_gone_after_shutdown() {
        // Once the actor has exited, every handle method should map
        // channel-send/receive failures to ActorGone — not panic, not hang,
        // not return a stale success.
        let cancel = CancellationToken::new();
        let (handle, join) = spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );

        cancel.cancel();
        tokio::time::timeout(Duration::from_millis(200), join)
            .await
            .expect("actor must exit within 200 ms")
            .expect("actor must not panic");

        assert!(matches!(
            handle.read_mode().await,
            Err(SmartGridError::ActorGone)
        ));
        assert!(matches!(
            handle.set_mode(SmartGridMode::Normal, false, None).await,
            Err(SmartGridError::ActorGone)
        ));
        assert!(matches!(
            handle.mode_changed_at().await,
            Err(SmartGridError::ActorGone)
        ));
        assert!(matches!(
            handle.scheduled_resume_at().await,
            Err(SmartGridError::ActorGone)
        ));
        assert!(matches!(
            handle.cancel_scheduled_resume().await,
            Err(SmartGridError::ActorGone)
        ));
    }

    #[tokio::test]
    async fn actor_drains_pending_commands_then_exits_on_cancel() {
        // Sending a command then cancelling should still produce a response
        // (the actor is event-loop driven, so the in-flight command finishes
        // before the cancel branch wins on the next iteration). We just want
        // to verify there's no deadlock / orphaned oneshot.
        let cancel = CancellationToken::new();
        let (handle, join) = spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );

        // Issue a read concurrently with cancellation; race is intentional.
        let h = handle.clone();
        let read_fut = tokio::spawn(async move { h.read_mode().await });
        cancel.cancel();

        // The read either succeeded (actor handled before exit) or failed
        // with ActorGone (channel closed first). Both are valid; what we
        // disallow is the task hanging.
        let result = tokio::time::timeout(Duration::from_millis(500), read_fut)
            .await
            .expect("read must complete within 500 ms");
        let _ = result.expect("read task must not panic");

        // Actor must have exited.
        tokio::time::timeout(Duration::from_millis(200), join)
            .await
            .expect("actor must exit within 200 ms")
            .expect("actor must not panic");
    }

    // ── push_pump_to_homey: unit tests for the helper itself ─────────
    //
    // The helper is tested in isolation rather than driving it through
    // the full actor loop. Two reasons: the test-only GpioController
    // errors on every `set_mode`, which would short-circuit the helper
    // call site inside `do_set_mode`; and the helper has its own
    // fire-and-forget tokio::spawn, so going through the actor adds
    // timing noise without exercising any additional code path.

    #[test]
    fn pump_on_for_matches_plan() {
        assert!(pump_on_for(SmartGridMode::Normal));
        assert!(pump_on_for(SmartGridMode::LowPrice));
        assert!(pump_on_for(SmartGridMode::Overcapacity));
        assert!(!pump_on_for(SmartGridMode::Blocking));
    }

    #[test]
    fn reconciler_target_prefers_boost_override_over_sg_intent() {
        use tokio::sync::watch;
        let (sg_tx, sg_rx) = watch::channel(true);
        let (boost_tx, boost_rx) = watch::channel(None::<bool>);

        // No override: target = sg intent.
        assert!(super::reconciler_target(&boost_rx, &sg_rx));

        // Override Some(false) wins.
        boost_tx.send(Some(false)).unwrap();
        assert!(!super::reconciler_target(&boost_rx, &sg_rx));

        // SG intent flips to false; override still wins (still Some(false)).
        sg_tx.send(false).unwrap();
        assert!(!super::reconciler_target(&boost_rx, &sg_rx));

        // Override cleared: target falls back to sg intent.
        boost_tx.send(None).unwrap();
        assert!(!super::reconciler_target(&boost_rx, &sg_rx));

        // SG intent back to true; no override → target true.
        sg_tx.send(true).unwrap();
        assert!(super::reconciler_target(&boost_rx, &sg_rx));
    }

    use crate::energy::price::test_support::{make_run, slot as isolated_slot};

    /// Blocking-mode resume must pick the start of the cheapest contiguous
    /// 30-min run, not an isolated single slot that happens to be cheaper
    /// on its own. Regression for the original "cheapest 15-min" behaviour
    /// where a brief dip would unblock the heater into a price that ramps
    /// up immediately afterwards.
    #[test]
    fn compute_resume_target_blocking_prefers_contiguous_run_over_isolated_cheaper_slot() {
        let state = PriceState::new("SE3".to_string());
        let mut prices = Vec::new();
        prices.extend(make_run(60, &[0.10, 0.10])); // contiguous 30-min run @ avg 0.10
        prices.push(isolated_slot(180, 0.02)); // isolated cheaper single slot (no neighbour)
        state.update_prices(prices, vec![]);
        let cfg = test_config();
        let target = compute_resume_target(&state, &cfg, SmartGridMode::Blocking)
            .expect("Blocking with prices in window must produce a target");
        let expected = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(60));
        let diff = target
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        assert!(
            diff < Duration::from_secs(5),
            "target must be ~now+60min (start of contiguous run), got diff {diff:?}"
        );
    }

    /// When no run of the required length fits anywhere inside the window,
    /// the helper falls back to `cheapest_within` so we still schedule a
    /// resume — better imperfect than nothing.
    #[test]
    fn compute_resume_target_blocking_falls_back_to_single_slot_when_no_run_fits() {
        let state = PriceState::new("SE3".to_string());
        // Only one 15-min slot; no second slot adjacent to it — a 30-min
        // contiguous run cannot be formed.
        state.update_prices(vec![isolated_slot(45, 0.20)], vec![]);
        let cfg = test_config();
        let target = compute_resume_target(&state, &cfg, SmartGridMode::Blocking)
            .expect("fallback to cheapest_within must keep a schedule");
        let expected = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(45));
        let diff = target
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        assert!(
            diff < Duration::from_secs(5),
            "fallback target must be the single available slot, got diff {diff:?}"
        );
    }

    /// `Normal` is a guarded case in `do_set_mode` (early-returned before we
    /// reach this helper), but the helper itself must be total — return
    /// `None` rather than panic.
    #[test]
    fn compute_resume_target_normal_returns_none() {
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let cfg = test_config();
        assert!(compute_resume_target(&state, &cfg, SmartGridMode::Normal).is_none());
    }

    use crate::energy::price::{PriceLevel, PricePoint};

    /// Build a 15-min leveled slot starting `offset_mins` from now. The
    /// `LowPrice`/`Overcapacity` dispatch in `compute_resume_target` routes to
    /// `cheap_window_end`, which only reasons about slots that carry a
    /// `PriceLevel`; the `make_run`/`slot` helpers leave `level: None`.
    fn leveled_slot(offset_mins: i64, spot_sek: f64, level: PriceLevel) -> PricePoint {
        let start = chrono::Utc::now() + chrono::Duration::minutes(offset_mins);
        let end = start + chrono::Duration::minutes(15);
        let mut p = PricePoint::from_spot(start.to_rfc3339(), end.to_rfc3339(), spot_sek, 0.0, 0.0);
        p.level = Some(level);
        p
    }

    /// `LowPrice` dispatches to `cheap_window_end`: resume at the start of the
    /// first non-cheap slot inside the window. Exercises the
    /// `LowPrice | Overcapacity` arm of `compute_resume_target`, which the
    /// Blocking-focused tests never reach.
    #[test]
    fn compute_resume_target_lowprice_resumes_at_first_non_cheap_slot() {
        let state = PriceState::new("SE3".to_string());
        state.update_prices(
            vec![
                leveled_slot(15, 0.10, PriceLevel::VeryCheap),
                leveled_slot(30, 0.12, PriceLevel::Cheap),
                leveled_slot(45, 0.50, PriceLevel::Normal), // first non-cheap
            ],
            vec![],
        );
        let cfg = test_config();
        let target = compute_resume_target(&state, &cfg, SmartGridMode::LowPrice)
            .expect("a non-cheap slot inside the window yields a resume target");
        let expected = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(45));
        let diff = target
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        assert!(
            diff < Duration::from_secs(5),
            "LowPrice resume must land at the first non-cheap slot, diff {diff:?}"
        );
    }

    /// `Overcapacity` shares the `cheap_window_end` dispatch with `LowPrice`.
    /// When every slot in the window is cheap, the helper caps the resume at
    /// the window end so the heater never stays buffering indefinitely.
    #[test]
    fn compute_resume_target_overcapacity_all_cheap_caps_at_window_end() {
        let state = PriceState::new("SE3".to_string());
        state.update_prices(
            vec![
                leveled_slot(15, 0.10, PriceLevel::VeryCheap),
                leveled_slot(30, 0.11, PriceLevel::Cheap),
            ],
            vec![],
        );
        let cfg = test_config(); // 12h window
        let target = compute_resume_target(&state, &cfg, SmartGridMode::Overcapacity)
            .expect("all-cheap window still produces a capped resume target");
        let expected = SystemTime::from(chrono::Utc::now() + chrono::Duration::hours(12));
        let diff = target
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        assert!(
            diff < Duration::from_secs(5),
            "all-cheap Overcapacity must cap at window end, diff {diff:?}"
        );
    }

    /// `cheap_window_end` returns `None` when there is no leveled price data
    /// in the window, so the buffer-mode dispatch yields no schedule.
    #[test]
    fn compute_resume_target_lowprice_no_prices_returns_none() {
        let state = PriceState::new("SE3".to_string());
        let cfg = test_config();
        assert!(compute_resume_target(&state, &cfg, SmartGridMode::LowPrice).is_none());
    }

    #[test]
    fn resolve_fires_at_prefers_explicit_over_computed() {
        let state = PriceState::new("SE3".to_string());
        let cfg = SmartGridConfig::default();
        let explicit = SystemTime::now() + Duration::from_hours(2);
        // Explicit time wins even for a mode whose auto-pick would differ.
        let got = resolve_fires_at(Some(explicit), &state, &cfg, SmartGridMode::Blocking);
        assert_eq!(got, Some(explicit));
    }

    #[test]
    fn resolve_fires_at_falls_back_to_compute_when_none() {
        // No price data → Blocking auto-pick yields None, and with no explicit
        // time the helper returns that None (proves the fallback is wired).
        let state = PriceState::new("SE3".to_string());
        let cfg = SmartGridConfig::default();
        let got = resolve_fires_at(None, &state, &cfg, SmartGridMode::Blocking);
        assert!(got.is_none());
    }

    /// With no explicit `resume_at`, `resolve_fires_at` must surface the
    /// per-mode auto-computation when it yields `Some`. Pairs with
    /// `resolve_fires_at_falls_back_to_compute_when_none` (the `None` arm).
    #[test]
    fn resolve_fires_at_falls_back_to_computed_some() {
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let cfg = test_config();
        let got = resolve_fires_at(None, &state, &cfg, SmartGridMode::Blocking);
        let expected = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(60));
        let target = got.expect("computed Blocking target must flow through resolve_fires_at");
        let diff = target
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        assert!(diff < Duration::from_secs(5), "diff {diff:?}");
    }

    #[tokio::test]
    async fn set_mode_with_explicit_resume_at_normal_never_schedules() {
        let cancel = CancellationToken::new();
        let (handle, _join) = test_support::spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            SmartGridConfig::default(),
            cancel,
        );
        let target = SystemTime::now() + Duration::from_hours(1);
        // Normal never schedules, regardless of an explicit resume_at. The
        // test-only GpioController errors on every write (no held request),
        // so the call surfaces the GPIO Apply error before the Normal guard
        // is reached — but the meaningful guarantee still holds: no resume
        // timer was registered.
        let result = handle
            .set_mode(SmartGridMode::Normal, true, Some(target))
            .await;
        assert!(matches!(result, Err(SmartGridError::Apply(_))));
        let scheduled = handle.scheduled_resume_at().await.unwrap();
        assert!(scheduled.is_none());
    }

    // ── Scheduling paths through the accepting test GPIO ─────────────
    //
    // These drive the real do_set_mode → resolve_fires_at → schedule path,
    // plus on_resume_fire and CancelScheduledResume-with-pending, which the
    // erroring controller can never reach (every set_mode short-circuits).

    #[tokio::test]
    async fn blocking_with_prices_schedules_resume_at_run_start() {
        let cancel = CancellationToken::new();
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let (handle, _join) =
            test_support::spawn_accepting_test_gpio(state, test_config(), cancel.clone());

        let scheduled = handle
            .set_mode(SmartGridMode::Blocking, true, None)
            .await
            .expect("Blocking set_mode succeeds with accepting GPIO")
            .expect("a resume must be scheduled");

        // Echoed schedule must match what scheduled_resume_at reports.
        let queried = handle.scheduled_resume_at().await.unwrap();
        assert_eq!(queried, Some(scheduled));

        // Target is the start of the contiguous run: now + 60 min.
        let expected = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(60));
        let diff = scheduled
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        assert!(diff < Duration::from_secs(5), "diff {diff:?}");

        // Mode actually flipped to Blocking in memory.
        assert!(matches!(
            handle.read_mode().await.unwrap(),
            SmartGridMode::Blocking
        ));
        cancel.cancel();
    }

    #[tokio::test]
    async fn lowprice_schedules_cheap_window_end_target() {
        let cancel = CancellationToken::new();
        let state = PriceState::new("SE3".to_string());
        state.update_prices(
            vec![
                leveled_slot(15, 0.10, PriceLevel::VeryCheap),
                leveled_slot(30, 0.12, PriceLevel::Cheap),
                leveled_slot(45, 0.50, PriceLevel::Normal), // first non-cheap
            ],
            vec![],
        );
        let (handle, _join) =
            test_support::spawn_accepting_test_gpio(state, test_config(), cancel.clone());

        let scheduled = handle
            .set_mode(SmartGridMode::LowPrice, true, None)
            .await
            .expect("LowPrice set_mode succeeds")
            .expect("buffer mode schedules a resume at cheap-window end");

        // Resume lands at the first non-cheap slot: now + 45 min.
        let expected = SystemTime::from(chrono::Utc::now() + chrono::Duration::minutes(45));
        let diff = scheduled
            .duration_since(expected)
            .unwrap_or_else(|e| e.duration());
        assert!(diff < Duration::from_secs(5), "diff {diff:?}");
        cancel.cancel();
    }

    #[tokio::test]
    async fn explicit_resume_at_overrides_auto_pick() {
        let cancel = CancellationToken::new();
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let (handle, _join) =
            test_support::spawn_accepting_test_gpio(state, test_config(), cancel.clone());

        let explicit = SystemTime::now() + Duration::from_hours(3);
        let scheduled = handle
            .set_mode(SmartGridMode::Blocking, true, Some(explicit))
            .await
            .expect("set_mode succeeds")
            .expect("explicit resume is scheduled");
        assert_eq!(scheduled, explicit);
        cancel.cancel();
    }

    #[tokio::test]
    async fn schedule_resume_false_does_not_schedule() {
        let cancel = CancellationToken::new();
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let (handle, _join) =
            test_support::spawn_accepting_test_gpio(state, test_config(), cancel.clone());

        let scheduled = handle
            .set_mode(SmartGridMode::Blocking, false, None)
            .await
            .expect("set_mode succeeds");
        assert!(scheduled.is_none());
        assert!(handle.scheduled_resume_at().await.unwrap().is_none());
        cancel.cancel();
    }

    #[tokio::test]
    async fn auto_resume_disabled_config_never_schedules() {
        let cancel = CancellationToken::new();
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let cfg = SmartGridConfig {
            auto_resume_enabled: false,
            ..SmartGridConfig::default()
        };
        let (handle, _join) = test_support::spawn_accepting_test_gpio(state, cfg, cancel.clone());

        let scheduled = handle
            .set_mode(SmartGridMode::Blocking, true, None)
            .await
            .expect("set_mode succeeds");
        assert!(scheduled.is_none(), "disabled config must not schedule");
        cancel.cancel();
    }

    #[tokio::test]
    async fn blocking_no_prices_logs_and_skips_schedule() {
        // schedule_resume=true but no price data → compute yields None → the
        // warn-and-return-None arm in do_set_mode.
        let cancel = CancellationToken::new();
        let (handle, _join) = test_support::spawn_accepting_test_gpio(
            PriceState::new("SE3".to_string()),
            test_config(),
            cancel.clone(),
        );

        let scheduled = handle
            .set_mode(SmartGridMode::Blocking, true, None)
            .await
            .expect("set_mode succeeds");
        assert!(scheduled.is_none());
        assert!(handle.scheduled_resume_at().await.unwrap().is_none());
        cancel.cancel();
    }

    #[tokio::test]
    async fn later_mode_change_cancels_pending_schedule() {
        let cancel = CancellationToken::new();
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let (handle, _join) =
            test_support::spawn_accepting_test_gpio(state, test_config(), cancel.clone());

        // Schedule a resume far in the future.
        let explicit = SystemTime::now() + Duration::from_hours(5);
        handle
            .set_mode(SmartGridMode::Blocking, true, Some(explicit))
            .await
            .unwrap()
            .expect("scheduled");
        assert!(handle.scheduled_resume_at().await.unwrap().is_some());

        // A subsequent manual change to Normal cancels the pending schedule.
        handle
            .set_mode(SmartGridMode::Normal, false, None)
            .await
            .unwrap();
        assert!(
            handle.scheduled_resume_at().await.unwrap().is_none(),
            "manual mode change must clear the pending auto-resume"
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn cancel_scheduled_resume_clears_pending_without_changing_mode() {
        let cancel = CancellationToken::new();
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let (handle, _join) =
            test_support::spawn_accepting_test_gpio(state, test_config(), cancel.clone());

        let explicit = SystemTime::now() + Duration::from_hours(5);
        handle
            .set_mode(SmartGridMode::Blocking, true, Some(explicit))
            .await
            .unwrap()
            .expect("scheduled");

        // Cancel hits the take()+abort() branch (pending was Some).
        handle.cancel_scheduled_resume().await.unwrap();
        assert!(handle.scheduled_resume_at().await.unwrap().is_none());
        // Mode is untouched by a bare cancel.
        assert!(matches!(
            handle.read_mode().await.unwrap(),
            SmartGridMode::Blocking
        ));

        // Idempotent: cancelling again with nothing pending is a no-op.
        handle.cancel_scheduled_resume().await.unwrap();
        assert!(handle.scheduled_resume_at().await.unwrap().is_none());
        cancel.cancel();
    }

    #[tokio::test]
    async fn auto_resume_timer_fires_and_flips_back_to_normal() {
        // Schedule a resume in the immediate past so the timer task's
        // duration_since check fails (already elapsed) and it posts
        // ResumeFire right away, exercising on_resume_fire end-to-end.
        let cancel = CancellationToken::new();
        let state = PriceState::new("SE3".to_string());
        state.update_prices(make_run(60, &[0.10, 0.10]), vec![]);
        let (handle, _join) =
            test_support::spawn_accepting_test_gpio(state, test_config(), cancel.clone());

        // resume_at = now (already due). The timer task sees remaining==zero /
        // an elapsed instant and posts ResumeFire immediately.
        let due = SystemTime::now();
        handle
            .set_mode(SmartGridMode::Blocking, true, Some(due))
            .await
            .unwrap()
            .expect("scheduled at a due instant");

        // Wait for the timer task to post ResumeFire and the actor to flip.
        let mut flipped = false;
        for _ in 0..100 {
            if matches!(handle.read_mode().await.unwrap(), SmartGridMode::Normal) {
                flipped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(flipped, "auto-resume timer must flip mode back to Normal");
        // The fired slot cleared itself.
        assert!(handle.scheduled_resume_at().await.unwrap().is_none());
        cancel.cancel();
    }

    #[test]
    fn set_mode_if_not_superseded_rejects_stale_generation() {
        // The generation guard on the controller (called by on_resume_fire)
        // must reject a fire whose snapshot generation no longer matches the
        // current one — the in-memory mode stays put.
        let mut gpio = GpioController::new_for_test_accepting(20, 21, false);
        gpio.set_mode(SmartGridMode::Blocking).unwrap();
        let snapshot = gpio.mode_generation();

        // A manual override bumps the generation past the snapshot.
        gpio.bump_mode_generation();

        // Now the would-be resume fire is superseded → Ok(false), no flip.
        let applied = gpio
            .set_mode_if_not_superseded(SmartGridMode::Normal, snapshot)
            .unwrap();
        assert!(!applied, "stale fire must be rejected");
        assert!(matches!(gpio.read_mode(), SmartGridMode::Blocking));

        // A matching generation applies the flip.
        let current = gpio.mode_generation();
        let applied = gpio
            .set_mode_if_not_superseded(SmartGridMode::Normal, current)
            .unwrap();
        assert!(applied, "matching generation must apply");
        assert!(matches!(gpio.read_mode(), SmartGridMode::Normal));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn push_pump_none_is_noop() {
        // No panic, no work. Just confirm the branch handles None.
        push_pump_to_homey(None, SmartGridMode::Blocking);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn push_pump_blocking_pushes_false_and_updates_watch() {
        let state: SharedMock = Arc::default();
        let addr = spawn_mock(state.clone()).await;
        let (hooks, mut rx) = make_hooks(addr);

        push_pump_to_homey(Some(&hooks), SmartGridMode::Blocking);

        // Watch update is synchronous — no waiting needed.
        rx.changed().await.unwrap();
        assert!(!(*rx.borrow()));

        wait_for_sets(&state, 1).await;
        assert_eq!(state.lock().unwrap().set_calls, vec![false]);

        // Cache was written-fresh by the push completion.
        let snap = hooks.cache.read().await;
        assert_eq!(snap.actual, Some(false));
        assert!(!snap.stale);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn push_pump_normal_pushes_true() {
        let state: SharedMock = Arc::default();
        let addr = spawn_mock(state.clone()).await;
        let (hooks, _rx) = make_hooks(addr);

        push_pump_to_homey(Some(&hooks), SmartGridMode::Normal);

        wait_for_sets(&state, 1).await;
        assert_eq!(state.lock().unwrap().set_calls, vec![true]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn push_pump_unreachable_homey_marks_cache_stale() {
        // Point at a port nobody's listening on so reqwest fails fast.
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let (hooks, _rx) = make_hooks(addr);

        push_pump_to_homey(Some(&hooks), SmartGridMode::Blocking);

        // Wait for the spawned task to fail and mark stale.
        for _ in 0..200 {
            let snap = hooks.cache.read().await;
            if snap.stale {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("cache never became stale despite Homey being unreachable");
    }

    // ── Warm-by heat-up ──────────────────────────────────────────────

    use super::test_support::spawn_accepting_test_gpio_with_modbus;

    /// Modbus fake returning a fixed temperature for any read.
    struct FakeReader(f32);
    #[async_trait::async_trait]
    impl ModbusWriter for FakeReader {
        async fn write_scaled(&self, _addr: u16, _v: f32) -> Result<(), String> {
            Ok(())
        }
        async fn read_scaled(&self, _addr: u16) -> Result<f32, String> {
            Ok(self.0)
        }
    }

    /// Build a bare actor (not spawned) so the sync fire handlers can be
    /// driven directly. Returns the receiver to keep `self_tx` alive.
    fn build_actor(
        modbus: Option<Arc<dyn ModbusWriter>>,
    ) -> (SmartGridActor, mpsc::Receiver<SmartGridCmd>) {
        let (tx, rx) = mpsc::channel(32);
        let actor = SmartGridActor {
            gpio: GpioController::new_for_test_accepting(20, 21, false),
            scheduled_resume: None,
            scheduled_warmby: None,
            price_state: PriceState::new("SE3".to_string()),
            config: test_config(),
            self_tx: tx,
            modbus,
            homey: None,
        };
        (actor, rx)
    }

    fn warmby_cmd(heatup_start: Option<SystemTime>) -> WarmByCommand {
        WarmByCommand { heatup_start }
    }

    #[tokio::test]
    async fn warm_by_skip_blocks_without_scheduling() {
        let cancel = CancellationToken::new();
        let (handle, _join) = spawn_accepting_test_gpio_with_modbus(
            PriceState::new("SE3".to_string()),
            test_config(),
            None,
            cancel.clone(),
        );
        let scheduled = handle
            .schedule_warm_by(warmby_cmd(None))
            .await
            .expect("schedule_warm_by ok");
        assert!(scheduled.is_none(), "skip → nothing scheduled");
        assert!(matches!(
            handle.read_mode().await.unwrap(),
            SmartGridMode::Blocking
        ));
        assert!(handle.scheduled_resume_at().await.unwrap().is_none());
        cancel.cancel();
    }

    #[tokio::test]
    async fn warm_by_schedules_phase_a_and_surfaces_pending() {
        let cancel = CancellationToken::new();
        let (handle, _join) = spawn_accepting_test_gpio_with_modbus(
            PriceState::new("SE3".to_string()),
            test_config(),
            Some(Arc::new(FakeReader(20.0))),
            cancel.clone(),
        );
        let start = SystemTime::now() + Duration::from_hours(3);
        let scheduled = handle
            .schedule_warm_by(warmby_cmd(Some(start)))
            .await
            .expect("ok")
            .expect("a heat-up start is scheduled");
        assert_eq!(scheduled, start);
        // Pending future heat-up start is surfaced for the dashboard.
        assert_eq!(handle.scheduled_resume_at().await.unwrap(), Some(start));
        assert!(matches!(
            handle.read_mode().await.unwrap(),
            SmartGridMode::Blocking
        ));
        cancel.cancel();
    }

    #[tokio::test]
    async fn warm_by_manual_override_cancels_pending_heatup() {
        let cancel = CancellationToken::new();
        let (handle, _join) = spawn_accepting_test_gpio_with_modbus(
            PriceState::new("SE3".to_string()),
            test_config(),
            Some(Arc::new(FakeReader(20.0))),
            cancel.clone(),
        );
        let start = SystemTime::now() + Duration::from_hours(3);
        handle
            .schedule_warm_by(warmby_cmd(Some(start)))
            .await
            .unwrap()
            .expect("scheduled");
        // A manual mode change clears the pending warm-by.
        handle
            .set_mode(SmartGridMode::Normal, false, None)
            .await
            .unwrap();
        assert!(handle.scheduled_resume_at().await.unwrap().is_none());
        assert!(matches!(
            handle.read_mode().await.unwrap(),
            SmartGridMode::Normal
        ));
        cancel.cancel();
    }

    #[tokio::test]
    async fn heatup_done_fire_reblocks_on_generation_match() {
        let (mut actor, _rx) = build_actor(None);
        actor.gpio.set_mode(SmartGridMode::Normal).unwrap();
        let generation = actor.gpio.mode_generation();
        let dummy = tokio::spawn(async {}).abort_handle();
        actor.scheduled_warmby = Some(WarmBySlot {
            heatup_start: SystemTime::now(),
            task: dummy,
            generation,
        });
        actor.on_heatup_done_fire(generation);
        assert!(matches!(actor.gpio.read_mode(), SmartGridMode::Blocking));
        assert!(actor.scheduled_warmby.is_none(), "slot cleared after done");
    }

    #[tokio::test]
    async fn heatup_done_fire_superseded_leaves_mode_untouched() {
        let (mut actor, _rx) = build_actor(None);
        actor.gpio.set_mode(SmartGridMode::Normal).unwrap();
        let stale = actor.gpio.mode_generation();
        // A manual change happened since the watcher captured its generation.
        actor.gpio.bump_mode_generation();
        let dummy = tokio::spawn(async {}).abort_handle();
        actor.scheduled_warmby = Some(WarmBySlot {
            heatup_start: SystemTime::now(),
            task: dummy,
            generation: stale,
        });
        actor.on_heatup_done_fire(stale);
        assert!(
            matches!(actor.gpio.read_mode(), SmartGridMode::Normal),
            "stale done-fire must not re-block"
        );
    }

    #[tokio::test]
    async fn heatup_start_fire_flips_to_normal_and_keeps_slot() {
        let (mut actor, _rx) = build_actor(Some(Arc::new(FakeReader(50.0))));
        actor.gpio.set_mode(SmartGridMode::Blocking).unwrap();
        let generation = actor.gpio.mode_generation();
        let dummy = tokio::spawn(async {}).abort_handle();
        actor.scheduled_warmby = Some(WarmBySlot {
            heatup_start: SystemTime::now(),
            task: dummy,
            generation,
        });
        actor.on_heatup_start_fire(generation);
        assert!(
            matches!(actor.gpio.read_mode(), SmartGridMode::Normal),
            "phase A flips to Normal"
        );
        assert!(
            actor.scheduled_warmby.is_some(),
            "slot retained for the phase-B watcher"
        );
    }

    #[tokio::test]
    async fn heatup_start_fire_superseded_does_nothing() {
        let (mut actor, _rx) = build_actor(Some(Arc::new(FakeReader(50.0))));
        actor.gpio.set_mode(SmartGridMode::Blocking).unwrap();
        let stale = actor.gpio.mode_generation();
        actor.gpio.bump_mode_generation();
        actor.on_heatup_start_fire(stale);
        assert!(
            matches!(actor.gpio.read_mode(), SmartGridMode::Blocking),
            "stale start-fire must not flip to Normal"
        );
    }

    /// Regression: a cancel must invalidate an already-queued start-fire. The
    /// phase-A timer can leave its sleep and queue `HeatupStartFire` before the
    /// cancel's abort lands; without a generation bump in `cancel_all_schedules`
    /// that stale fire would flip to Normal and, with the slot cleared, strand
    /// the system in Normal with no watcher.
    #[tokio::test]
    async fn cancel_then_stale_start_fire_keeps_blocking() {
        let (mut actor, _rx) = build_actor(Some(Arc::new(FakeReader(50.0))));
        actor.gpio.set_mode(SmartGridMode::Blocking).unwrap();
        let stale = actor.gpio.mode_generation();
        let dummy = tokio::spawn(async {}).abort_handle();
        actor.scheduled_warmby = Some(WarmBySlot {
            heatup_start: SystemTime::now(),
            task: dummy,
            generation: stale,
        });
        // Simulate DELETE /scheduled_resume landing before the queued fire.
        actor.cancel_all_schedules();
        actor.on_heatup_start_fire(stale);
        assert!(
            matches!(actor.gpio.read_mode(), SmartGridMode::Blocking),
            "stale start-fire after cancel must not flip to Normal"
        );
        assert!(actor.scheduled_warmby.is_none());
    }
}
