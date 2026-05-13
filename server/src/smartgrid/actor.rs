//! SmartGrid actor.
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
use tracing::{debug, error, info, warn};

use crate::config::SmartGridConfig;
use crate::energy::price::PriceState;
use crate::energy::tibber::parse_iso8601;
use crate::homey::HomeyClient;
use crate::homey::cache::HomeyPumpCache;

use super::gpio::GpioController;
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
}

/// Compute the pump's desired state from the `SmartGrid` mode.
///
/// Pump OFF only when actively blocking; in every other mode it should run.
#[must_use]
pub fn pump_on_for(mode: SmartGridMode) -> bool {
    !matches!(mode, SmartGridMode::Blocking)
}

/// Fire-and-forget: push the pump state implied by `mode` to Homey, and
/// publish the desired state synchronously so the reconciliation poller
/// observes it immediately. On push failure the cache is marked stale; the
/// poller's next tick retries.
pub fn push_pump_to_homey(hooks: Option<&HomeyHooks>, mode: SmartGridMode) {
    let Some(hooks) = hooks else { return };
    let on = pump_on_for(mode);
    let _ = hooks.desired_tx.send(on);
    let client = hooks.client.clone();
    let cache = hooks.cache.clone();
    tokio::spawn(async move {
        match client.set_pump_onoff(on).await {
            Ok(()) => cache.write_fresh(on).await,
            Err(e) => {
                tracing::warn!("Homey pump push failed: {e}");
                cache.mark_stale().await;
            }
        }
    });
}

/// Errors a SetMode command can surface.
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
    ) -> Result<Option<SystemTime>, SmartGridError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(SmartGridCmd::SetMode {
                mode,
                schedule_resume,
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

struct SmartGridActor {
    gpio: GpioController,
    scheduled_resume: Option<ScheduledResume>,
    price_state: PriceState,
    config: SmartGridConfig,
    /// Cloned for the resume timer task to post back `ResumeFire`.
    self_tx: mpsc::Sender<SmartGridCmd>,
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
        price_state,
        config,
        self_tx: tx.clone(),
        homey,
    };
    let join = tokio::spawn(actor.run(rx, cancel));
    Ok((SmartGridHandle { tx }, join))
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
                cmd = rx.recv() => match cmd {
                    Some(c) => self.handle(c),
                    None => {
                        info!("SmartGrid actor: all senders dropped, exiting");
                        break;
                    }
                }
            }
        }
        // Abort any pending resume timer so we don't leak it past shutdown.
        if let Some(scheduled) = self.scheduled_resume.take() {
            scheduled.timer_task.abort();
        }
    }

    fn handle(&mut self, cmd: SmartGridCmd) {
        match cmd {
            SmartGridCmd::SetMode {
                mode,
                schedule_resume,
                respond_to,
            } => {
                let result = self.do_set_mode(mode, schedule_resume);
                let _ = respond_to.send(result);
            }
            SmartGridCmd::ReadMode { respond_to } => {
                let _ = respond_to.send(Ok(self.gpio.read_mode()));
            }
            SmartGridCmd::ModeChangedAt { respond_to } => {
                let _ = respond_to.send(Ok(self.gpio.mode_changed_at()));
            }
            SmartGridCmd::ScheduledResumeAt { respond_to } => {
                let _ = respond_to.send(self.scheduled_resume.as_ref().map(|s| s.fires_at));
            }
            SmartGridCmd::CancelScheduledResume { respond_to } => {
                if let Some(prev) = self.scheduled_resume.take() {
                    prev.timer_task.abort();
                    debug!(
                        "Cancelled pending auto-resume scheduled at {:?}",
                        prev.fires_at
                    );
                }
                let _ = respond_to.send(());
            }
            SmartGridCmd::ResumeFire {
                fires_at,
                generation,
            } => self.on_resume_fire(fires_at, generation),
        }
    }

    fn do_set_mode(
        &mut self,
        mode: SmartGridMode,
        schedule_resume: bool,
    ) -> Result<Option<SystemTime>, ApplyModeError> {
        // Bump generation FIRST. Any in-flight resume timer that has already
        // passed its sleep will see a mismatched generation when its
        // ResumeFire message arrives and bail out.
        self.gpio.bump_mode_generation();

        // Always cancel a prior schedule before mutating: a manual change
        // overrides any pending auto-flip.
        if let Some(prev) = self.scheduled_resume.take() {
            prev.timer_task.abort();
        }

        self.gpio.set_mode(mode).map_err(ApplyModeError::Gpio)?;

        // Pump tracks SmartGrid mode: ON for Normal/LowPrice/Overcapacity,
        // OFF for Blocking. Fire-and-forget — must not stall the actor.
        push_pump_to_homey(self.homey.as_ref(), mode);

        if mode == SmartGridMode::Normal || !schedule_resume || !self.config.auto_resume_enabled {
            return Ok(None);
        }

        let window = Duration::from_secs(self.config.auto_resume_window_hours.saturating_mul(3600));
        let fires_at = match mode {
            SmartGridMode::Blocking => self
                .price_state
                .cheapest_within(window)
                .and_then(|slot| parse_iso8601(&slot.starts_at).ok()),
            SmartGridMode::LowPrice | SmartGridMode::Overcapacity => {
                self.price_state.cheap_window_end(window)
            }
            SmartGridMode::Normal => unreachable!("returned above"),
        };

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
                let step = remaining.min(Duration::from_secs(60));
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
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Test-only helpers that let the smartgrid actor be exercised without
    //! real `/dev/gpiochip0` access. Mirrors `GpioController::new_for_test`.

    use super::*;

    /// Spawn the actor with a test-only GpioController (no hardware ioctls).
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
            price_state,
            config,
            self_tx: tx.clone(),
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
        (
            HomeyHooks {
                client,
                cache,
                desired_tx,
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
        SmartGridConfig {
            auto_resume_enabled: true,
            auto_resume_window_hours: 8,
        }
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

    /// Regression for Critical #2 from the code review: concurrent SetMode
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
        let t1 = tokio::spawn(async move { h1.set_mode(SmartGridMode::Blocking, false).await });
        let t2 = tokio::spawn(async move { h2.set_mode(SmartGridMode::LowPrice, false).await });

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
            .set_mode(SmartGridMode::Blocking, false)
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
            handle.set_mode(SmartGridMode::Normal, false).await,
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
}
