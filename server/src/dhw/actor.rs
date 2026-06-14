//! `DhwActor` skeleton and crash-recovery prologue.
//!
//! This task (Task 6) introduces the actor's *shape* and the recovery
//! sequence that runs at startup when a persisted boost is found. The
//! command-handler bodies for `SetComfort`, `StartShower`, `StartBath`,
//! and `Cancel` land in later tasks (7, 8, 11, 13) and return placeholder
//! errors here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::dhw::state::{DhwPersistedState, DhwSnapshot};

/// Narrow trait covering the Modbus writes/reads the DHW actor needs.
///
/// Methods operate in *scaled physical units* (e.g. °C, kW) — the real
/// implementation forwards to the existing `CtcActor`, which divides by the
/// parameter's scaling factor internally before writing the raw u16.
#[async_trait::async_trait]
pub trait ModbusWriter: Send + Sync {
    async fn write_scaled(&self, addr: u16, value: f32) -> Result<(), String>;
    async fn read_scaled(&self, addr: u16) -> Result<f32, String>;
}

/// Narrow trait for `SmartGrid` mode application used by the DHW actor.
#[async_trait::async_trait]
pub trait SgController: Send + Sync {
    async fn set_normal(&self) -> Result<(), String>;
    async fn set_overcapacity(&self) -> Result<(), String>;
}

/// Crash-recovery prologue. Runs once at actor startup when a persisted
/// boost is found in the JSON file; restores the heater to a safe state
/// and clears the file.
///
/// Sequence:
/// 1. `61503 = 0` — stop any heater-side hot-water boost.
/// 2. `61591 = 0` — close the immersion gate.
/// 3. `61636 = prior_c` — restore the immersion engage temp (Bath only).
/// 4. SG → Normal.
/// 5. Clear the boost-override watch channel so the Homey reconciler
///    stops forcing the override.
/// 6. Write an empty `DhwPersistedState` back to disk.
///
/// Caller MUST run [`rollback_partial_bath`] on any Err — persistence happens
/// after this returns Ok, so a mid-sequence failure would otherwise leave the
/// heater in `Overcapacity` / `61636 = engage_temp` / `boost_override = Some`
/// with no persisted state for recovery to unwind.
#[allow(clippy::too_many_arguments)] // narrow internal helper; one call site
async fn apply_bath_side_effects(
    modbus: &dyn ModbusWriter,
    sg: &dyn SgController,
    boost_override_tx: &tokio::sync::watch::Sender<Option<bool>>,
    cfg: &crate::config::DhwConfig,
    prior_engage_c: f32,
    hours: f32,
    spot_f32: f32,
    cheap: bool,
) -> Result<bool, crate::dhw::error::DhwError> {
    use crate::dhw::error::DhwError;

    boost_override_tx
        .send(Some(false))
        .map_err(|_| DhwError::HomeyOverrideSendFailed)?;
    sg.set_overcapacity().await.map_err(DhwError::SmartGrid)?;
    // Skip the 61636 write when the heater is already at engage_temp — saves
    // an EEPROM flash cycle. Step is 1.0 °C so 0.5 epsilon is safe.
    if (prior_engage_c - cfg.immersion_engage_temp_c).abs() > 0.5 {
        modbus
            .write_scaled(61636, cfg.immersion_engage_temp_c)
            .await
            .map_err(DhwError::Modbus)?;
    }
    modbus
        .write_scaled(61503, hours)
        .await
        .map_err(DhwError::Modbus)?;

    let mut gate = crate::dhw::immersion::ImmersionGate::new(
        cfg.immersion_allow_price_sek_per_kwh,
        cfg.immersion_hysteresis_sek_per_kwh,
    );
    match gate.evaluate(spot_f32, cheap) {
        crate::dhw::immersion::ImmersionDecision::Engage => {
            modbus
                .write_scaled(61591, cfg.immersion_kw_when_allowed)
                .await
                .map_err(DhwError::Modbus)?;
            Ok(true)
        }
        crate::dhw::immersion::ImmersionDecision::Disengage
        | crate::dhw::immersion::ImmersionDecision::NoChange => Ok(false),
    }
}

/// Best-effort undo of any partial Bath side effects. Errors are logged and
/// swallowed — we already have an Err to return to the caller, and leaving
/// the heater in a slightly-wrong state is preferable to masking the
/// original error.
async fn rollback_partial_bath(
    modbus: &dyn ModbusWriter,
    sg: &dyn SgController,
    boost_override_tx: &tokio::sync::watch::Sender<Option<bool>>,
    prior_c: f32,
) {
    if let Err(e) = modbus.write_scaled(61503, 0.0).await {
        tracing::warn!("DHW Bath start rollback: 61503=0 failed: {e}");
    }
    if let Err(e) = modbus.write_scaled(61591, 0.0).await {
        tracing::warn!("DHW Bath start rollback: 61591=0 failed: {e}");
    }
    if let Err(e) = modbus.write_scaled(61636, prior_c).await {
        tracing::warn!("DHW Bath start rollback: 61636 restore failed: {e}");
    }
    if let Err(e) = sg.set_normal().await {
        tracing::warn!("DHW Bath start rollback: SG=Normal failed: {e}");
    }
    let _ = boost_override_tx.send(None);
}

pub async fn run_recovery(
    persist_path: &Path,
    modbus: &dyn ModbusWriter,
    sg: &dyn SgController,
    boost_override_tx: &watch::Sender<Option<bool>>,
) -> Result<(), String> {
    let mut state = DhwPersistedState::load(persist_path).map_err(|e| e.to_string())?;
    let Some(boost) = state.boost.take() else {
        return Ok(());
    };

    modbus.write_scaled(61503, 0.0).await?;
    modbus.write_scaled(61591, 0.0).await?;
    if let Some(prior_c) = boost.prior_immersion_engage_temp_c {
        modbus.write_scaled(61636, prior_c).await?;
    }
    sg.set_normal().await?;
    let _ = boost_override_tx.send(None);

    let empty = DhwPersistedState::default();
    empty.save(persist_path).map_err(|e| e.to_string())?;
    tracing::warn!("DHW recovery cleared mid-flight boost from previous run ({boost:?})");
    Ok(())
}

/// Write the comfort level to `61500` (factor 1.0; scaled == raw).
///
/// `ComfortLevel::Manuell` is **not** a writable target — the heater
/// derives it from `61500=3` paired with `61501`. Calls with `Manuell`
/// return an error and do not touch Modbus.
///
/// # Errors
/// Returns `DhwError::Modbus` when `level` is `Manuell`, or when the
/// underlying Modbus write fails.
pub async fn write_comfort(
    modbus: &dyn ModbusWriter,
    level: crate::dhw::error::ComfortLevel,
) -> Result<(), crate::dhw::error::DhwError> {
    let scaled = level
        .as_scaled()
        .ok_or(crate::dhw::error::DhwError::Modbus(
            "ComfortLevel::Manuell is not a writable target".into(),
        ))?;
    modbus
        .write_scaled(61500, scaled)
        .await
        .map_err(crate::dhw::error::DhwError::Modbus)
}

/// Activation path for UC-A (Shower preset).
///
/// Pre-flight: reads stop-temp `62001` and the cached `DhwUpper` sample
/// (sensor backed by `62276` / `CTC_ACTUAL_TEMP_DHW`). If `dhw_upper >=
/// target` the heater would do nothing useful, so we short-circuit with
/// `AlreadyAtTarget` — no Modbus writes, no override send, no persistence.
///
/// Otherwise the activation:
/// 1. Sets the boost override to `Some(false)` so the Homey reconciler
///    forces the heater's own "extra hot water" toggle off — we drive the
///    boost ourselves via `61503`.
/// 2. Writes `61503 = 0.5` (hours). The actor divides by the parameter's
///    0.5 scaling factor and writes raw 1 to the device.
///
/// Returns `Started { scheduled_end }` on success; the caller is
/// responsible for stashing state, persisting, and spawning the watcher.
///
/// # Errors
/// * `DhwError::Modbus` if the `62001` read or the `61503` write fails.
/// * `DhwError::Sensor("dhw_upper")` if the storage has no cached
///   `DhwUpper` sample yet.
/// * `DhwError::HomeyOverrideSendFailed` if no Homey reconciler is
///   listening on the boost-override watch channel.
pub async fn start_shower_impl(
    modbus: &dyn ModbusWriter,
    store: &crate::storage::Store,
    boost_override_tx: &tokio::sync::watch::Sender<Option<bool>>,
    duration_minutes: u32,
) -> Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError> {
    use crate::dhw::error::DhwError;

    let target_c = modbus.read_scaled(62001).await.map_err(DhwError::Modbus)?;

    let dhw_c = store
        .latest_sample(crate::storage::Sensor::DhwUpper)
        .map(|(_, v)| v)
        .ok_or(DhwError::Sensor("dhw_upper"))?;

    if dhw_c >= target_c {
        return Ok(crate::dhw::error::StartReport::AlreadyAtTarget { dhw_c, target_c });
    }

    boost_override_tx
        .send(Some(false))
        .map_err(|_| DhwError::HomeyOverrideSendFailed)?;
    // 30 min = 0.5 h. Actor divides by factor 0.5 -> raw 1.
    modbus
        .write_scaled(61503, 0.5)
        .await
        .map_err(DhwError::Modbus)?;

    let scheduled_end = chrono::Utc::now() + chrono::Duration::minutes(i64::from(duration_minutes));
    Ok(crate::dhw::error::StartReport::Started { scheduled_end })
}

/// Activation path for UC-B (Bath preset).
///
/// Pre-flight gates run before any side effect, so an invalid request leaves
/// the heater untouched:
///
/// 1. **Range check** on `hours` (`[0.5, cfg.bath_max_hours]` in 0.5 steps).
///    Returns `DhwError::HoursOutOfRange`.
/// 2. **Cheap-band gate** on the current `PriceLevel`. The slot covering
///    "now" must be `VeryCheap` or `Cheap`; otherwise we abort with
///    `DhwError::PriceNotCheap` and a stringified level so the HTTP surface
///    can include it in the error body. A missing current slot or a slot
///    with `level: None` (price data hasn't been classified yet) is treated
///    as not-cheap by the same rule.
///
/// Once both gates pass we snapshot `61636` for restore at Bath stop, then
/// apply side effects in order:
///
/// 3. boost-override = `Some(false)` — disables Homey's own "extra hot
///    water" toggle so the heater's boost is driven solely by `61503`.
/// 4. `SmartGrid` → Overcapacity — pumps the buffer with whatever capacity
///    the grid currently offers.
/// 5. `61636 = cfg.immersion_engage_temp_c` — sets the immersion-engage
///    storage temperature for the duration of the Bath.
/// 6. `61503 = hours` — kicks off the heater-side boost timer. The actor
///    divides by the 0.5 scaling factor internally, so `hours=2.0` writes
///    raw `4` to the device.
/// 7. Immersion gate first-evaluation — if `spot_sek < on_threshold` we
///    write `61591 = cfg.immersion_kw_when_allowed` and flag the gate as
///    engaged. Otherwise we leave `61591` untouched.
///
/// On a write failure mid-sequence the heater is left in a partial state;
/// recovery happens via the persisted-state machinery (Task 13's stop
/// sequence covers cancel-path cleanup).
///
/// # Errors
/// * `DhwError::HoursOutOfRange` — `hours` outside `[0.5, bath_max_hours]`
///   or not a multiple of 0.5.
/// * `DhwError::PriceNotCheap` — current `PriceLevel` is not
///   `VeryCheap`/`Cheap` (or unavailable).
/// * `DhwError::Modbus` — any `61636` read or `61636`/`61503`/`61591` write
///   fails.
/// * `DhwError::HomeyOverrideSendFailed` — no Homey reconciler is
///   listening on the boost-override watch channel.
/// * `DhwError::SmartGrid` — the SG controller's Overcapacity write fails.
pub async fn start_bath_impl(
    modbus: &dyn ModbusWriter,
    sg: &dyn SgController,
    price: &crate::energy::price::PriceState,
    boost_override_tx: &tokio::sync::watch::Sender<Option<bool>>,
    cfg: &crate::config::DhwConfig,
    hours: f32,
) -> Result<
    (
        crate::dhw::error::StartReport,
        crate::dhw::state::DhwBoostState,
    ),
    crate::dhw::error::DhwError,
> {
    use crate::dhw::error::DhwError;

    // 1. Range validation. 0.5h steps; reject NaN by failing the lower bound
    //    check (NaN < 0.5 is false) AND the upper bound check, so we must use
    //    `!is_finite` as a guard. Plain `<` / `>` is enough because NaN fails
    //    both comparisons and falls into the step check, where NaN/0.5 is NaN
    //    and (NaN - NaN).abs() is NaN — which is not `<= 1e-3`, so it
    //    correctly returns the error.
    let max = cfg.bath_max_hours;
    if !hours.is_finite()
        || hours < 0.5
        || hours > max
        || ((hours / 0.5).round() - hours / 0.5).abs() > 1e-3
    {
        return Err(DhwError::HoursOutOfRange { min: 0.5, max });
    }

    // 2. Cheap-band gate. Look up the slot covering "now". A missing slot or
    //    one without a classified level fails the gate; the caller sees
    //    `PriceNotCheap { current_level: "Unknown" }` and can retry once the
    //    price-fetch loop has classified the day.
    let current = price.get_current();
    // f64 spot_sek -> f32 for the immersion gate, which works in SEK/kWh and
    // doesn't need sub-1e-7 precision.
    #[allow(clippy::cast_possible_truncation)]
    let spot_f32 = current.as_ref().map_or(0.0_f32, |p| p.spot_sek as f32);
    let level = current.as_ref().and_then(|p| p.level);
    let cheap = matches!(
        level,
        Some(crate::energy::price::PriceLevel::VeryCheap | crate::energy::price::PriceLevel::Cheap)
    );
    if !cheap {
        let level_str = level.map_or_else(|| "Unknown".to_string(), |l| format!("{l:?}"));
        return Err(DhwError::PriceNotCheap {
            current_level: level_str,
        });
    }

    // 3. Snapshot 61636 for restore.
    let prior_c = modbus.read_scaled(61636).await.map_err(DhwError::Modbus)?;

    // 4. Side effects with best-effort rollback. If any step after the first
    //    write fails, undo what we wrote so the heater isn't left in
    //    Overcapacity / engage_temp / boost-override state with no
    //    `DhwBoostState` to drive recovery on next restart.
    let res = apply_bath_side_effects(
        modbus,
        sg,
        boost_override_tx,
        cfg,
        prior_c,
        hours,
        spot_f32,
        cheap,
    )
    .await;
    let immersion_engaged = match res {
        Ok(engaged) => engaged,
        Err(e) => {
            rollback_partial_bath(modbus, sg, boost_override_tx, prior_c).await;
            return Err(e);
        }
    };

    let started_at = chrono::Utc::now();
    // `hours` is in [0.5, bath_max_hours], so hours * 3600 fits comfortably
    // in u64 without truncation/sign issues.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let duration_secs = (hours * 3600.0).round() as u64;
    let scheduled_end =
        started_at + chrono::Duration::seconds(i64::try_from(duration_secs).unwrap_or(i64::MAX));
    let state = crate::dhw::state::DhwBoostState {
        preset: crate::dhw::state::BoostPreset::Bath { hours },
        started_at,
        duration_secs,
        prior_immersion_engage_temp_c: Some(prior_c),
        immersion_engaged,
    };
    Ok((
        crate::dhw::error::StartReport::Started { scheduled_end },
        state,
    ))
}

/// Stop sequence for an active Bath boost.
///
/// Runs the cleanup writes that mirror `start_bath_impl`'s side effects, in
/// the order documented in the DHW Controls spec §3.2:
///
/// 1. `61503 = 0` — stop the heater-side boost timer. Skipped when `reason ==
///    TimerExpired` (the heater already has its own timer at 0; one fewer
///    flash write).
/// 2. `61591 = 0` — close the immersion gate. Skipped when
///    `state.immersion_engaged == false` (the gate never wrote a non-zero
///    value, so no need to clear it).
/// 3. `61636 = prior_c` — restore the immersion engage temperature snapshotted
///    at Bath start. Skipped when no snapshot is present (defensive — Bath
///    always snapshots, but Shower doesn't, and the Shower branch returns
///    early before reaching this fn anyway).
/// 4. SG → Normal — always.
/// 5. `boost_override_tx.send(None)` — release Homey's "extra hot water"
///    toggle. Always.
///
/// Shower's preset returns `Ok(())` immediately; its stop path is handled by
/// the Shower watcher (Task 9), which only clears the boost override. No
/// Modbus writes happen on Shower stop — flash-frugal by design.
///
/// State clearing (the in-memory `state` field, watcher handles, and
/// persistence file) is the **caller's** responsibility. This fn touches only
/// the heater + SG + override channel; even on partial-failure paths the
/// caller proceeds to clear local state so the next process startup can run
/// recovery.
///
/// # Errors
/// * `DhwError::Modbus` — any of the `61503`/`61591`/`61636` writes fail.
/// * `DhwError::SmartGrid` — SG Normal write fails.
/// * `DhwError::HomeyOverrideSendFailed` — no reconciler is listening.
async fn stop_boost(
    state: &crate::dhw::state::DhwBoostState,
    reason: crate::dhw::error::CancelReason,
    modbus: &dyn ModbusWriter,
    sg: &dyn SgController,
    boost_override_tx: &tokio::sync::watch::Sender<Option<bool>>,
) -> Result<(), crate::dhw::error::DhwError> {
    use crate::dhw::error::{CancelReason, DhwError};

    // Shower stop has no Modbus/SG cleanup — caller handles it via the
    // watcher path. Manual cancel of an active Shower is rejected upstream
    // (ShowerCannotBeCancelled), so we shouldn't normally reach this fn with
    // a Shower preset; the early return is defensive.
    if !matches!(state.preset, crate::dhw::state::BoostPreset::Bath { .. }) {
        return Ok(());
    }

    // 1. `61503 = 0` — skip on TimerExpired (heater's own timer already 0).
    if !matches!(reason, CancelReason::TimerExpired) {
        modbus
            .write_scaled(61503, 0.0)
            .await
            .map_err(DhwError::Modbus)?;
    }
    // 2. `61591 = 0` — only if the gate previously wrote a non-zero value.
    if state.immersion_engaged {
        modbus
            .write_scaled(61591, 0.0)
            .await
            .map_err(DhwError::Modbus)?;
    }
    // 3. `61636 = prior_c` — always restore when we have a snapshot.
    if let Some(prior_c) = state.prior_immersion_engage_temp_c {
        modbus
            .write_scaled(61636, prior_c)
            .await
            .map_err(DhwError::Modbus)?;
    }
    // 4. SG back to Normal.
    sg.set_normal().await.map_err(DhwError::SmartGrid)?;
    // 5. Release the boost override.
    boost_override_tx
        .send(None)
        .map_err(|_| DhwError::HomeyOverrideSendFailed)?;

    Ok(())
}

/// Commands accepted by the actor. Handlers for everything except
/// `Snapshot` are stubbed for now and land in Tasks 7-13.
pub enum DhwCmd {
    Snapshot {
        respond_to: oneshot::Sender<DhwSnapshot>,
    },
    SetComfort {
        level: crate::dhw::error::ComfortLevel,
        respond_to: oneshot::Sender<Result<(), crate::dhw::error::DhwError>>,
    },
    StartShower {
        respond_to:
            oneshot::Sender<Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError>>,
    },
    StartBath {
        hours: f32,
        respond_to:
            oneshot::Sender<Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError>>,
    },
    /// Manual cancel — only the HTTP DELETE endpoint sends this. Watcher-
    /// initiated stops use the dedicated `bath_watcher_done_rx` path with
    /// a richer [`CancelReason`].
    Cancel {
        respond_to: oneshot::Sender<Result<bool, crate::dhw::error::DhwError>>,
    },
    /// Persist current state synchronously and ack. Used by the graceful-
    /// shutdown hook in `main.rs` so a Bath in progress survives `SIGTERM`.
    ShutdownSave {
        respond_to: oneshot::Sender<Result<(), crate::dhw::error::DhwError>>,
    },
}

/// Cloneable handle to the DHW actor. Acquired by `DhwActor::spawn` (Task 14).
#[derive(Clone)]
pub struct DhwHandle {
    tx: mpsc::Sender<DhwCmd>,
}

impl DhwHandle {
    /// Fetch a snapshot of the actor's current state.
    ///
    /// # Panics
    /// Panics if the actor task has been dropped — the dashboard and HTTP
    /// routes assume the actor lives for the lifetime of the process.
    pub async fn snapshot(&self) -> DhwSnapshot {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(DhwCmd::Snapshot { respond_to: tx })
            .await
            .expect("dhw actor down");
        rx.await.expect("dhw actor dropped snapshot reply")
    }

    /// Set the heater's comfort program (`61500`).
    ///
    /// # Panics
    /// Panics if the actor task is down or drops its reply.
    ///
    /// # Errors
    /// Forwards any `DhwError` returned by the actor's `SetComfort` handler.
    pub async fn set_comfort(
        &self,
        level: crate::dhw::error::ComfortLevel,
    ) -> Result<(), crate::dhw::error::DhwError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(DhwCmd::SetComfort {
                level,
                respond_to: tx,
            })
            .await
            .expect("dhw actor down");
        rx.await.expect("dhw actor dropped reply")
    }

    /// Begin the Shower preset (UC-A, 30-minute boost).
    ///
    /// # Panics
    /// Panics if the actor task is down or drops its reply.
    ///
    /// # Errors
    /// Forwards any `DhwError` from `StartShower`.
    pub async fn start_shower(
        &self,
    ) -> Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(DhwCmd::StartShower { respond_to: tx })
            .await
            .expect("dhw actor down");
        rx.await.expect("dhw actor dropped reply")
    }

    /// Begin the Bath preset (UC-B). Implementation lands in Task 11; for
    /// now the actor returns a placeholder error.
    ///
    /// # Panics
    /// Panics if the actor task is down or drops its reply.
    ///
    /// # Errors
    /// Forwards any `DhwError` from `StartBath`.
    pub async fn start_bath(
        &self,
        hours: f32,
    ) -> Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(DhwCmd::StartBath {
                hours,
                respond_to: tx,
            })
            .await
            .expect("dhw actor down");
        rx.await.expect("dhw actor dropped reply")
    }

    /// Cancel an active boost. Returns `Ok(true)` for a successful Bath
    /// cancel, `Ok(false)` when nothing is active (idempotent no-op), and
    /// `Err(ShowerCannotBeCancelled)` for an active Shower.
    ///
    /// # Panics
    /// Panics if the actor task is down or drops its reply.
    ///
    /// # Errors
    /// Forwards any `DhwError` from `Cancel`.
    pub async fn cancel(&self) -> Result<bool, crate::dhw::error::DhwError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(DhwCmd::Cancel { respond_to: tx })
            .await
            .expect("dhw actor down");
        rx.await.expect("dhw actor dropped reply")
    }

    /// Test-only constructor that wraps a raw mpsc sender into a `DhwHandle`.
    /// Used by route integration tests to fake the actor with a lightweight
    /// inline task.
    #[cfg(test)]
    pub(crate) fn from_sender(tx: mpsc::Sender<DhwCmd>) -> Self {
        Self { tx }
    }

    /// Persist any active boost to the actor's `persist_path` and wait for
    /// ack. Called from the graceful-shutdown hook in `main.rs`. Idempotent
    /// no-op when no boost is active or no `persist_path` is configured.
    ///
    /// # Errors
    /// * `DhwError::Persistence` — the actor has shut down before the command
    ///   landed, dropped the reply, or the underlying `.save()` failed.
    pub async fn shutdown_save(&self) -> Result<(), crate::dhw::error::DhwError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(DhwCmd::ShutdownSave { respond_to: tx })
            .await
            .map_err(|_| crate::dhw::error::DhwError::Persistence("actor down".into()))?;
        rx.await
            .map_err(|_| crate::dhw::error::DhwError::Persistence("dropped".into()))?
    }
}

/// The DHW actor. Owns its receiver, the persisted boost state (if any),
/// and the trait-object collaborators it talks to.
pub struct DhwActor {
    rx: mpsc::Receiver<DhwCmd>,
    state: Option<crate::dhw::state::DhwBoostState>,
    watcher_abort: Option<tokio::task::AbortHandle>,
    /// Oneshot receiver fed by the active Shower watcher (Task 9) when it
    /// finishes naturally. `run()` selects on this alongside `rx`.
    watcher_done_rx: Option<oneshot::Receiver<()>>,
    /// Oneshot receiver fed by the active Bath watcher (Task 12) with the
    /// `CancelReason` that fired. Kept separate from `watcher_done_rx` to
    /// keep the Shower path untouched.
    bath_watcher_done_rx: Option<oneshot::Receiver<crate::dhw::error::CancelReason>>,
    /// Side-channel from the Bath watcher carrying immersion-gate transitions
    /// (option (c) in the Task-12 design — avoids wrapping `state` in a
    /// shared mutex). The actor mirrors `ImmersionEngaged(bool)` events into
    /// `self.state.immersion_engaged` and re-persists.
    bath_event_rx: Option<mpsc::Receiver<crate::dhw::watcher::BathWatcherEvent>>,
    modbus: Arc<dyn ModbusWriter>,
    sg: Arc<dyn SgController>,
    boost_override_tx: watch::Sender<Option<bool>>,
    store: crate::storage::Store,
    price_state: Arc<crate::energy::price::PriceState>,
    cfg: crate::config::DhwConfig,
    persist_path: Option<PathBuf>,
    last_comfort: Option<crate::dhw::error::ComfortLevel>,
}

impl DhwActor {
    /// Construct a `DhwActor` wired to the real collaborators and spawn it
    /// on the current Tokio runtime. Returns the cheap-clone handle used by
    /// HTTP routes and the graceful-shutdown hook.
    pub fn spawn(
        modbus: Arc<dyn ModbusWriter>,
        sg: Arc<dyn SgController>,
        boost_override_tx: watch::Sender<Option<bool>>,
        store: crate::storage::Store,
        price_state: Arc<crate::energy::price::PriceState>,
        cfg: crate::config::DhwConfig,
    ) -> DhwHandle {
        let (tx, rx) = mpsc::channel::<DhwCmd>(32);
        let persist_path = cfg.persist_path.clone();
        let actor = DhwActor {
            rx,
            state: None,
            watcher_abort: None,
            watcher_done_rx: None,
            bath_watcher_done_rx: None,
            bath_event_rx: None,
            modbus,
            sg,
            boost_override_tx,
            store,
            price_state,
            cfg,
            persist_path,
            last_comfort: None,
        };
        tokio::spawn(actor.run());
        DhwHandle { tx }
    }

    /// Run the actor loop until the command channel closes.
    ///
    /// The prologue performs crash recovery if `persist_path` is set, then
    /// the loop dispatches `DhwCmd` variants to their (currently placeholder)
    /// handlers.
    pub async fn run(mut self) {
        if let Some(p) = self.persist_path.clone()
            && let Err(e) =
                run_recovery(&p, &*self.modbus, &*self.sg, &self.boost_override_tx).await
        {
            tracing::warn!("DHW recovery failed: {e}");
        }
        // Seed `last_comfort` from the heater so the first snapshot reflects
        // reality. Factor=1.0 for 61500, so scaled == raw. The heater only
        // ever stores 0/1/2/3 here; unknown values map to Manuell via
        // `ComfortLevel::from_raw`.
        match self.modbus.read_scaled(61500).await {
            Ok(v) => {
                // Cast is safe: 61500 is a small integer enum (0..=3) in f32.
                #[allow(clippy::cast_possible_truncation)]
                let raw = v as i32;
                self.last_comfort = Some(crate::dhw::error::ComfortLevel::from_raw(raw));
            }
            Err(e) => tracing::warn!("DHW startup 61500 read failed: {e}"),
        }
        // Receive loop: select on `rx.recv()` plus the active watcher's
        // `done_rx`. When a watcher fires naturally, the actor handles the
        // natural-stop cleanup (clear state, persist empty, drop the
        // `AbortHandle`).
        loop {
            // The watcher-done branches are each gated on their receiver
            // being `Some(_)` and use `as_mut().expect(...)` so the inner
            // `await` operates on a mutable borrow of the receiver (Tokio's
            // `oneshot::Receiver` and `mpsc::Receiver` are both `Unpin`).
            // The select is cancellation-safe — if a different branch fires
            // first, the receivers are preserved for the next iteration.
            let shower_active = self.watcher_done_rx.is_some();
            let bath_active = self.bath_watcher_done_rx.is_some();
            let bath_events_active = self.bath_event_rx.is_some();
            tokio::select! {
                cmd = self.rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    self.handle_cmd(cmd).await;
                }
                _ = async { self.watcher_done_rx.as_mut().expect("guarded by shower_active").await }, if shower_active => {
                    self.on_watcher_done();
                }
                reason = async { self.bath_watcher_done_rx.as_mut().expect("guarded by bath_active").await }, if bath_active => {
                    let reason = reason.unwrap_or(crate::dhw::error::CancelReason::TimerExpired);
                    self.on_bath_watcher_done(reason).await;
                }
                ev = async { self.bath_event_rx.as_mut().expect("guarded by bath_events_active").recv().await }, if bath_events_active => {
                    if let Some(ev) = ev {
                        self.on_bath_event(ev);
                    }
                }
            }
        }
    }

    async fn handle_cmd(&mut self, cmd: DhwCmd) {
        match cmd {
            DhwCmd::Snapshot { respond_to } => {
                // Re-read 61500 so a comfort change made at the heater's
                // physical panel (not via our HTTP) shows up in the dashboard
                // within one poll cycle.
                if let Ok(v) = self.modbus.read_scaled(61500).await {
                    #[allow(clippy::cast_possible_truncation)]
                    let raw = v as i32;
                    self.last_comfort = Some(crate::dhw::error::ComfortLevel::from_raw(raw));
                }
                let _ = respond_to.send(self.snapshot());
            }
            DhwCmd::SetComfort { level, respond_to } => {
                let result = write_comfort(&*self.modbus, level).await;
                if result.is_ok() {
                    self.last_comfort = Some(level);
                }
                let _ = respond_to.send(result);
            }
            DhwCmd::StartShower { respond_to } => {
                let result = if self.state.is_some() {
                    Err(crate::dhw::error::DhwError::BoostAlreadyActive)
                } else {
                    start_shower_impl(
                        &*self.modbus,
                        &self.store,
                        &self.boost_override_tx,
                        self.cfg.shower_duration_minutes,
                    )
                    .await
                };
                if matches!(&result, Ok(crate::dhw::error::StartReport::Started { .. })) {
                    let started_at = chrono::Utc::now();
                    let duration_secs =
                        u64::from(self.cfg.shower_duration_minutes).saturating_mul(60);
                    let state = crate::dhw::state::DhwBoostState {
                        preset: crate::dhw::state::BoostPreset::Shower,
                        started_at,
                        duration_secs,
                        prior_immersion_engage_temp_c: None,
                        immersion_engaged: false,
                    };
                    self.state = Some(state.clone());
                    if let Some(p) = self.persist_path.as_ref() {
                        let persisted = crate::dhw::state::DhwPersistedState {
                            schema_version: 1,
                            boost: Some(state),
                        };
                        if let Err(e) = persisted.save(p) {
                            tracing::warn!("DHW persist after StartShower failed: {e}");
                        }
                    }
                    // Spawn the 30-min watcher: clears the boost override and
                    // signals back via `done_tx` when the timer elapses. Store
                    // the AbortHandle so a future cancel (Task 13) can drop it.
                    let (done_tx, done_rx) = oneshot::channel();
                    let boost_tx = self.boost_override_tx.clone();
                    let task = tokio::spawn(crate::dhw::watcher::run_shower_watcher(
                        boost_tx,
                        std::time::Duration::from_secs(duration_secs),
                        done_tx,
                    ));
                    self.watcher_abort = Some(task.abort_handle());
                    self.watcher_done_rx = Some(done_rx);
                }
                let _ = respond_to.send(result);
            }
            DhwCmd::StartBath { hours, respond_to } => {
                let result = self.handle_start_bath(hours).await;
                let _ = respond_to.send(result);
            }
            DhwCmd::Cancel { respond_to } => {
                let result = self.cancel_manual().await;
                let _ = respond_to.send(result);
            }
            DhwCmd::ShutdownSave { respond_to } => {
                let result = self.handle_shutdown_save();
                let _ = respond_to.send(result);
            }
        }
    }

    /// Persist the current in-memory state to `persist_path`. No-op when
    /// either is `None` so the graceful-shutdown hook never errors on
    /// idle/persist-disabled deployments.
    fn handle_shutdown_save(&self) -> Result<(), crate::dhw::error::DhwError> {
        let (Some(p), Some(state)) = (self.persist_path.as_ref(), self.state.as_ref()) else {
            return Ok(());
        };
        let persisted = crate::dhw::state::DhwPersistedState {
            schema_version: 1,
            boost: Some(state.clone()),
        };
        persisted
            .save(p)
            .map_err(|e| crate::dhw::error::DhwError::Persistence(e.to_string()))
    }

    /// Run the Bath pre-flight + side effects via `start_bath_impl`, then
    /// stash state, persist, and spawn the 60s watcher on success. Extracted
    /// from `handle_cmd` to keep that match readable.
    async fn handle_start_bath(
        &mut self,
        hours: f32,
    ) -> Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError> {
        if self.state.is_some() {
            return Err(crate::dhw::error::DhwError::BoostAlreadyActive);
        }
        let (report, state) = start_bath_impl(
            &*self.modbus,
            &*self.sg,
            &self.price_state,
            &self.boost_override_tx,
            &self.cfg,
            hours,
        )
        .await?;

        let duration_secs = state.duration_secs;
        let immersion_engaged = state.immersion_engaged;
        self.state = Some(state.clone());
        if let Some(p) = self.persist_path.as_ref() {
            let persisted = crate::dhw::state::DhwPersistedState {
                schema_version: 1,
                boost: Some(state),
            };
            if let Err(e) = persisted.save(p) {
                tracing::warn!("DHW persist after StartBath failed: {e}");
            }
        }

        // Spawn the 60s Bath watcher. The watcher owns the stop-trigger
        // evaluation and the immersion-gate re-evaluation; the actor mirrors
        // immersion events back into `self.state` and reacts to `done_rx` for
        // the cancel reason (Task 13 will add the Modbus/SG cleanup).
        let (done_tx, done_rx) = oneshot::channel();
        let (ev_tx, ev_rx) = mpsc::channel(8);
        let started_at = tokio::time::Instant::now();
        let task = tokio::spawn(crate::dhw::watcher::run_bath_watcher(
            std::time::Duration::from_secs(duration_secs),
            started_at,
            immersion_engaged,
            self.modbus.clone(),
            self.store.clone(),
            self.price_state.clone(),
            self.cfg.clone(),
            ev_tx,
            done_tx,
        ));
        self.watcher_abort = Some(task.abort_handle());
        self.bath_watcher_done_rx = Some(done_rx);
        self.bath_event_rx = Some(ev_rx);
        Ok(report)
    }

    /// Manual-cancel arm of `DhwCmd::Cancel`. Returns:
    /// * `Ok(false)` — no active boost (idempotent no-op).
    /// * `Err(ShowerCannotBeCancelled)` — Shower runs to completion.
    /// * `Ok(true)` — active Bath cancelled. The Bath stop sequence
    ///   (`stop_boost`) runs FIRST (touches `61503`/`61591`/`61636`/SG/
    ///   override), THEN local state is cleared. If `stop_boost` fails
    ///   partway through, the error is propagated to the caller — local
    ///   state is **not** cleared in that case, so the next manual cancel
    ///   (or restart-recovery) can have another go.
    async fn cancel_manual(&mut self) -> Result<bool, crate::dhw::error::DhwError> {
        let Some(state) = self.state.as_ref() else {
            return Ok(false);
        };
        match state.preset {
            crate::dhw::state::BoostPreset::Shower => {
                Err(crate::dhw::error::DhwError::ShowerCannotBeCancelled)
            }
            crate::dhw::state::BoostPreset::Bath { .. } => {
                // Run the cleanup writes BEFORE clearing local state. On
                // error we propagate without clearing — the caller / user
                // can retry.
                stop_boost(
                    state,
                    crate::dhw::error::CancelReason::Manual,
                    &*self.modbus,
                    &*self.sg,
                    &self.boost_override_tx,
                )
                .await?;
                if let Some(h) = self.watcher_abort.take() {
                    h.abort();
                }
                self.watcher_done_rx = None;
                self.bath_watcher_done_rx = None;
                self.bath_event_rx = None;
                self.state = None;
                if let Some(p) = self.persist_path.as_ref() {
                    let empty = crate::dhw::state::DhwPersistedState::default();
                    if let Err(e) = empty.save(p) {
                        tracing::warn!("DHW persist after Bath cancel failed: {e}");
                    }
                }
                Ok(true)
            }
        }
    }

    /// Handle natural watcher completion: the watcher has already cleared the
    /// boost override, so we just drop our tracking state and persistence.
    fn on_watcher_done(&mut self) {
        self.state = None;
        self.watcher_abort = None;
        self.watcher_done_rx = None;
        if let Some(p) = self.persist_path.as_ref() {
            let empty = crate::dhw::state::DhwPersistedState::default();
            if let Err(e) = empty.save(p) {
                tracing::warn!("DHW persist after watcher completion failed: {e}");
            }
        }
        tracing::info!("DHW Shower watcher completed; state cleared");
    }

    /// Handle a Bath-watcher stop trigger.
    ///
    /// Runs the shared `stop_boost` cleanup (`61503=0` unless reason ==
    /// `TimerExpired`, `61591=0` if the gate was engaged, `61636` restore,
    /// SG=Normal, override release), then clears local state and persistence.
    ///
    /// Unlike `cancel_manual`, this path is fire-and-forget from the
    /// watcher's perspective — the watcher has already exited by the time we
    /// run. So on `stop_boost` error we log a warning and clear local state
    /// anyway. The heater may be in an inconsistent state until the next
    /// process restart, at which point recovery (`run_recovery`) cleans it
    /// up — but with `state` cleared, the actor isn't pretending a boost is
    /// still active.
    async fn on_bath_watcher_done(&mut self, reason: crate::dhw::error::CancelReason) {
        tracing::info!("DHW Bath watcher stopped: {reason:?}; running stop sequence");
        // Watcher task has exited; drain its channels either way.
        self.watcher_abort = None;
        self.bath_watcher_done_rx = None;
        self.bath_event_rx = None;

        let Some(state) = self.state.as_ref() else {
            return;
        };
        if let Err(e) = stop_boost(
            state,
            reason,
            &*self.modbus,
            &*self.sg,
            &self.boost_override_tx,
        )
        .await
        {
            // Preserve `self.state` and the persisted record so the next
            // process restart's recovery can finish the cleanup we couldn't
            // complete (typically a transient Modbus / SG error).
            tracing::warn!("DHW stop_boost failed during Bath watcher cleanup: {e:?}");
            return;
        }
        self.state = None;
        if let Some(p) = self.persist_path.as_ref() {
            let empty = crate::dhw::state::DhwPersistedState::default();
            if let Err(e) = empty.save(p) {
                tracing::warn!("DHW persist after Bath watcher stop failed: {e}");
            }
        }
    }

    /// Mirror an immersion-gate transition from the Bath watcher into
    /// `self.state` and re-persist. Keeps the snapshot/persist views in sync
    /// without the watcher and actor sharing a mutex.
    fn on_bath_event(&mut self, ev: crate::dhw::watcher::BathWatcherEvent) {
        let crate::dhw::watcher::BathWatcherEvent::ImmersionEngaged(engaged) = ev;
        let Some(state) = self.state.as_mut() else {
            return;
        };
        state.immersion_engaged = engaged;
        if let Some(p) = self.persist_path.as_ref() {
            let persisted = crate::dhw::state::DhwPersistedState {
                schema_version: 1,
                boost: Some(state.clone()),
            };
            if let Err(e) = persisted.save(p) {
                tracing::warn!("DHW persist after immersion event failed: {e}");
            }
        }
    }

    fn snapshot(&self) -> DhwSnapshot {
        use crate::dhw::state::DhwBoostSnapshot;
        DhwSnapshot {
            comfort_level: self
                .last_comfort
                .unwrap_or(crate::dhw::error::ComfortLevel::Manuell),
            boost: self.state.as_ref().map(|s| {
                let dur =
                    chrono::Duration::seconds(i64::try_from(s.duration_secs).unwrap_or(i64::MAX));
                let scheduled_end = s.started_at + dur;
                let now = chrono::Utc::now();
                DhwBoostSnapshot {
                    preset: s.preset,
                    started_at: s.started_at,
                    scheduled_end,
                    elapsed_s: (now - s.started_at).num_seconds(),
                    remaining_s: (scheduled_end - now).num_seconds().max(0),
                    immersion_engaged: s.immersion_engaged,
                }
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct FakeModbus {
        calls: Arc<Mutex<Vec<String>>>,
        reads: std::collections::HashMap<u16, f32>,
    }

    impl FakeModbus {
        fn new(calls: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                calls,
                reads: std::collections::HashMap::default(),
            }
        }
        fn with_reads(calls: Arc<Mutex<Vec<String>>>, reads: Vec<(u16, f32)>) -> Self {
            Self {
                calls,
                reads: reads.into_iter().collect(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModbusWriter for FakeModbus {
        async fn write_scaled(&self, addr: u16, v: f32) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("modbus_write_scaled {addr} = {v}"));
            Ok(())
        }
        async fn read_scaled(&self, addr: u16) -> Result<f32, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("modbus_read_scaled {addr}"));
            Ok(self.reads.get(&addr).copied().unwrap_or(0.0))
        }
    }

    struct FakeSg {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl FakeSg {
        fn new(calls: Arc<Mutex<Vec<String>>>) -> Self {
            Self { calls }
        }
    }

    #[async_trait::async_trait]
    impl SgController for FakeSg {
        async fn set_normal(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("sg_set_mode Normal".into());
            Ok(())
        }
        async fn set_overcapacity(&self) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push("sg_set_mode Overcapacity".into());
            Ok(())
        }
    }

    #[tokio::test]
    async fn crash_recovery_runs_documented_sequence() {
        use crate::dhw::state::{BoostPreset, DhwBoostState, DhwPersistedState};
        use chrono::Utc;
        let dir = tempfile::tempdir().unwrap();
        let persist = dir.path().join("dhw.json");
        DhwPersistedState {
            schema_version: 1,
            boost: Some(DhwBoostState {
                preset: BoostPreset::Bath { hours: 1.0 },
                started_at: Utc::now(),
                duration_secs: 3600,
                prior_immersion_engage_temp_c: Some(60.0),
                immersion_engaged: true,
            }),
        }
        .save(&persist)
        .unwrap();

        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let fake_modbus = FakeModbus::new(calls.clone());
        let fake_sg = FakeSg::new(calls.clone());
        let (override_tx, mut override_rx) =
            tokio::sync::watch::channel::<Option<bool>>(Some(false));

        run_recovery(&persist, &fake_modbus, &fake_sg, &override_tx)
            .await
            .unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            *calls,
            vec![
                "modbus_write_scaled 61503 = 0".to_string(),
                "modbus_write_scaled 61591 = 0".to_string(),
                "modbus_write_scaled 61636 = 60".to_string(),
                "sg_set_mode Normal".to_string(),
            ]
        );
        assert_eq!(*override_rx.borrow_and_update(), None);
        let after = DhwPersistedState::load(&persist).unwrap();
        assert!(after.boost.is_none());
    }

    #[tokio::test]
    async fn set_comfort_writes_61500_scaled_value() {
        use crate::dhw::error::ComfortLevel;
        let calls: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::default();
        let modbus = FakeModbus::new(calls.clone());

        let res = crate::dhw::actor::write_comfort(&modbus, ComfortLevel::Komfort).await;
        assert!(res.is_ok());
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["modbus_write_scaled 61500 = 2".to_string()]
        );
    }

    #[tokio::test]
    async fn set_comfort_rejects_manuell() {
        use crate::dhw::error::ComfortLevel;
        let calls: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::default();
        let modbus = FakeModbus::new(calls.clone());

        let res = crate::dhw::actor::write_comfort(&modbus, ComfortLevel::Manuell).await;
        assert!(res.is_err());
        // No Modbus write should have occurred.
        assert!(calls.lock().unwrap().is_empty());
    }

    fn test_store_with_dhw_upper(c: f32) -> (tempfile::TempDir, crate::storage::Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = crate::storage::Store::open(dir.path().join("ctc.redb")).expect("open store");
        store
            .record_sample(
                crate::storage::Sensor::DhwUpper,
                std::time::SystemTime::now(),
                c,
            )
            .expect("record dhw_upper sample");
        (dir, store)
    }

    #[tokio::test]
    async fn start_shower_writes_61503_and_sets_override_when_not_at_target() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::with_reads(calls.clone(), vec![(62001_u16, 55.0_f32)]);
        let (_dir, store) = test_store_with_dhw_upper(50.0);
        let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);

        let result = crate::dhw::actor::start_shower_impl(&modbus, &store, &boost_tx, 30).await;

        let report = result.unwrap();
        assert!(matches!(
            report,
            crate::dhw::error::StartReport::Started { .. }
        ));
        assert_eq!(*boost_rx.borrow_and_update(), Some(false));
        let log = calls.lock().unwrap();
        assert!(log.contains(&"modbus_read_scaled 62001".to_string()));
        assert!(log.contains(&"modbus_write_scaled 61503 = 0.5".to_string()));
    }

    #[tokio::test]
    async fn start_shower_short_circuits_when_already_at_target() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::with_reads(calls.clone(), vec![(62001_u16, 55.0_f32)]);
        let (_dir, store) = test_store_with_dhw_upper(56.0);
        let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);

        let report = crate::dhw::actor::start_shower_impl(&modbus, &store, &boost_tx, 30)
            .await
            .unwrap();
        assert!(matches!(
            report,
            crate::dhw::error::StartReport::AlreadyAtTarget { .. }
        ));
        assert_eq!(*boost_rx.borrow_and_update(), None);
        let log = calls.lock().unwrap();
        assert!(log.iter().all(|c| !c.starts_with("modbus_write_scaled")));
    }

    #[tokio::test(start_paused = true)]
    async fn shower_lifecycle_clears_state_after_duration() {
        // Full Shower lifecycle: send StartShower → advance virtual time
        // past the configured duration → assert that the actor cleared its
        // boost state, the override watch channel is back to None, and the
        // persisted file shows no active boost.
        use crate::dhw::error::StartReport;
        use crate::dhw::state::DhwPersistedState;

        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeModbus::with_reads(
            calls.clone(),
            vec![(62001_u16, 55.0_f32), (61500_u16, 1.0_f32)],
        ));
        let sg: Arc<dyn SgController> = Arc::new(FakeSg::new(calls.clone()));
        let (_dir_store, store) = test_store_with_dhw_upper(50.0);
        let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);

        let dir = tempfile::tempdir().unwrap();
        let persist_path = dir.path().join("dhw.json");

        // 1-minute duration so virtual time can sail past it quickly.
        let cfg = crate::config::DhwConfig {
            shower_duration_minutes: 1,
            ..crate::config::DhwConfig::default()
        };

        let (cmd_tx, cmd_rx) = mpsc::channel::<DhwCmd>(8);
        let actor = DhwActor {
            rx: cmd_rx,
            state: None,
            watcher_abort: None,
            watcher_done_rx: None,
            bath_watcher_done_rx: None,
            bath_event_rx: None,
            modbus,
            sg,
            boost_override_tx: boost_tx.clone(),
            store,
            price_state: Arc::new(crate::energy::price::PriceState::new("SE3".to_string())),
            cfg,
            persist_path: Some(persist_path.clone()),
            last_comfort: None,
        };

        let actor_task = tokio::spawn(actor.run());

        // Kick off Shower.
        let (start_tx, start_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::StartShower {
                respond_to: start_tx,
            })
            .await
            .unwrap();
        let report = start_rx.await.unwrap().unwrap();
        assert!(matches!(report, StartReport::Started { .. }));
        // Override was set to Some(false) during start.
        assert_eq!(*boost_rx.borrow_and_update(), Some(false));

        // Snapshot now should show an active boost.
        let (snap_tx, snap_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::Snapshot {
                respond_to: snap_tx,
            })
            .await
            .unwrap();
        let mid_snap = snap_rx.await.unwrap();
        assert!(mid_snap.boost.is_some());

        // Persisted file should reflect the boost.
        let mid_file = DhwPersistedState::load(&persist_path).unwrap();
        assert!(mid_file.boost.is_some());

        // Advance virtual time past the 1-min duration. We need to yield so
        // the actor's select loop observes the watcher's signal before we
        // ask for the next snapshot.
        tokio::time::advance(std::time::Duration::from_secs(70)).await;
        tokio::task::yield_now().await;

        // Snapshot should now show no active boost.
        let (snap_tx, snap_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::Snapshot {
                respond_to: snap_tx,
            })
            .await
            .unwrap();
        let after_snap = snap_rx.await.unwrap();
        assert!(after_snap.boost.is_none());

        // The override should be cleared.
        assert_eq!(*boost_rx.borrow_and_update(), None);

        // Persistence file should reflect cleared state.
        let after_file = DhwPersistedState::load(&persist_path).unwrap();
        assert!(after_file.boost.is_none());

        drop(cmd_tx);
        actor_task.await.unwrap();
    }

    /// Build a `PriceState` whose "now" slot has the given level/spot.
    ///
    /// We seed a slot that starts 1 min ago and ends 14 min from now, so
    /// `get_current()` returns it deterministically regardless of how long
    /// the test takes to set up.
    fn price_state_with_current(
        level: crate::energy::price::PriceLevel,
        spot_sek: f64,
    ) -> crate::energy::price::PriceState {
        let now = chrono::Utc::now();
        let start = now - chrono::Duration::minutes(1);
        let end = now + chrono::Duration::minutes(14);
        let mut point = crate::energy::price::PricePoint::from_spot(
            start.to_rfc3339(),
            end.to_rfc3339(),
            spot_sek,
            0.0,
            0.0,
        );
        point.level = Some(level);
        let state = crate::energy::price::PriceState::new("SE3".to_string());
        state.update_prices(vec![point], vec![]);
        state
    }

    #[tokio::test]
    async fn start_bath_rejects_hours_out_of_range() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::new(calls.clone());
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);
        let price = price_state_with_current(crate::energy::price::PriceLevel::VeryCheap, 0.10);
        let cfg = crate::config::DhwConfig::default(); // bath_max_hours = 2.0

        // 5.0 h is well above the 2.0 h max.
        let err = crate::dhw::actor::start_bath_impl(&modbus, &sg, &price, &boost_tx, &cfg, 5.0)
            .await
            .expect_err("must reject out-of-range hours");
        match err {
            crate::dhw::error::DhwError::HoursOutOfRange { min, max } => {
                assert!((min - 0.5).abs() < f32::EPSILON, "min: got {min}, want 0.5");
                assert!((max - 2.0).abs() < f32::EPSILON, "max: got {max}, want 2.0");
            }
            other => panic!("expected HoursOutOfRange, got {other:?}"),
        }
        // No Modbus / SG side effects must have run.
        assert!(calls.lock().unwrap().is_empty(), "no side effects allowed");
    }

    #[tokio::test]
    async fn start_bath_rejects_when_price_not_cheap() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::new(calls.clone());
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);
        let price = price_state_with_current(crate::energy::price::PriceLevel::Normal, 0.40);
        let cfg = crate::config::DhwConfig::default();

        let err = crate::dhw::actor::start_bath_impl(&modbus, &sg, &price, &boost_tx, &cfg, 1.0)
            .await
            .expect_err("must reject non-cheap price");
        match err {
            crate::dhw::error::DhwError::PriceNotCheap { current_level } => {
                assert_eq!(current_level, "Normal");
            }
            other => panic!("expected PriceNotCheap, got {other:?}"),
        }
        assert!(calls.lock().unwrap().is_empty(), "no side effects allowed");
    }

    #[tokio::test]
    async fn start_bath_writes_61636_61503_and_engages_immersion_when_cheap() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::with_reads(calls.clone(), vec![(61636_u16, 60.0_f32)]);
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);
        // spot=0.30 is below the immersion on-threshold (0.50 - 0.05 = 0.45),
        // and the slot is VeryCheap so the cheap-band gate passes.
        let price = price_state_with_current(crate::energy::price::PriceLevel::VeryCheap, 0.30);
        let cfg = crate::config::DhwConfig::default();

        let (report, state) =
            crate::dhw::actor::start_bath_impl(&modbus, &sg, &price, &boost_tx, &cfg, 2.0)
                .await
                .expect("start_bath should succeed");

        assert!(matches!(
            report,
            crate::dhw::error::StartReport::Started { .. }
        ));
        assert!(state.immersion_engaged, "immersion must be engaged");
        assert!(
            matches!(state.preset, crate::dhw::state::BoostPreset::Bath { hours }
                if (hours - 2.0).abs() < f32::EPSILON),
            "preset must be Bath(2.0)"
        );
        assert!(
            state
                .prior_immersion_engage_temp_c
                .is_some_and(|c| (c - 60.0).abs() < f32::EPSILON),
            "must snapshot prior 61636 = 60.0"
        );
        assert_eq!(state.duration_secs, 7200);

        // Override fired before any Modbus write.
        assert_eq!(*boost_rx.borrow_and_update(), Some(false));

        let log = calls.lock().unwrap();
        // Snapshot must happen before any write.
        assert!(
            log.contains(&"modbus_read_scaled 61636".to_string()),
            "must read 61636: {log:?}"
        );
        assert!(
            log.contains(&"sg_set_mode Overcapacity".to_string()),
            "must set SG=Overcapacity: {log:?}"
        );
        assert!(
            log.contains(&"modbus_write_scaled 61636 = 50".to_string()),
            "must write 61636 = 50: {log:?}"
        );
        assert!(
            log.contains(&"modbus_write_scaled 61503 = 2".to_string()),
            "must write 61503 = 2: {log:?}"
        );
        assert!(
            log.contains(&"modbus_write_scaled 61591 = 3".to_string()),
            "must write 61591 = 3 when immersion engages: {log:?}"
        );

        // Order check: 61636 read before any write; SG=Overcap before
        // 61636/61503 writes; 61503 before 61591.
        let pos = |needle: &str| {
            log.iter()
                .position(|s| s == needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        let read_61636 = pos("modbus_read_scaled 61636");
        let sg_overcap = pos("sg_set_mode Overcapacity");
        let write_61636 = pos("modbus_write_scaled 61636 = 50");
        let write_61503 = pos("modbus_write_scaled 61503 = 2");
        let write_61591 = pos("modbus_write_scaled 61591 = 3");
        assert!(read_61636 < sg_overcap, "read 61636 must precede SG write");
        assert!(sg_overcap < write_61636, "SG must precede 61636 write");
        assert!(write_61636 < write_61503, "61636 must precede 61503");
        assert!(write_61503 < write_61591, "61503 must precede 61591");
    }

    #[tokio::test]
    async fn start_bath_skips_immersion_when_spot_above_on_threshold() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::with_reads(calls.clone(), vec![(61636_u16, 55.0_f32)]);
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);
        // spot=0.50 is above on_threshold (0.50 - 0.05 = 0.45) but the slot
        // is still classified Cheap, so the cheap-band gate passes and the
        // Bath starts — only the immersion sub-gate stays closed.
        let price = price_state_with_current(crate::energy::price::PriceLevel::Cheap, 0.50);
        let cfg = crate::config::DhwConfig::default();

        let (_report, state) =
            crate::dhw::actor::start_bath_impl(&modbus, &sg, &price, &boost_tx, &cfg, 1.0)
                .await
                .expect("start_bath should succeed");

        assert!(
            !state.immersion_engaged,
            "immersion must NOT engage above on_threshold"
        );

        let log = calls.lock().unwrap();
        assert!(
            log.iter()
                .all(|c| !c.starts_with("modbus_write_scaled 61591")),
            "no 61591 write allowed when immersion stays disengaged: {log:?}"
        );
        // But the Bath itself still ran.
        assert!(log.contains(&"modbus_write_scaled 61503 = 1".to_string()));
        assert!(log.contains(&"modbus_write_scaled 61636 = 50".to_string()));
    }

    #[tokio::test]
    async fn start_bath_skips_61636_write_when_prior_equals_engage_temp() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        // prior 61636 already equals cfg.immersion_engage_temp_c (default 50.0)
        // → skip the EEPROM write entirely.
        let modbus = FakeModbus::with_reads(calls.clone(), vec![(61636_u16, 50.0_f32)]);
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);
        let price = price_state_with_current(crate::energy::price::PriceLevel::VeryCheap, 0.30);
        let cfg = crate::config::DhwConfig::default();

        crate::dhw::actor::start_bath_impl(&modbus, &sg, &price, &boost_tx, &cfg, 1.0)
            .await
            .expect("start_bath should succeed");

        let log = calls.lock().unwrap();
        assert!(
            log.iter()
                .all(|c| !c.starts_with("modbus_write_scaled 61636")),
            "no 61636 write expected when prior already equals engage_temp: {log:?}"
        );
        // 61503 (boost timer) is still written.
        assert!(log.contains(&"modbus_write_scaled 61503 = 1".to_string()));
    }

    #[tokio::test]
    async fn snapshot_refreshes_comfort_level_from_61500() {
        // Build an actor, seed 61500 = 2.0 (Komfort), send Snapshot, assert
        // last_comfort reflects the live heater value.
        use crate::dhw::error::ComfortLevel;
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeModbus::with_reads(
            calls.clone(),
            vec![(61500_u16, 2.0_f32)],
        ));
        let sg: Arc<dyn SgController> = Arc::new(FakeSg::new(calls.clone()));
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);
        let dir = tempfile::tempdir().unwrap();
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<DhwCmd>(8);
        let (_, store) = test_store_with_dhw_upper(50.0);
        let actor = crate::dhw::actor::DhwActor {
            rx: cmd_rx,
            state: None,
            watcher_abort: None,
            watcher_done_rx: None,
            bath_watcher_done_rx: None,
            bath_event_rx: None,
            modbus,
            sg,
            boost_override_tx: boost_tx,
            store,
            price_state: Arc::new(crate::energy::price::PriceState::new("SE3".to_string())),
            cfg: crate::config::DhwConfig::default(),
            persist_path: Some(dir.path().join("dhw.json")),
            last_comfort: None,
        };
        let actor_task = tokio::spawn(actor.run());

        let (snap_tx, snap_rx) = tokio::sync::oneshot::channel();
        cmd_tx
            .send(DhwCmd::Snapshot {
                respond_to: snap_tx,
            })
            .await
            .unwrap();
        let snap = snap_rx.await.unwrap();
        assert_eq!(snap.comfort_level, ComfortLevel::Komfort);

        drop(cmd_tx);
        let _ = actor_task.await;
    }

    /// Build a Bath `DhwBoostState` with the given immersion + prior-snapshot
    /// fields. `started_at` is `Utc::now()` and `duration_secs` is 1h — both
    /// irrelevant to `stop_boost`, which only reads the preset + immersion +
    /// prior fields.
    fn bath_state(
        immersion_engaged: bool,
        prior_immersion_engage_temp_c: Option<f32>,
    ) -> crate::dhw::state::DhwBoostState {
        crate::dhw::state::DhwBoostState {
            preset: crate::dhw::state::BoostPreset::Bath { hours: 1.0 },
            started_at: chrono::Utc::now(),
            duration_secs: 3600,
            prior_immersion_engage_temp_c,
            immersion_engaged,
        }
    }

    #[tokio::test]
    async fn stop_boost_timer_expiry_skips_61503_writes_other_cleanup() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::new(calls.clone());
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(Some(false));
        let state = bath_state(true, Some(60.0));

        super::stop_boost(
            &state,
            crate::dhw::error::CancelReason::TimerExpired,
            &modbus,
            &sg,
            &boost_tx,
        )
        .await
        .expect("stop_boost should succeed");

        let log = calls.lock().unwrap();
        // No 61503 write on TimerExpired.
        assert!(
            log.iter()
                .all(|c| !c.starts_with("modbus_write_scaled 61503")),
            "no 61503 write allowed on TimerExpired: {log:?}"
        );
        // But the rest of the cleanup ran.
        assert!(
            log.contains(&"modbus_write_scaled 61591 = 0".to_string()),
            "61591=0 must run when immersion was engaged: {log:?}"
        );
        assert!(
            log.contains(&"modbus_write_scaled 61636 = 60".to_string()),
            "61636 restore must run: {log:?}"
        );
        assert!(
            log.contains(&"sg_set_mode Normal".to_string()),
            "SG=Normal must run: {log:?}"
        );
        assert_eq!(*boost_rx.borrow_and_update(), None, "override must clear");
    }

    #[tokio::test]
    async fn stop_boost_manual_cancel_writes_61503_zero() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::new(calls.clone());
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(Some(false));
        let state = bath_state(true, Some(60.0));

        super::stop_boost(
            &state,
            crate::dhw::error::CancelReason::Manual,
            &modbus,
            &sg,
            &boost_tx,
        )
        .await
        .expect("stop_boost should succeed");

        let log = calls.lock().unwrap();
        assert!(
            log.contains(&"modbus_write_scaled 61503 = 0".to_string()),
            "61503=0 must run on Manual: {log:?}"
        );
        assert!(
            log.contains(&"modbus_write_scaled 61591 = 0".to_string()),
            "61591=0 must run when immersion engaged: {log:?}"
        );
        assert!(
            log.contains(&"modbus_write_scaled 61636 = 60".to_string()),
            "61636 restore must run: {log:?}"
        );
        assert!(
            log.contains(&"sg_set_mode Normal".to_string()),
            "SG=Normal must run: {log:?}"
        );
        assert_eq!(*boost_rx.borrow_and_update(), None, "override must clear");

        // Order check: 61503 before 61591 before 61636 before SG.
        let pos = |needle: &str| {
            log.iter()
                .position(|s| s == needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        let w_61503 = pos("modbus_write_scaled 61503 = 0");
        let w_61591 = pos("modbus_write_scaled 61591 = 0");
        let w_61636 = pos("modbus_write_scaled 61636 = 60");
        let sg_norm = pos("sg_set_mode Normal");
        assert!(w_61503 < w_61591, "61503 must precede 61591");
        assert!(w_61591 < w_61636, "61591 must precede 61636");
        assert!(w_61636 < sg_norm, "61636 must precede SG=Normal");
    }

    #[tokio::test]
    async fn stop_boost_immersion_not_engaged_skips_61591_write() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::new(calls.clone());
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(Some(false));
        let state = bath_state(false, Some(60.0));

        super::stop_boost(
            &state,
            crate::dhw::error::CancelReason::Manual,
            &modbus,
            &sg,
            &boost_tx,
        )
        .await
        .expect("stop_boost should succeed");

        let log = calls.lock().unwrap();
        assert!(
            log.iter()
                .all(|c| !c.starts_with("modbus_write_scaled 61591")),
            "no 61591 write allowed when immersion not engaged: {log:?}"
        );
        // The other writes still happen.
        assert!(log.contains(&"modbus_write_scaled 61503 = 0".to_string()));
        assert!(log.contains(&"modbus_write_scaled 61636 = 60".to_string()));
        assert!(log.contains(&"sg_set_mode Normal".to_string()));
    }

    #[tokio::test]
    async fn stop_boost_no_61636_snapshot_skips_61636_write() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::new(calls.clone());
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(Some(false));
        // Defensive case: Bath without a snapshot (shouldn't happen in
        // practice — start_bath always snapshots — but stop_boost must not
        // panic / write garbage).
        let state = bath_state(true, None);

        super::stop_boost(
            &state,
            crate::dhw::error::CancelReason::Manual,
            &modbus,
            &sg,
            &boost_tx,
        )
        .await
        .expect("stop_boost should succeed");

        let log = calls.lock().unwrap();
        assert!(
            log.iter()
                .all(|c| !c.starts_with("modbus_write_scaled 61636")),
            "no 61636 write allowed when no snapshot: {log:?}"
        );
        // The other writes still happen.
        assert!(log.contains(&"modbus_write_scaled 61503 = 0".to_string()));
        assert!(log.contains(&"modbus_write_scaled 61591 = 0".to_string()));
        assert!(log.contains(&"sg_set_mode Normal".to_string()));
    }

    #[tokio::test]
    async fn stop_boost_shower_preset_returns_early_with_no_writes() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus = FakeModbus::new(calls.clone());
        let sg = FakeSg::new(calls.clone());
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(Some(false));
        let state = crate::dhw::state::DhwBoostState {
            preset: crate::dhw::state::BoostPreset::Shower,
            started_at: chrono::Utc::now(),
            duration_secs: 1800,
            prior_immersion_engage_temp_c: None,
            immersion_engaged: false,
        };

        super::stop_boost(
            &state,
            crate::dhw::error::CancelReason::Manual,
            &modbus,
            &sg,
            &boost_tx,
        )
        .await
        .expect("stop_boost should succeed");

        assert!(
            calls.lock().unwrap().is_empty(),
            "Shower preset must short-circuit with zero side effects"
        );
    }

    #[tokio::test]
    async fn cancel_manual_active_bath_runs_stop_boost_and_clears_state() {
        // Integration: build a DhwActor with an active Bath state pre-seeded,
        // send DhwCmd::Cancel, and assert the full Bath stop sequence ran
        // (Modbus + SG + override) and that state + persistence are cleared.
        use crate::dhw::state::DhwPersistedState;

        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeModbus::with_reads(
            calls.clone(),
            vec![(61500_u16, 1.0_f32)],
        ));
        let sg: Arc<dyn SgController> = Arc::new(FakeSg::new(calls.clone()));
        let (_dir_store, store) = test_store_with_dhw_upper(50.0);
        let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(Some(false));

        let dir = tempfile::tempdir().unwrap();
        let persist_path = dir.path().join("dhw.json");

        // Pre-seed persistence to mirror an active Bath; the actor will load
        // it via run_recovery — but recovery itself touches Modbus, which
        // we'd rather not exercise here. So we leave persistence empty and
        // construct the actor with `state` already populated.
        let bath = crate::dhw::state::DhwBoostState {
            preset: crate::dhw::state::BoostPreset::Bath { hours: 1.0 },
            started_at: chrono::Utc::now(),
            duration_secs: 3600,
            prior_immersion_engage_temp_c: Some(60.0),
            immersion_engaged: true,
        };

        let (cmd_tx, cmd_rx) = mpsc::channel::<DhwCmd>(8);
        let actor = DhwActor {
            rx: cmd_rx,
            state: Some(bath.clone()),
            watcher_abort: None,
            watcher_done_rx: None,
            bath_watcher_done_rx: None,
            bath_event_rx: None,
            modbus,
            sg,
            boost_override_tx: boost_tx.clone(),
            store,
            price_state: Arc::new(crate::energy::price::PriceState::new("SE3".to_string())),
            cfg: crate::config::DhwConfig::default(),
            persist_path: Some(persist_path.clone()),
            last_comfort: None,
        };
        // Seed persistence so we can assert it gets cleared.
        crate::dhw::state::DhwPersistedState {
            schema_version: 1,
            boost: Some(bath),
        }
        .save(&persist_path)
        .unwrap();

        let actor_task = tokio::spawn(actor.run());

        let (cancel_tx, cancel_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::Cancel {
                respond_to: cancel_tx,
            })
            .await
            .unwrap();
        let cancelled = cancel_rx.await.unwrap().unwrap();
        assert!(
            cancelled,
            "cancel_manual should return true for active Bath"
        );

        // Snapshot should now show no active boost.
        let (snap_tx, snap_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::Snapshot {
                respond_to: snap_tx,
            })
            .await
            .unwrap();
        let after_snap = snap_rx.await.unwrap();
        assert!(after_snap.boost.is_none(), "state must be cleared");

        assert_eq!(
            *boost_rx.borrow_and_update(),
            None,
            "override must be released"
        );

        // Snapshot the log into a plain Vec so we don't hold the MutexGuard
        // across the `actor_task.await` below (clippy::await_holding_lock).
        let log_snapshot: Vec<String> = calls.lock().unwrap().clone();
        assert!(
            log_snapshot.contains(&"modbus_write_scaled 61503 = 0".to_string()),
            "Manual cancel must write 61503=0: {log_snapshot:?}"
        );
        assert!(
            log_snapshot.contains(&"modbus_write_scaled 61591 = 0".to_string()),
            "Manual cancel with immersion engaged must write 61591=0: {log_snapshot:?}"
        );
        assert!(
            log_snapshot.contains(&"modbus_write_scaled 61636 = 60".to_string()),
            "Manual cancel must restore 61636: {log_snapshot:?}"
        );
        assert!(
            log_snapshot.contains(&"sg_set_mode Normal".to_string()),
            "Manual cancel must set SG=Normal: {log_snapshot:?}"
        );

        let after_file = DhwPersistedState::load(&persist_path).unwrap();
        assert!(
            after_file.boost.is_none(),
            "persistence file must be cleared"
        );

        drop(cmd_tx);
        actor_task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn bath_watcher_done_natural_expiry_clears_state_after_stop_sequence() {
        // Verify the natural-expiry path: actor runs stop_boost (skipping
        // 61503=0 because reason==TimerExpired) and clears state + override
        // + persistence.
        use crate::dhw::state::DhwPersistedState;

        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        // 61636 read happens during start_bath_impl; reading 61500 happens at
        // actor startup.
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeModbus::with_reads(
            calls.clone(),
            vec![(61636_u16, 60.0_f32), (61500_u16, 1.0_f32)],
        ));
        let sg: Arc<dyn SgController> = Arc::new(FakeSg::new(calls.clone()));
        let (_dir_store, store) = test_store_with_dhw_upper(50.0);
        let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);

        let dir = tempfile::tempdir().unwrap();
        let persist_path = dir.path().join("dhw.json");

        // VeryCheap so the Bath start passes the price gate, and spot=0.30
        // engages the immersion gate.
        let price = price_state_with_current(crate::energy::price::PriceLevel::VeryCheap, 0.30);

        // Tiny duration: 0.5 h is the smallest allowed by start_bath_impl.
        // We'll advance virtual time past it. duration_secs = 1800.
        let cfg = crate::config::DhwConfig::default();

        let (cmd_tx, cmd_rx) = mpsc::channel::<DhwCmd>(8);
        let actor = DhwActor {
            rx: cmd_rx,
            state: None,
            watcher_abort: None,
            watcher_done_rx: None,
            bath_watcher_done_rx: None,
            bath_event_rx: None,
            modbus,
            sg,
            boost_override_tx: boost_tx.clone(),
            store,
            price_state: Arc::new(price),
            cfg,
            persist_path: Some(persist_path.clone()),
            last_comfort: None,
        };

        let actor_task = tokio::spawn(actor.run());

        // Start Bath: 0.5h.
        let (start_tx, start_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::StartBath {
                hours: 0.5,
                respond_to: start_tx,
            })
            .await
            .unwrap();
        let report = start_rx.await.unwrap().unwrap();
        assert!(matches!(
            report,
            crate::dhw::error::StartReport::Started { .. }
        ));
        // Override fired during start.
        assert_eq!(*boost_rx.borrow_and_update(), Some(false));

        // Persistence should now reflect the active Bath.
        let mid_file = DhwPersistedState::load(&persist_path).unwrap();
        assert!(
            mid_file.boost.is_some(),
            "persistence must show active Bath"
        );

        // Advance virtual time well past the 30-min duration. The Bath
        // watcher's 60s ticks (consume immediate first + tick) need a couple
        // of yields to evaluate, but a single large advance + yield works
        // because the timer-expiry check fires on the first real tick.
        tokio::time::advance(std::time::Duration::from_hours(1)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        // Snapshot: state cleared.
        let (snap_tx, snap_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::Snapshot {
                respond_to: snap_tx,
            })
            .await
            .unwrap();
        let after_snap = snap_rx.await.unwrap();
        assert!(
            after_snap.boost.is_none(),
            "state must be cleared after timer expiry"
        );

        // Override cleared.
        assert_eq!(*boost_rx.borrow_and_update(), None);

        // Persistence cleared.
        let after_file = DhwPersistedState::load(&persist_path).unwrap();
        assert!(after_file.boost.is_none());

        // The stop sequence ran: NO 61503=0 write (TimerExpired skips it),
        // YES 61591=0 (immersion was engaged), YES 61636=60 restore, YES
        // SG=Normal. Snapshot into a Vec so we don't hold the MutexGuard
        // across the actor_task.await below.
        let log_snapshot: Vec<String> = calls.lock().unwrap().clone();
        assert!(
            log_snapshot
                .iter()
                .filter(|c| c.as_str() == "modbus_write_scaled 61503 = 0")
                .count()
                == 0,
            "TimerExpired must NOT write 61503=0: {log_snapshot:?}"
        );
        assert!(
            log_snapshot.contains(&"modbus_write_scaled 61591 = 0".to_string()),
            "Bath cleanup must write 61591=0 when immersion engaged: {log_snapshot:?}"
        );
        assert!(
            log_snapshot.contains(&"modbus_write_scaled 61636 = 60".to_string()),
            "Bath cleanup must restore 61636: {log_snapshot:?}"
        );
        assert!(
            log_snapshot.contains(&"sg_set_mode Normal".to_string()),
            "Bath cleanup must set SG=Normal: {log_snapshot:?}"
        );

        drop(cmd_tx);
        actor_task.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_save_persists_active_boost() {
        use crate::dhw::state::{BoostPreset, DhwBoostState, DhwPersistedState};

        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        // 61500 is read once during the actor's startup prologue.
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeModbus::with_reads(
            calls.clone(),
            vec![(61500_u16, 1.0_f32)],
        ));
        let sg: Arc<dyn SgController> = Arc::new(FakeSg::new(calls.clone()));
        let (_dir_store, store) = test_store_with_dhw_upper(50.0);
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);

        let dir = tempfile::tempdir().unwrap();
        let persist_path = dir.path().join("dhw.json");

        let bath = DhwBoostState {
            preset: BoostPreset::Bath { hours: 1.0 },
            started_at: chrono::Utc::now(),
            duration_secs: 3600,
            prior_immersion_engage_temp_c: Some(60.0),
            immersion_engaged: true,
        };

        let (cmd_tx, cmd_rx) = mpsc::channel::<DhwCmd>(8);
        let actor = DhwActor {
            rx: cmd_rx,
            state: Some(bath.clone()),
            watcher_abort: None,
            watcher_done_rx: None,
            bath_watcher_done_rx: None,
            bath_event_rx: None,
            modbus,
            sg,
            boost_override_tx: boost_tx,
            store,
            price_state: Arc::new(crate::energy::price::PriceState::new("SE3".to_string())),
            cfg: crate::config::DhwConfig::default(),
            persist_path: Some(persist_path.clone()),
            last_comfort: None,
        };
        let actor_task = tokio::spawn(actor.run());

        let (resp_tx, resp_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::ShutdownSave {
                respond_to: resp_tx,
            })
            .await
            .unwrap();
        resp_rx.await.unwrap().expect("shutdown_save must succeed");

        let after = DhwPersistedState::load(&persist_path).unwrap();
        let saved = after.boost.expect("must persist the active boost");
        assert!(
            matches!(saved.preset, BoostPreset::Bath { hours } if (hours - 1.0).abs() < f32::EPSILON),
            "saved preset must match: {:?}",
            saved.preset
        );
        assert!(saved.immersion_engaged, "immersion flag must round-trip");

        drop(cmd_tx);
        actor_task.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_save_no_op_when_idle() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let modbus: Arc<dyn ModbusWriter> = Arc::new(FakeModbus::with_reads(
            calls.clone(),
            vec![(61500_u16, 1.0_f32)],
        ));
        let sg: Arc<dyn SgController> = Arc::new(FakeSg::new(calls.clone()));
        let (_dir_store, store) = test_store_with_dhw_upper(50.0);
        let (boost_tx, _boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);

        let dir = tempfile::tempdir().unwrap();
        let persist_path = dir.path().join("dhw.json");

        let (cmd_tx, cmd_rx) = mpsc::channel::<DhwCmd>(8);
        let actor = DhwActor {
            rx: cmd_rx,
            state: None,
            watcher_abort: None,
            watcher_done_rx: None,
            bath_watcher_done_rx: None,
            bath_event_rx: None,
            modbus,
            sg,
            boost_override_tx: boost_tx,
            store,
            price_state: Arc::new(crate::energy::price::PriceState::new("SE3".to_string())),
            cfg: crate::config::DhwConfig::default(),
            persist_path: Some(persist_path.clone()),
            last_comfort: None,
        };
        let actor_task = tokio::spawn(actor.run());

        let (resp_tx, resp_rx) = oneshot::channel();
        cmd_tx
            .send(DhwCmd::ShutdownSave {
                respond_to: resp_tx,
            })
            .await
            .unwrap();
        resp_rx.await.unwrap().expect("idle shutdown_save must Ok");

        // No file should have been created.
        assert!(
            !persist_path.exists(),
            "idle shutdown_save must not touch the persist file"
        );

        drop(cmd_tx);
        actor_task.await.unwrap();
    }
}
