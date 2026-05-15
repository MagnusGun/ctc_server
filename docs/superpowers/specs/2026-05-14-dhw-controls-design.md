# DHW Controls — Design Spec

**Date**: 2026-05-14
**Author**: Magnus Gunnarsson (brainstormed with Claude)
**Status**: Draft — pending user review
**Repo**: `ctc_server` (Rust workspace, Axum + tokio-modbus actor pattern)

---

## 1. Problem

Add user-facing controls for domestic hot water (DHW) production to the dashboard. Two related capabilities:

1. **DHW dropdown** — a new dashboard control, independent of the existing SmartGrid dropdown, that lets the user:
   - Fire a short "shower" boost (≈30 min, stop when tank reaches the heater's program target).
   - Fire a long "bath" boost (user-picked duration, max DHW prioritisation via SG=Overcapacity).
   - Set the persistent DHW comfort level (Economy / Normal / Komfort).
2. **Immersion-heater controller (Bath-only)** — automatically allow the DHW immersion heater (`61591` + `61636`) to assist *only during a Bath boost* and *only while electricity spot price is below an absolute threshold*. Hysteresis-guarded to limit Modbus writes (flash-wear concern on heater registers).

Out of scope:
- SmartGrid dropdown — unchanged.
- Manuell DHW mode (61500=3) — not exposed in UI; user will switch heater to Normal manually at the pump before this feature ships.
- Heating circ-pump control — uses the existing `HomeyClient::set_pump_onoff` integration; no new transport needed.
- Always-on immersion controller — explicitly rejected. Immersion only ever runs during a Bath boost.

---

## 2. Background

### 2.1 CTC EcoHeat 400 DHW model (per service docs)

- `61500 Varmvatten mode` (RW, scaling 1, values 0-3): persistent program selector.
  - `0 = Ekonomi` (program stop temp 50 °C)
  - `1 = Normal` (program stop temp 55 °C)
  - `2 = Komfort` (program stop temp 58 °C)
  - `3 = Manuell` (program stop temp from `61501`)
- `61501 Manuell Stopptemp Varmvatten` (RW, scaling 0.1, °C): active only when `61500=3`.
- `61503 Extra varmvatten timer` (RW, scaling 0.5, hours):
  - Live constraints read from heater: `min=0`, `max=20` (raw) → 0.0–10.0 h, step `1` (raw) → 0.5 h.
  - Writing raw `N` triggers Extra DHW for `N × 0.5` hours. Heater raises DHW stop temp to ~60 °C internally during boost. Writing `0` cancels.
  - 30 min boost = raw `1`. 1 h = raw `2`. Max 10 h = raw `20`.
- `61591 Max elpatron VV kW / Övre` (RW, scaling 0.1, kW): cap on immersion-heater power for DHW. Currently `0.0` on this rig (immersion fully disabled). Setting to non-zero kW *permits* immersion to engage when heater logic calls for it — but does **not** by itself force engagement during Extra-DHW; see `61636`.
- `61636 Elpanna extra VV °C / Boiler DHW °C` (RW, scaling 0.1, °C, range 30–70, step 1): tank temperature at which the immersion heater is allowed to assist *during Extra DHW*. Default `60 °C` (factory) — at that value immersion only engages right at the XVV stop temp, contributing little. Lowering it (e.g. to 50 °C) lets immersion ramp DHW from 50 °C up to the 60 °C XVV stop, meaningfully shortening Bath duration.
- `62001 Stopptemp varmvatten` (R, scaling 0.1, °C): current effective DHW stop temp. Tracks the active program's target.
- `62005 Status` (R, scaling 1): system function code. `5 = DHW` confirms heater is actively charging DHW.
- `62276 Actual temperature DHW` (R, scaling 0.1, °C): upper-tank temp sensor.

### 2.2 Existing actor & integrations reused

- `CtcActor` (`server/src/modbus/actor.rs`) — sequential Modbus access via mpsc + oneshot.
- `HomeyClient::set_pump_onoff(bool)` (`server/src/homey/mod.rs`) — REST call to the heating-circ smart plug. **After this feature ships, this function is called only from the Homey reconciler poller** (§4.7). All upstream code (`SmartGridActor`, `DhwActor`) sets intent into a `watch` channel; the reconciler is the single writer to Homey.
- `PriceState` + 5-band `PriceLevel` (`server/src/energy/price.rs`) — already populated even when Tibber is off.
- Storage poller (`server/src/storage/poller.rs`) — already polls `CTC_ROOM_TEMP`, `CTC_ACTUAL_TEMP_DHW`, and `CTC_SYSTEM_STATUS (62005)`. No additions needed; the dashboard chip in §4.6 just consumes the already-cached value.
- Atomic-JSON persistence pattern from `heatpump_stats` (`.tmp` + rename) — reuse for DHW state.
- `AbortHandle` task-cancellation pattern from SmartGrid auto-resume scheduler — reuse for boost watchers.

### 2.3 Live state on rig (2026-05-14)

```
61500 = 3       (Manuell — will be switched to 1=Normal by user before deploy)
61501 = 55.0 °C
61503 = 0       (no boost active)
61591 = 0.0 kW  (immersion disabled)
61636 = 60.0 °C (factory default — immersion only joins right at XVV stop)
62001 = 55.0 °C (effective stop temp)
62005 = ?       (already polled as Sensor::SystemStatus; current value not snapshotted here)
62276 = 56.0 °C (current upper-tank temp)
```

---

## 3. Use Cases

### 3.1 UC-A — Shower (quick boost)

**Trigger**: user picks `⚡ Shower (30 min)` in the DHW dropdown.

**Side actions on activation** (in this order):

1. Read `62001 Stopptemp varmvatten` (current effective DHW stop temp).
2. Read `62276 Actual temperature DHW` (upper-tank sample — use the latest cached value from the storage poller; do not issue a fresh Modbus read).
3. **Pre-flight short-circuit**: if `62276 ≥ 62001`, return immediately `{ started: false, reason: "already_at_target", dhw_c: <62276>, target_c: <62001> }`. **No Modbus writes, no Homey call, no persistence, no watcher.** Saves both money (no unnecessary HP run) and flash (no `61503` write).
4. `boost_override_tx.send(Some(false))` — instruct the Homey reconciler (§4.7) to drive the heating-circ pump OFF for the duration of the boost. The reconciler converges within one poll tick. No direct `HomeyClient` call; the reconciler is the only writer to Homey.
5. Write `61503 = 1` (raw, 0.5 h).
6. Persist `DhwBoostState { preset: Shower, started_at, duration: 30 min, prior_immersion_engage_temp_c: None, immersion_engaged: false }` to `dhw_state.json`. *(No `prior_pump_on` field — the reconciler reverts to SG-derived intent on override clear, which already encodes the user-visible "what should the pump do right now" state.)*
7. Spawn watcher task (`AbortHandle` stored on actor).

The pre-flight check protects against unnecessary boosts. Once a boost is in flight, the heater's own Extra-DHW timer owns mid-boost behaviour — the watcher does not re-check tank temperature.

**Watcher** — single-purpose, fire-and-forget:

- One timer: 30 min from `started_at`.
- On expiry: re-enable the Homey pump, clear state. **That's the entire watcher.**
- No DHW-temp polling. No price gate. No room-temp gate. No dropdown-change observation.

**Stop sequence (flash-frugal, single path)**:

Only one way for Shower to end: timer expiry after 30 min. There is no UI-exposed cancel for an active Shower; the dropdown disables both UC-A and UC-B rows while a boost is active (§4.6).

1. Watcher fires at `started_at + 30 min`.
2. `boost_override_tx.send(None)` — reconciler reverts pump to SG-derived intent (`pump_on_for(current_sg_mode)`).
3. Clear `DhwBoostState`.
4. Emit dashboard event "Shower complete".

**No `61503 = 0` write.** Heater's own timer counted down to 0 at the same instant; writing again is redundant and costs flash.

The DELETE endpoint (§4.3) is **disabled while a Shower is active** — it returns `409 Conflict`. (For Bath, DELETE remains usable; see §3.2.)

### 3.2 UC-B — Bath (long boost with immersion controller)

**Trigger**: user picks `⚡ Bath (Nh)`, confirms in modal showing:
- Hours slider (0.5–2.0, step 0.5) — Bath max capped at 2 h (longer durations are not useful: tank reaches XVV stop temp inside that window even with cold start).
- Immersion threshold preview (read-only display of `immersion_allow_price_sek_per_kwh` from config)

**Pre-flight gate** (before any side action):

- Sample current `PriceLevel` from `PriceState`. If the band is **not in `{VeryCheap, Cheap}`** at the moment of activation: return `409 Conflict` with `{ error: "price_not_cheap", current_level: "<band>" }`. No Modbus writes, no override, no persistence. The user can wait for a cheap slot or change comfort level if they want immediate heat at any price.

**Side actions on activation** (after pre-flight passes):

1. `boost_override_tx.send(Some(false))` — reconciler drives heating-circ pump OFF.
2. Apply `SmartGridMode::Overcapacity` via existing SG actor (also cancels any pending auto-resume **without re-arming on Bath stop** — if the user wants Blocking again afterwards they re-apply it manually, matching the existing "any mode change cancels schedule" rule). The SG actor will update `desired_tx` to `true` (pump_on_for(Overcapacity)) but the boost override masks it; pump stays off.
3. Read current `61636` → save as `prior_immersion_engage_temp_c` for restore.
4. Write `61636 = config.dhw.immersion_engage_temp_c` (default 50 °C — lowers the threshold so immersion can actually contribute during the boost rather than just at the very end).
5. Write `61503 = 2N` (raw).
6. Evaluate immersion gate (see below). If `spot_sek_per_kwh < 0.45` and PriceLevel ∈ {VeryCheap, Cheap}: write `61591 = immersion_kw_when_allowed` (3.0); else leave `61591 = 0`.
7. Persist `DhwBoostState { preset: Bath { hours: N }, started_at, duration: N*3600s, prior_immersion_engage_temp_c, immersion_engaged: bool }`. *(No `prior_pump_on` field — see UC-A note.)*
8. Spawn watcher task.

**Watcher loop** (fixed 60 s tokio interval; **no event channel on `PriceState`** — sub-minute price-band crossings are tolerated, the Bath stop is at most 60 s late):

| Trigger | Action |
|---|---|
| Timer expiry (Nh from `started_at`) | Stop boost. |
| `CTC_ROOM_TEMP < boost_room_temp_bail_c` (default 17.0 °C) | Stop boost. |
| `PriceLevel ∉ {VeryCheap, Cheap}` | Stop boost. |
| Manual dropdown change | Stop boost. |
| Immersion gate re-evaluation | See below; toggles `61591`, does *not* stop boost. |

**Immersion gate logic (hysteresis-guarded)**:

```
allow_thr = config.dhw.immersion_allow_price_sek_per_kwh   (default 0.50)
hyst      = config.dhw.immersion_hysteresis_sek_per_kwh    (default 0.05)
on_thr    = allow_thr - hyst                                (0.45)
off_thr   = allow_thr + hyst                                (0.55)

if immersion_engaged is false and spot < on_thr and PriceLevel ∈ {VeryCheap, Cheap}:
    write 61591 = immersion_kw_when_allowed   (3.0)
    immersion_engaged = true
elif immersion_engaged is true and spot > off_thr:
    write 61591 = 0
    immersion_engaged = false
# Inside dead-zone or no change needed: write nothing.
```

Bath max 2 h × hourly spot slots → expected ≤ 1 immersion toggle per boost. Combined with the idempotent guard, flash-wear pressure on `61591` stays minimal.

**Stop sequence (flash-frugal on `61503`)**:

| Step | Action | Skipped when… |
|---|---|---|
| 1 | Cancel watcher (`AbortHandle::abort()`). | Never. |
| 2 | Write `61503 = 0`. | **Stop cause = timer expiry** — the heater's own `61503` counter is already at 0 at the same instant; redundant write. Performed for *every* early stop (room bail, price-level leaves Cheap, manual cancel via DELETE). |
| 3 | Write `61591 = 0`. | `immersion_engaged == false` — we never raised it. |
| 4 | Write `61636 = prior_immersion_engage_temp_c`. | Never — heater never restores this on its own; we always undo. |
| 5 | Apply `SmartGridMode::Normal` via SG actor. | Never — we set Overcapacity, we revert. |
| 6 | `boost_override_tx.send(None)` (reconciler reverts pump to SG-derived intent). | Never. |
| 7 | Clear `DhwBoostState` from persistence. | Never. |
| 8 | Emit dashboard event. | Never. |

Only step 2 benefits from the flash-frugal trick. Steps 4–6 cover state that the heater (or our SG actor / Homey) does not reset autonomously, so they always run regardless of stop cause.

### 3.3 UC-C — Set DHW Comfort Level

**Trigger**: user opens `🌡 Comfort level ▸` inline submenu, picks Economy / Normal / Komfort.

**Side action**: write `61500 = 0 | 1 | 2` (raw). No boost interaction.

**Manuell handling**: at app startup, if `61500 = 3` is read, the comfort sub-menu opens with no row selected. The first comfort pick the user makes writes `61500 = 0/1/2`, which moves the heater off Manuell permanently (per user decision — Manuell can be re-applied at the physical pump if ever needed).

---

## 4. Architecture

### 4.1 Module layout

New module under `server/src/dhw/`:

```
server/src/dhw/
├── mod.rs           # public re-exports
├── controller.rs    # DhwController: owns DhwState, drives all transitions
├── state.rs         # DhwState, DhwBoostState, BoostPreset, persistence
└── watcher.rs       # tokio task body: timer + stop-trigger evaluation
```

New routes module:

```
server/src/routes/dhw.rs
```

### 4.2 Data model

```rust
// server/src/dhw/state.rs

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BoostPreset {
    Shower,
    Bath { hours: f32 },
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DhwBoostState {
    pub preset: BoostPreset,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub duration_secs: u64,
    pub prior_immersion_engage_temp_c: Option<f32>,   // None for Shower (never touched 61636)
    pub immersion_engaged: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DhwPersistedState {
    pub schema_version: u32,        // start at 1
    pub boost: Option<DhwBoostState>,
}
```

### 4.2.1 Actor pattern (consistent with `CtcActor` / SmartGrid actor)

`DhwController` is itself an actor — long-lived task receiving operations via mpsc. Matches the rest of the codebase; cleaner than `Arc<Mutex<...>>` shared state for the same reasons `CtcActor` is an actor (sequential transitions, owned watcher `AbortHandle`, easy persistence on each transition).

```rust
// server/src/dhw/controller.rs

pub enum DhwOp {
    StartShower { resp: oneshot::Sender<Result<StartReport, DhwError>> },
    StartBath  { hours: f32, resp: oneshot::Sender<Result<StartReport, DhwError>> },
    Cancel     { reason: CancelReason, resp: oneshot::Sender<Result<(), DhwError>> },
    SetComfort { level: ComfortLevel, resp: oneshot::Sender<Result<(), DhwError>> },
    Snapshot   { resp: oneshot::Sender<DhwSnapshot> },

    // Internal — emitted by the watcher loop or price-state watcher.
    WatcherTick,
    PriceEvent,
}

pub type DhwSender = mpsc::Sender<DhwOp>;

pub struct DhwActor {
    rx: mpsc::Receiver<DhwOp>,
    state: Option<DhwBoostState>,
    watcher_abort: Option<AbortHandle>,
    modbus_tx: ModbusSender,
    boost_override_tx: watch::Sender<Option<bool>>,   // §4.7 — sets/clears boost pump override; no direct HomeyClient handle on DhwActor
    smartgrid: SmartGridHandle,        // existing public handle in server/src/smartgrid/mod.rs
    store: Store,                       // already cheap-clone; no Arc wrapper at this boundary
    price_state: Arc<PriceState>,
    cfg: DhwConfig,
    persist_path: Option<PathBuf>,
}

impl DhwActor {
    pub async fn run(mut self) { /* receive loop dispatching DhwOp variants */ }
}
```

HTTP handlers send `DhwOp` over the channel and await the oneshot response — same shape as the existing CTC routes.

### 4.3 HTTP API

| Method | Path | Body / Query | Response |
|---|---|---|---|
| `GET` | `/api/v1/dhw/state` | – | `{ comfort_level: "normal"\|…\|"manuell", boost: null \| { preset, started_at, elapsed_s, remaining_s, …, immersion_engaged } }` |
| `POST` | `/api/v1/dhw/comfort` | `?level=economy\|normal\|komfort` | `204 No Content` |
| `POST` | `/api/v1/dhw/boost` | `?preset=shower` | `{ started: true, scheduled_end: "…" }` on activation; `{ started: false, reason: "already_at_target", dhw_c, target_c }` if pre-flight (§3.1 step 3) short-circuits; `409 Conflict` if any boost already active. |
| `POST` | `/api/v1/dhw/boost` | `?preset=bath&hours=N` (N ∈ [0.5, 2.0]) | `{ started: true, scheduled_end: "…" }`. `422 Unprocessable Entity` with `{ error: "out_of_range", field: "hours", min: 0.5, max: 2.0 }` if `hours` is outside the slider range. `409 Conflict` with `{ error: "price_not_cheap", current_level: "<band>" }` if the current PriceLevel is not in {VeryCheap, Cheap}. `409 Conflict` with `{ error: "boost_already_active" }` if a Shower or Bath is already in flight. |
| `DELETE` | `/api/v1/dhw/boost` | – | `204 No Content` for an active Bath. `409 Conflict` for an active Shower (Shower has no cancel path). `204` with no-op if nothing active. |

### 4.4 Config (new `[dhw]` table)

```toml
[dhw]
shower_duration_minutes              = 30      # UC-A timer length (heater-side; watcher matches for pump restore)
bath_max_hours                       = 2.0     # UC-B slider upper bound (range [0.5, 2.0], step 0.5)
boost_room_temp_bail_c               = 17.0    # UC-B safety bail
immersion_allow_price_sek_per_kwh    = 0.50    # UC-B central immersion gate
immersion_hysteresis_sek_per_kwh     = 0.05    # UC-B dead-zone
immersion_kw_when_allowed            = 3.0     # UC-B 61591 write value while gated on
immersion_engage_temp_c              = 50.0    # UC-B 61636 write value while boost active (default lets immersion ramp 50→60 °C)

# Optional override via env: CTC_DHW_PERSIST_PATH
persist_path                         = "/app/data/dhw_state.json"
```

All knobs are read once at startup. Hot-reload is out of scope.

### 4.5 Persistence

- File: `${persist_path}` (default off; bind-mount `/app/data/dhw_state.json` in Docker).
- Atomic write: serialise to `<path>.tmp` then `rename`.
- Schema version 1. Unknown versions log a warning and start fresh.
- Saved on:
  - Boost start.
  - Immersion engaged/disengaged.
  - Boost end.
  - Graceful shutdown (via `axum::serve(...).with_graceful_shutdown` — already wired for heatpump_stats).
- **Crash recovery on startup** (long-lived `DhwActor` resets state if file shows boost mid-flight): if file exists with `boost: Some(_)`:
  1. Write `61503 = 0` (cancel any heater-side boost).
  2. Write `61591 = 0` (force immersion off).
  3. If `prior_immersion_engage_temp_c.is_some()`: write `61636 = prior_immersion_engage_temp_c` (restore Bath-modified XVV threshold).
  4. Apply `SmartGridMode::Normal` via SG actor (Bath may have left SG at Overcapacity). This also updates `desired_tx` for the reconciler.
  5. `boost_override_tx.send(None)` — drop any stale boost override. The reconciler converges the pump within one poll tick using `pump_on_for(Normal)`.
  6. Clear file.
  7. Log a warning describing what was cleaned up.

The `DhwActor` performs this exact sequence as the first step of `run()`, before its mpsc receive loop accepts any operations.

### 4.6 Dashboard wiring (React, in `server/static/app.jsx`)

New `<DhwControl />` component:

```
┌─ DHW ─────────────────────┐
│ Current:  Normal · 55 °C  │
│ ▾ select                  │
└───────────────────────────┘
       ▼ on open
┌──────────────────────────┐    ┌───────────────────────┐
│ ⚡ Shower    (30 min)    │    │ Comfort level         │
│ ⚡ Bath      (custom h)  │    │   Economy   50 °C     │
│ 🌡 Comfort level     ▸ ──┼──▶ │ ● Normal    55 °C     │
└──────────────────────────┘    │   Komfort   58 °C     │
                                └───────────────────────┘
```

While a boost is active, the dropdown trigger shows: `⚡ Shower · 18 min left` or `⚡ Bath · 1 h 14 min left · immersion ON`. **Both `⚡ Shower` and `⚡ Bath` rows are disabled** in the open dropdown until the active boost ends — only the Comfort-level submenu remains interactive.

- For Shower: there is no Cancel row (Shower runs to completion).
- For Bath: a `Cancel boost` row is present (calls `DELETE /api/v1/dhw/boost`).

If `62005 = 5` (DHW), a small chip "charging DHW" appears next to the trigger.

#### 4.6.1 Header badge (boost active)

A new badge slot is added to the header bar (`index.html`), styled like the existing SmartGrid and Powersave badges. Visible only while a boost is active:

- Shower active: `⚡ DHW Boost · 18 min` (no immersion mention — Shower doesn't touch immersion).
- Bath active, immersion off: `⚡ DHW Boost · 1 h 14 min`.
- Bath active, immersion on: `⚡ DHW Boost · 1 h 14 min · ⚙ immersion`.

The badge is read-only (clicking does nothing). Source: `GET /api/v1/dhw/state` already polled every 5 s by the dashboard refresh loop.

#### 4.6.2 Spot-price chart — boost-window band

Same mechanism as the existing SmartGrid Blocking → auto-resume translucent band (see CLAUDE.md → SmartGrid Auto-Resume). Reuse the rendering primitive: while a boost is active, the spot-price chart draws a **translucent vertical band** spanning `[started_at, boost_end]` where `boost_end = started_at + duration` (30 min for Shower, `hours * 3600 s` for Bath).

- The band's right edge is the "back to normal" instant — visually identical idiom to the Blocking band, so users already familiar with that cue read it the same way.
- Visibility is data-driven (anchored to slot timestamps, not wall-clock): the band appears as long as any part of `[started_at, boost_end]` falls inside the chart's slot range, and clips at the chart edges if the boost crosses the visible window.
- Distinct colour from the SmartGrid Blocking band so the two can co-exist on the chart without confusion. Pick from the existing palette (`oklch(...)` tokens in `styles.css`) — e.g. the warm/hot family used elsewhere for DHW, with low alpha.
- If a SmartGrid Blocking band and a DHW boost band overlap on the chart, both render — alpha compositing handles the overlap; no z-order rules needed.

Implementation note: the existing SmartGrid band code in `app.jsx` reads `scheduled_resume_at` from the SG state endpoint and a window length. The DHW band reads `boost.started_at` and `boost.scheduled_end` from `/api/v1/dhw/state`. Same plumbing pattern, separate band component.

### 4.7 Homey pump intent — boost-override lane

Today, `SmartGridActor` owns a `watch::Sender<bool> desired_tx` and the Homey reconciler poller (`server/src/homey/poller.rs:62-81`) writes `desired_tx.borrow()` to the smart plug whenever the actual state diverges. If `DhwActor` were to call `HomeyClient::set_pump_onoff(false)` directly, the next reconciler tick (~30 s) would read `desired_tx = true` (because SG=Overcapacity implies pump on) and flip the pump back ON mid-boost.

**Fix**: extend `HomeyHooks` with a second `watch` channel for boost-override intent. The reconciler now reads both lanes with strict priority:

```rust
struct HomeyHooks {
    sg_desired_rx:     watch::Receiver<bool>,            // existing
    boost_override_rx: watch::Receiver<Option<bool>>,    // NEW
}

// inside the reconciler tick:
let target = match *boost_override_rx.borrow() {
    Some(v) => v,                       // boost wins
    None    => *sg_desired_rx.borrow(), // fall back to SG intent
};
let actual = homey.get_pump_onoff().await?;
if actual != target { homey.set_pump_onoff(target).await?; }
```

`DhwActor` owns the corresponding `boost_override_tx: watch::Sender<Option<bool>>`:

- **Bath start** — `boost_override_tx.send(Some(false))`. From this moment, no matter what `desired_tx` says (SG=Overcapacity normally implies pump=on), the reconciler keeps the pump OFF.
- **Bath stop (any cause)** — `boost_override_tx.send(None)`. Reconciler reverts to SG-derived intent within one poll tick. The pump returns to `pump_on_for(current_sg_mode)`.

`SmartGridActor` is unchanged from the outside; it still publishes intent to `desired_tx` whenever a mode changes. It no longer races DHW for direct Homey writes because **only the reconciler calls `set_pump_onoff`**. Any direct `set_pump_onoff` call sites elsewhere (currently only in `push_pump_to_homey`) are migrated to set `desired_tx` instead.

This also resolves the crash-recovery race in §4.5: the recovery sequence sets `boost_override_tx.send(None)` and `SmartGridMode::Normal`, then the reconciler converges the pump to whatever `pump_on_for(Normal)` returns — no need to call `set_pump_onoff(true)` directly. §4.5 step 5 simplifies accordingly:

> 5. Set `boost_override_tx = None` (drops any stale boost override). The reconciler converges the pump within one poll tick using `pump_on_for(Normal)`.

### 4.8 Modbus operations added

| Reg | Op | Trigger |
|---|---|---|
| `61500` | Write | Comfort level change. |
| `61503` | Write | Boost start (Shower writes `1`, Bath writes `2N`). On stop: written `=0` only for **early** stops (Bath room/price/manual cancel). Natural timer expiry (Shower or Bath) writes nothing — heater's counter is already at 0. |
| `61591` | Write | Bath immersion gate transitions. |
| `61636` | Read + Write | Bath start (snapshot prior → lower to engage temp) and Bath stop (restore prior). Untouched by Shower. |
| `62001` | Read | Shower activation pre-flight — compared against the cached `62276` to decide if a boost is even needed. |
| `62005` | Read | Already polled as `Sensor::SystemStatus` (`storage/poller.rs:55`). This feature only adds a dashboard chip that reads the cached value. |
| `62276` | Read | Already polled; surfaced in dashboard. Not consumed by Shower or Bath watchers. |

All writes go through the existing `CtcActor` write path (validation against device min/max/step, read-back verification).

---

## 5. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Process crashes mid-boost → `61503`, `61591`, `61636`, SG, Homey pump stay in non-default state. | Persistence + on-startup recovery in `DhwActor::run` (§4.5). Conservative: pump restored to ON, SG to Normal, `61636` to prior snapshot. |
| Repeated `61591` writes wear the heater's flash. | Idempotent write guard + ±0.05 SEK hysteresis → ≤ 1 toggle per 2 h Bath. |
| Repeated `61636` writes wear the heater's flash. | Exactly 2 writes per Bath (one to lower on start, one to restore on stop). Not touched by Shower. |
| User picks a comfort level while a boost is active. | Allowed — `61500` and `61503` are independent. The boost completes against the heater's Extra-DHW logic regardless of program. |
| Heater enters Manuell again at the pump while server thinks it's Normal. | On every comfort-level write, read back `61500` and reconcile dashboard state. |
| Homey API unreachable. | The reconciler retries on its own tick cadence (existing behaviour). DhwActor never blocks on Homey — it only writes to `boost_override_tx`. If Homey is unreachable for the entire boost, the heating-circ pump may run while a boost is in flight; not a safety issue (heater handles its own thermal protection). Logged as a warning by the reconciler. |
| Tank already at target when user fires Shower. | Detected at activation step 2 (UC-A); return 200 with `started: false`. No side effects. |
| Long Bath outlasts the configured immersion-allow window. | Immersion gate re-evaluates on price-state events; immersion turns off cleanly when price rises through the upper hysteresis band. |

---

## 6. Testing

### 6.1 Unit

- `BoostPreset` serialisation round-trip.
- Raw-value math: `0.5 h → raw 1` (Shower), `2.0 h → raw 4` (Bath max).
- Immersion hysteresis state machine: ascending/descending price sweeps, no toggling inside dead-zone.
- Shower fire-and-forget after activation: pre-flight reads `62001` and cached `62276`; if tank already at target, returns `already_at_target` with zero side effects. Otherwise the watcher only fires the pump-restore at `started_at + 30 min`; no `61503` write on completion.
- Bath `61636` save/restore: prior value snapshot at start, exact restore at stop, regardless of stop cause.
- Bath `61503` stop-write matrix: `61503=0` is written for early stops (room bail, price-band bail, manual DELETE) and **skipped** on natural timer expiry. Property: `61503` write count on stop is at most 1 across the entire Bath lifecycle.
- DELETE endpoint state machine: `409` while Shower active; `204` while Bath active or nothing active.
- Activation conflict: any `POST /dhw/boost` while a boost is already in flight returns `409`.
- Persistence round-trip (`DhwPersistedState` → JSON → struct).
- Recovery path: file with `boost: Some(...)` triggers the documented sequence (61503=0, 61591=0, optionally 61636 restore, SG=Normal, pump=on).

### 6.2 Integration (mocked Modbus)

- Full UC-A lifecycle: activate → tick → tank reaches threshold → stop sequence.
- Full UC-B lifecycle: activate → mid-boost immersion ON → price rise → immersion OFF → PriceLevel leaves Cheap → boost stop.
- Manual cancel mid-boost (DELETE endpoint).
- Comfort change during active boost — no interference.

### 6.3 Property

- For any sequence of price ticks, `61591` write count ≤ price-band-crossings + 2.
- After any stop reason, `61503=0`, `61591=0`, pump restored — invariant.

### 6.4 Manual (against live ctc.lan)

- Pre-deploy: switch rig from Manuell to Normal at the pump (one-time).
- Shower from a 50 °C start: should stop at 55 °C ≤ 30 min, no immersion writes, pump comes back on.
- Bath, 2 h, with daytime cheap window then evening peak: confirm immersion-on then immersion-off then boost-cancel.
- Kill the server during a Bath → restart → confirm cleanup writes happened.

---

## 7. Build sequence (for the plan)

0. **Pre-implementation verification on ctc.lan**:
   - Confirm `POST /api/v1/ctc?addr=X&value=Y` semantics by reading `post_ctc_data` in `server/src/routes/ctc.rs` and round-tripping a no-op write on a safe register (e.g. write the value already read back from `61500`). Document whether `value` is raw or scaled. **No further coding until this is settled** — the entire DHW write path depends on it.
1. `dhw::state` + persistence (incl. tests).
2. `dhw::actor` (long-lived, owns state, runs crash-recovery on `run()` entry) + comfort op + matching HTTP route.
3. Watcher for UC-A; UC-A endpoints + integration tests; verify "no `61503` write on natural stop" property.
4. UC-B activation path (with `61636` save/restore, without immersion gating) + integration tests.
5. Immersion gate + hysteresis + write-frugality tests.
6. Persistence wiring + verify crash-recovery sequence on startup.
7. Dashboard wiring for the existing `62005 (Sensor::SystemStatus)` cached value — "charging DHW" chip (no poller change needed; the sensor is already in the polled set).
8. Dashboard `<DhwControl />` component, wired to new endpoints.
9. User-side pre-deploy: switch heater `61500` from 3 (Manuell) to 1 (Normal) at the physical pump.
10. Deploy.

Each step finishes with `cargo fmt`, `cargo clippy --all-targets -- -W clippy::pedantic`, `cargo test --all-targets` clean before the next begins.

### 7.1 Resolved design decisions (carried into plan)

- **Actor pattern** for `DhwController` (now `DhwActor`) — consistent with `CtcActor` and SmartGrid actor; long-lived, mpsc-driven.
- **Watcher**: per-boost `tokio::spawn` with `AbortHandle` stored on the `DhwActor`. The actor itself is the long-lived task; watcher tasks are short-lived children. Crash-recovery happens once in `DhwActor::run` prologue.
- **Shower has no cancel path** (per §3.1 / §4.3): the dropdown disables both boost rows while a Shower is in flight, and `DELETE /api/v1/dhw/boost` returns `409 Conflict` for an active Shower. Shower runs to completion via the heater's own timer; our watcher only fires the pump-restore at `started_at + 30 min`. Zero `61503` writes on Shower stop.
- **Bath max 2 h** (slider `[0.5, 2.0]` step `0.5`).
- **`61636` is in scope** for Bath (was deferred — re-promoted because without lowering it the immersion power gate `61591` is functionally inert during XVV).
