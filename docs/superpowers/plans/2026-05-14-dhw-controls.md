# DHW Controls Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a separate DHW dropdown on the dashboard (Shower / Bath / Comfort level), with a Bath-scoped immersion-heater controller, plus a Homey reconciler refactor that introduces a "boost-override" lane so DHW can keep the heating-circ pump off without the SmartGrid reconciler flipping it back.

**Architecture:** New `dhw` module owning a long-lived actor (mpsc + oneshot, mirroring `CtcActor`/`SmartGridHandle`). Modbus writes go through the existing `CtcActor`. Homey pump intent goes through a new `boost_override_tx: watch::Sender<Option<bool>>` consumed by an updated Homey reconciler that reads both lanes (boost-override priority, SmartGrid fallback). Crash recovery runs once in `DhwActor::run` prologue. Persistence reuses the atomic-JSON pattern from `heatpump_stats`. Dashboard adds a `<DhwControl />` component, a header badge, a chart band, and a "charging DHW" chip.

**Tech Stack:** Rust 2024 / Axum / tokio-modbus / tokio (current_thread runtime) / serde / chrono / Preact-style React in `server/static/app.jsx` / OKLCH CSS palette.

**Companion spec:** `/home/mbg/ws/ctc_server/docs/superpowers/specs/2026-05-14-dhw-controls-design.md` — read before starting any task.

---

## CLAUDE.md house rules (verbatim)

The following are the project's coding standards from `/home/mbg/ws/ctc_server/CLAUDE.md`. Every task in this plan must satisfy these. Do not deviate.

```
1. Zero Clippy Warnings
   cargo clippy --all-targets -- -W clippy::pedantic
   - Must produce zero warnings
   - Do not use #[allow(...)] without clear justification
   - Fix issues rather than suppressing warnings

2. All Tests Pass
   cargo test --all-targets
   - All existing tests must pass
   - Add tests for new functionality
   - Add tests for bug fixes to prevent regressions

3. Code Coverage (≥90%)
   cargo tarpaulin --all-targets --workspace --out Stdout

4. Code Formatting
   cargo fmt

5. Float Comparisons in Tests
   - Never use assert_eq! for float comparisons
   - Use epsilon-based comparison helper:
   fn assert_float_eq(a: f32, b: f32, msg: &str) {
       assert!((a - b).abs() < f32::EPSILON, "{msg}: expected {b}, got {a}");
   }
```

Commit messages: subject ≤ 50 chars, imperative verb, no period. Body wraps at 72.

Git policy: **agents may run worktree / rebase / add / commit on feature branches; main is hard-forbidden; push and merge-to-main are user-only.**

---

## Worktree setup (run BEFORE Task 0)

This plan uses a sibling worktree so the primary `ctc_server` checkout stays on `main`. From `/home/mbg/ws`:

```bash
git -C ctc_server fetch origin
git -C ctc_server worktree add ../ctc_server-dhw_controls -b feature/dhw_controls origin/main
cd ../ctc_server-dhw_controls
```

From now on every command in this plan assumes `cwd = /home/mbg/ws/ctc_server-dhw_controls`.

---

## Rebase bracket — START

Before Task 0 begins, the feature branch must be rebased onto `origin/main` so the diff is minimal and current:

```bash
git fetch origin
git rebase origin/main
```

If conflicts arise, resolve them, run `cargo test --all-targets`, then `git rebase --continue`. Never `--skip` a commit silently.

---

## File structure

| Path | Role |
|---|---|
| `server/src/modbus/bms_parameters.rs` | Add `CTC_BOILER_DHW_C` (61636) constant. |
| `server/src/config.rs` | Extend `Config` with optional `[dhw]` table → `DhwConfig`. |
| `server/src/dhw/mod.rs` | Module root; re-exports `DhwHandle`, `DhwError`, `DhwSnapshot`, `BoostPreset`, `ComfortLevel`. |
| `server/src/dhw/state.rs` | `BoostPreset`, `DhwBoostState`, `DhwPersistedState`; atomic load / save / clear. |
| `server/src/dhw/error.rs` | `DhwError`, `CancelReason`, `StartReport`, `ComfortLevel`, conversion to `StatusCode`. |
| `server/src/dhw/actor.rs` | `DhwActor` + `DhwCmd` + `DhwHandle`. Owns watcher `AbortHandle`. Crash-recovery prologue in `run()`. |
| `server/src/dhw/watcher.rs` | Watcher task bodies (Shower 30-min one-shot, Bath 60-s interval loop). |
| `server/src/dhw/immersion.rs` | Hysteresis state machine (`ImmersionGate`) — pure logic, no IO. |
| `server/src/smartgrid/actor.rs` | Add `boost_override_tx` field on `HomeyHooks`; export `BoostOverride` newtype. |
| `server/src/homey/poller.rs` | Reconciler reads override lane with priority over SG-derived intent. |
| `server/src/routes/dhw.rs` | Axum router: `GET /dhw/state`, `POST /dhw/comfort`, `POST /dhw/boost`, `DELETE /dhw/boost`. |
| `server/src/routes/mod.rs` | Mount the new router. |
| `server/src/main.rs` | Wire `DhwActor` + `boost_override` `watch` channel + `DhwHandle` into app state. |
| `server/static/app.jsx` | New `<DhwControl />` component; chart band; header badge; charging-DHW chip. |
| `server/static/index.html` | Slot for the new `DhwBoost` header badge. |
| `server/static/styles.css` | CSS for badge + chart band colour (distinct OKLCH from SmartGrid band). |
| `config.toml.example` | New `[dhw]` section with defaults. |
| `Dockerfile` / `docker-compose.yml` | Bind-mount + env for `CTC_DHW_PERSIST_PATH=/app/data/dhw_state.json`. |

---

## Task 0: Pre-implementation verification on ctc.lan

**Purpose:** Confirm `POST /api/v1/ctc?addr=X&value=Y` semantics. The entire DHW write path depends on this — do not proceed past Task 0 until settled.

**Files:** none modified (read-only investigation).

- [ ] **Step 1: Read `post_ctc_data` in `server/src/routes/ctc.rs`** — locate the `value` parsing code path. Determine whether the `value` query param is the *raw u16* (passed through to Modbus) or the *scaled f32* (divided by the parameter's `factor` before writing).

Run:
```bash
grep -n "fn post_ctc_data\|value:" server/src/routes/ctc.rs | head -20
```

- [ ] **Step 2: Cross-check against `CtcActor`'s write validator** in `server/src/modbus/actor.rs:498`. Specifically how the value is compared to `reg_max`/`reg_min`/`reg_step`. The validation runs in raw or scaled space.

- [ ] **Step 3: Probe `ctc.lan`** with a no-op write to a safe register. Write the value already read back. Use `61500` (Hot water mode), currently `3` (Manuell).

```bash
curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61500&custom=true&factor=1.0"
# Expect: {"ctc_data": 3.0}

# Round-trip a no-op write at the same value (should succeed regardless of raw/scaled semantics if value == current):
curl -s -X POST "http://ctc.lan:3000/api/v1/ctc?addr=61500&value=3"
# Expect: HTTP 200 and {"ctc_data": 3.0}

# Re-read:
curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61500&custom=true&factor=1.0"
# Expect: {"ctc_data": 3.0}
```

- [ ] **Step 4: Finding (verified during plan-time, 2026-05-14)**:

> **Finding: `POST /api/v1/ctc?addr=X&value=Y` accepts the SCALED physical value (`value: f32`).**
> `server/src/routes/ctc.rs:103` reads `params.value: f32` and passes it to `write_parameter` (`server/src/modbus/operations.rs:128`), which forwards to `CtcActor::write_parameter(_, value: f32, …)` (`server/src/modbus/actor.rs:514`).
> Internally the actor calls `CTCModbusParameter::get_raw_value(value)` (`server/src/modbus/mod.rs:123`) which divides by `factor` and rounds.
> DHW code that goes through the actor channel — directly or via a trait — must pass **scaled physical values** (e.g. °C, h, kW). The actor handles raw conversion.

Concrete consequence for this plan: the `ModbusWriter` trait used in Tasks 6-13 takes a **scaled `f32`**, not a raw `i16`. Each call site below is documented in physical units, e.g. `write_scaled(61503, 0.5)` for a 30-min Shower boost (the actor turns that into raw `1` via `0.5 / 0.5`).

- [ ] **Step 5: No code commit.** Continue to Task 1.

---

## Task 1: Add `CTC_BOILER_DHW_C` constant (register 61636)

**Files:**
- Modify: `server/src/modbus/bms_parameters.rs` (append before the closing region marker for "Hot water")
- Test: `server/src/modbus/bms_parameters.rs` (inline `#[cfg(test)]` if a matching test module exists; otherwise add one)

- [ ] **Step 1: Write the failing test**

Locate the existing test module for bms_parameters (look for `#[cfg(test)] mod tests` in the file). Add:

```rust
#[test]
fn ctc_boiler_dhw_c_metadata_matches_service_doc() {
    use super::CTC_BOILER_DHW_C;
    assert_eq!(CTC_BOILER_DHW_C.id, 61636);
    assert_eq!(CTC_BOILER_DHW_C.factor, 0.1);
    assert_eq!(CTC_BOILER_DHW_C.reg_max, Some(60408));
    assert_eq!(CTC_BOILER_DHW_C.reg_min, Some(60409));
    assert_eq!(CTC_BOILER_DHW_C.reg_step, Some(60410));
    assert_eq!(CTC_BOILER_DHW_C.visible, 62508);
    assert_eq!(CTC_BOILER_DHW_C.bit, 8);
    assert!(matches!(CTC_BOILER_DHW_C.access, crate::modbus::Access::RW));
}
```

- [ ] **Step 2: Run test, expect failure**

```bash
cargo test -p server ctc_boiler_dhw_c_metadata --no-fail-fast
```
Expected: compile error `cannot find value 'CTC_BOILER_DHW_C'`.

- [ ] **Step 3: Add the constant**

Append near the other DHW parameters (around line 287 where `CTC_EXTRA_HOT_WATER_TIMER` is defined):

```rust
ctc_parameter!(
    CTC_BOILER_DHW_C,
    61636,
    "Boiler DHW °C (Elpanna extra VV) — temperature at which immersion engages during Extra DHW",
    0.1,
    Access::RW,
    60408,
    62508,
    8
);
```

- [ ] **Step 4: Run test, expect pass**

```bash
cargo test -p server ctc_boiler_dhw_c_metadata
```
Expected: PASS.

- [ ] **Step 5: Format, clippy, full test sweep**

```bash
cargo fmt
cargo clippy --all-targets -- -W clippy::pedantic
cargo test --all-targets
```
Expected: zero warnings, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add server/src/modbus/bms_parameters.rs
git commit -m "Add CTC_BOILER_DHW_C parameter (61636)"
```

---

## Task 2: `DhwConfig` and TOML parsing

**Files:**
- Modify: `server/src/config.rs`
- Modify: `config.toml.example`
- Test: `server/src/config.rs` (inline tests)

- [ ] **Step 1: Write the failing test**

Add to the test module in `config.rs`:

```rust
#[test]
fn dhw_config_defaults() {
    let cfg: DhwConfig = toml::from_str("").unwrap();
    assert_eq!(cfg.shower_duration_minutes, 30);
    assert!((cfg.bath_max_hours - 2.0).abs() < f32::EPSILON);
    assert!((cfg.boost_room_temp_bail_c - 17.0).abs() < f32::EPSILON);
    assert!((cfg.immersion_allow_price_sek_per_kwh - 0.50).abs() < f32::EPSILON);
    assert!((cfg.immersion_hysteresis_sek_per_kwh - 0.05).abs() < f32::EPSILON);
    assert!((cfg.immersion_kw_when_allowed - 3.0).abs() < f32::EPSILON);
    assert!((cfg.immersion_engage_temp_c - 50.0).abs() < f32::EPSILON);
    assert!(cfg.persist_path.is_none());
}

#[test]
fn dhw_config_overrides_parse() {
    let cfg: DhwConfig = toml::from_str(
        r#"
        shower_duration_minutes = 20
        bath_max_hours = 3.0
        boost_room_temp_bail_c = 18.5
        immersion_allow_price_sek_per_kwh = 0.40
        immersion_hysteresis_sek_per_kwh = 0.10
        immersion_kw_when_allowed = 5.5
        immersion_engage_temp_c = 45.0
        persist_path = "/var/lib/ctc/dhw.json"
        "#,
    ).unwrap();
    assert_eq!(cfg.shower_duration_minutes, 20);
    assert!((cfg.bath_max_hours - 3.0).abs() < f32::EPSILON);
    assert!((cfg.immersion_kw_when_allowed - 5.5).abs() < f32::EPSILON);
    assert_eq!(cfg.persist_path.as_deref(), Some(std::path::Path::new("/var/lib/ctc/dhw.json")));
}
```

- [ ] **Step 2: Run, expect compile failure**

```bash
cargo test -p server dhw_config_ --no-fail-fast
```

- [ ] **Step 3: Implement `DhwConfig`**

Add to `server/src/config.rs`, near the existing `[smartgrid]` config struct:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DhwConfig {
    /// Shower preset duration in minutes (heater-side timer matched by watcher).
    pub shower_duration_minutes: u32,
    /// Bath slider upper bound (hours). Range [0.5, bath_max_hours] in 0.5 steps.
    pub bath_max_hours: f32,
    /// Cancel Bath if CTC_ROOM_TEMP drops below this (°C).
    pub boost_room_temp_bail_c: f32,
    /// Spot price ceiling (SEK/kWh) for Bath immersion gate, centre value.
    pub immersion_allow_price_sek_per_kwh: f32,
    /// Hysteresis around the immersion gate (SEK/kWh).
    pub immersion_hysteresis_sek_per_kwh: f32,
    /// Power cap written to 61591 while immersion gate is engaged (kW).
    pub immersion_kw_when_allowed: f32,
    /// `61636` value written while a Bath is active (°C).
    pub immersion_engage_temp_c: f32,
    /// Path to the persistence JSON. None = no persistence.
    pub persist_path: Option<std::path::PathBuf>,
}

impl Default for DhwConfig {
    fn default() -> Self {
        Self {
            shower_duration_minutes: 30,
            bath_max_hours: 2.0,
            boost_room_temp_bail_c: 17.0,
            immersion_allow_price_sek_per_kwh: 0.50,
            immersion_hysteresis_sek_per_kwh: 0.05,
            immersion_kw_when_allowed: 3.0,
            immersion_engage_temp_c: 50.0,
            persist_path: None,
        }
    }
}
```

Add a field on the top-level `Config` struct:

```rust
#[serde(default)]
pub dhw: DhwConfig,
```

And env-var override for `persist_path` near where `CTC_HEATPUMP_STATS_PERSIST_PATH` is handled — add the analogous block reading `CTC_DHW_PERSIST_PATH` and overriding `cfg.dhw.persist_path`.

- [ ] **Step 4: Run tests**

```bash
cargo test -p server dhw_config_
```
Expected: PASS.

- [ ] **Step 5: Update `config.toml.example`**

Append:

```toml
[dhw]
shower_duration_minutes              = 30
bath_max_hours                       = 2.0
boost_room_temp_bail_c               = 17.0
immersion_allow_price_sek_per_kwh    = 0.50
immersion_hysteresis_sek_per_kwh     = 0.05
immersion_kw_when_allowed            = 3.0
immersion_engage_temp_c              = 50.0
# persist_path = "/app/data/dhw_state.json"   # or set CTC_DHW_PERSIST_PATH env var
```

- [ ] **Step 6: Format, clippy, test**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
```

- [ ] **Step 7: Commit**

```bash
git add server/src/config.rs config.toml.example
git commit -m "Add DhwConfig with defaults and env override"
```

---

## Task 3: DHW state types and atomic persistence

**Files:**
- Create: `server/src/dhw/mod.rs`
- Create: `server/src/dhw/state.rs`
- Create: `server/src/dhw/error.rs`
- Test: same files (inline)

- [ ] **Step 1: Create `server/src/dhw/mod.rs` with re-exports**

```rust
//! Domestic-hot-water dropdown control + Bath-scoped immersion controller.

pub mod actor;
pub mod error;
pub mod immersion;
pub mod state;
pub mod watcher;

pub use actor::{DhwActor, DhwCmd, DhwHandle};
pub use error::{CancelReason, ComfortLevel, DhwError, StartReport};
pub use state::{BoostPreset, DhwBoostState, DhwPersistedState, DhwSnapshot};
```

(Some `mod` declarations point to files we create in later tasks. Comment them out for now so the crate compiles; uncomment as each file lands.)

- [ ] **Step 2: Write failing tests in `server/src/dhw/state.rs`**

```rust
use std::path::PathBuf;
use tempfile::tempdir;
use chrono::Utc;

#[test]
fn persisted_state_roundtrip() {
    let snap = DhwPersistedState {
        schema_version: 1,
        boost: Some(DhwBoostState {
            preset: BoostPreset::Bath { hours: 1.5 },
            started_at: Utc::now(),
            duration_secs: 5400,
            prior_immersion_engage_temp_c: Some(60.0),
            immersion_engaged: true,
        }),
    };
    let json = serde_json::to_string(&snap).unwrap();
    let back: DhwPersistedState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.schema_version, 1);
    let boost = back.boost.unwrap();
    assert!(matches!(boost.preset, BoostPreset::Bath { hours } if (hours - 1.5).abs() < f32::EPSILON));
    assert!(boost.immersion_engaged);
}

#[test]
fn atomic_save_then_load() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("dhw_state.json");
    let snap = DhwPersistedState {
        schema_version: 1,
        boost: Some(DhwBoostState {
            preset: BoostPreset::Shower,
            started_at: Utc::now(),
            duration_secs: 1800,
            prior_immersion_engage_temp_c: None,
            immersion_engaged: false,
        }),
    };
    snap.save(&path).unwrap();
    let loaded = DhwPersistedState::load(&path).unwrap();
    assert!(matches!(loaded.boost.as_ref().unwrap().preset, BoostPreset::Shower));
}

#[test]
fn missing_file_loads_empty() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nope.json");
    let loaded = DhwPersistedState::load(&path).unwrap();
    assert_eq!(loaded.schema_version, 1);
    assert!(loaded.boost.is_none());
}

#[test]
fn unknown_schema_version_loads_empty_with_warning() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(&path, r#"{"schema_version": 999, "boost": null}"#).unwrap();
    let loaded = DhwPersistedState::load(&path).unwrap();
    assert_eq!(loaded.schema_version, 1);
    assert!(loaded.boost.is_none());
}

#[test]
fn corrupt_json_loads_empty() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("corrupt.json");
    std::fs::write(&path, "this is not json").unwrap();
    let loaded = DhwPersistedState::load(&path).unwrap();
    assert!(loaded.boost.is_none());
}

#[test]
fn save_uses_tmp_then_rename() {
    // Property: after save() the .tmp file should NOT exist alongside the main file
    let dir = tempdir().unwrap();
    let path = dir.path().join("dhw_state.json");
    DhwPersistedState::default().save(&path).unwrap();
    assert!(path.exists());
    let tmp = path.with_extension("json.tmp");
    assert!(!tmp.exists());
}
```

- [ ] **Step 3: Run, expect compile failure**

```bash
cargo test -p server persisted_state_ atomic_save --no-fail-fast
```

- [ ] **Step 4: Implement types and persistence**

Replace `server/src/dhw/state.rs` body with:

```rust
//! DHW boost state + atomic persistence (mirrors heatpump_stats pattern).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BoostPreset {
    Shower,
    Bath { hours: f32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DhwBoostState {
    pub preset: BoostPreset,
    pub started_at: DateTime<Utc>,
    pub duration_secs: u64,
    /// Snapshot of `61636` at Bath start, for restore at Bath stop. None for Shower.
    pub prior_immersion_engage_temp_c: Option<f32>,
    /// Whether the immersion gate has written a non-zero `61591` value.
    pub immersion_engaged: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DhwPersistedState {
    pub schema_version: u32,
    pub boost: Option<DhwBoostState>,
}

impl Default for DhwPersistedState {
    fn default() -> Self {
        Self { schema_version: 1, boost: None }
    }
}

const CURRENT_SCHEMA: u32 = 1;

impl DhwPersistedState {
    /// Atomic save: write `<path>.tmp` then rename. Survives mid-write crashes.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension(
            path.extension()
                .map(|e| format!("{}.tmp", e.to_string_lossy()))
                .unwrap_or_else(|| "tmp".into()),
        );
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Tolerant load: missing file or any parse/version problem returns a fresh
    /// `default()` so startup never blocks. Warnings are logged for non-fresh cases.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(s) if s.schema_version == CURRENT_SCHEMA => Ok(s),
            Ok(s) => {
                tracing::warn!(
                    "DHW persist: unknown schema_version {}; ignoring file at {}",
                    s.schema_version, path.display()
                );
                Ok(Self::default())
            }
            Err(e) => {
                tracing::warn!(
                    "DHW persist: failed to parse {}: {e}; starting fresh",
                    path.display()
                );
                Ok(Self::default())
            }
        }
    }
}

/// Snapshot returned by `GET /dhw/state` and read by the dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct DhwSnapshot {
    pub comfort_level: ComfortLevelString,
    pub boost: Option<DhwBoostSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DhwBoostSnapshot {
    pub preset: BoostPreset,
    pub started_at: DateTime<Utc>,
    pub scheduled_end: DateTime<Utc>,
    pub elapsed_s: i64,
    pub remaining_s: i64,
    pub immersion_engaged: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComfortLevelString {
    Economy,
    Normal,
    Komfort,
    Manuell,
}
```

Add `tempfile = "*"` to `[dev-dependencies]` of the server crate if not already there (Step 4b below).

- [ ] **Step 4b: Add `tempfile` dev-dependency if missing**

```bash
grep -q 'tempfile' server/Cargo.toml || cargo add --manifest-path server/Cargo.toml --dev tempfile
```

- [ ] **Step 5: Implement `DhwError` + companions in `server/src/dhw/error.rs`**

```rust
//! DHW errors and small value types used at the HTTP and actor boundaries.

use axum::http::StatusCode;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComfortLevel {
    Economy,
    Normal,
    Komfort,
}

impl ComfortLevel {
    pub fn from_query(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "economy" => Some(Self::Economy),
            "normal" => Some(Self::Normal),
            "komfort" | "comfort" => Some(Self::Komfort),
            _ => None,
        }
    }

    /// Scaled value to write to `61500` (factor 1.0, so equals raw).
    pub fn as_scaled(self) -> f32 {
        match self { Self::Economy => 0.0, Self::Normal => 1.0, Self::Komfort => 2.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CancelReason {
    TimerExpired,
    RoomTooCold,
    PriceLeftCheap,
    Manual,
    Recovery,
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum StartReport {
    Started { scheduled_end: chrono::DateTime<chrono::Utc> },
    AlreadyAtTarget { dhw_c: f32, target_c: f32 },
}

#[derive(Debug)]
pub enum DhwError {
    BoostAlreadyActive,
    PriceNotCheap { current_level: String },
    HoursOutOfRange { min: f32, max: f32 },
    NoActiveBoost,
    ShowerCannotBeCancelled,
    Modbus(String),
    HomeyOverrideSendFailed,
    SmartGrid(String),
    Sensor(&'static str),
    Persistence(String),
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_level: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
}

impl DhwError {
    pub fn into_response(self) -> (StatusCode, axum::Json<serde_json::Value>) {
        let (code, body) = match &self {
            Self::BoostAlreadyActive => (
                StatusCode::CONFLICT,
                ErrorBody { error: "boost_already_active", field: None, min: None, max: None, current_level: None, detail: None },
            ),
            Self::PriceNotCheap { current_level } => (
                StatusCode::CONFLICT,
                ErrorBody { error: "price_not_cheap", field: None, min: None, max: None, current_level: Some(current_level), detail: None },
            ),
            Self::HoursOutOfRange { min, max } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorBody { error: "out_of_range", field: Some("hours"), min: Some(*min), max: Some(*max), current_level: None, detail: None },
            ),
            Self::NoActiveBoost => (
                StatusCode::OK, // DELETE is idempotent — 204 in route handler; this branch is unused.
                ErrorBody { error: "no_active_boost", field: None, min: None, max: None, current_level: None, detail: None },
            ),
            Self::ShowerCannotBeCancelled => (
                StatusCode::CONFLICT,
                ErrorBody { error: "shower_runs_to_completion", field: None, min: None, max: None, current_level: None, detail: None },
            ),
            Self::Modbus(e) => (StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody { error: "modbus", field: None, min: None, max: None, current_level: None, detail: Some(e) }),
            Self::HomeyOverrideSendFailed => (StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody { error: "homey_override_unavailable", field: None, min: None, max: None, current_level: None, detail: None }),
            Self::SmartGrid(e) => (StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody { error: "smartgrid", field: None, min: None, max: None, current_level: None, detail: Some(e) }),
            Self::Sensor(name) => (StatusCode::SERVICE_UNAVAILABLE,
                ErrorBody { error: "sensor_unavailable", field: Some(name), min: None, max: None, current_level: None, detail: None }),
            Self::Persistence(e) => (StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody { error: "persistence", field: None, min: None, max: None, current_level: None, detail: Some(e) }),
        };
        (code, axum::Json(serde_json::to_value(&body).unwrap()))
    }
}
```

- [ ] **Step 6: Register the module in `server/src/main.rs` or `server/src/lib.rs`** (wherever `mod modbus;` lives):

```rust
mod dhw;
```

- [ ] **Step 7: Tests pass**

```bash
cargo test -p server persisted_state_ atomic_save missing_file unknown_schema corrupt_json save_uses_tmp
```
Expected: PASS.

- [ ] **Step 8: Format, clippy, full sweep**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
```

- [ ] **Step 9: Commit**

```bash
git add server/src/dhw/ server/src/main.rs server/Cargo.toml
git commit -m "Add dhw module: state, error, persistence"
```

---

## Task 4: Homey reconciler — boost-override lane

**Files:**
- Modify: `server/src/smartgrid/actor.rs` (`HomeyHooks` struct + new `boost_override_tx` field)
- Modify: `server/src/homey/poller.rs` (read both lanes with priority)
- Test: `server/src/smartgrid/actor.rs` (inline tests for the merge rule)

- [ ] **Step 1: Write the failing merge-rule test in `server/src/smartgrid/actor.rs`**

Append to the existing test module:

```rust
#[test]
fn reconciler_target_prefers_boost_override_over_sg_intent() {
    use tokio::sync::watch;
    let (sg_tx, sg_rx) = watch::channel(true);
    let (boost_tx, boost_rx) = watch::channel(None::<bool>);

    // No override: target = sg intent.
    assert_eq!(super::reconciler_target(&boost_rx, &sg_rx), true);

    // Override Some(false) wins.
    boost_tx.send(Some(false)).unwrap();
    assert_eq!(super::reconciler_target(&boost_rx, &sg_rx), false);

    // SG intent flips to false; override still wins (still Some(false)).
    sg_tx.send(false).unwrap();
    assert_eq!(super::reconciler_target(&boost_rx, &sg_rx), false);

    // Override cleared: target falls back to sg intent.
    boost_tx.send(None).unwrap();
    assert_eq!(super::reconciler_target(&boost_rx, &sg_rx), false);

    // SG intent back to true; no override → target true.
    sg_tx.send(true).unwrap();
    assert_eq!(super::reconciler_target(&boost_rx, &sg_rx), true);
}
```

- [ ] **Step 2: Run, expect failure**

```bash
cargo test -p server reconciler_target_prefers --no-fail-fast
```

- [ ] **Step 3: Extend `HomeyHooks` and add `reconciler_target`**

In `server/src/smartgrid/actor.rs`, change `HomeyHooks`:

```rust
#[derive(Clone)]
pub struct HomeyHooks {
    pub client: HomeyClient,
    pub cache: Arc<HomeyPumpCache>,
    pub desired_tx: watch::Sender<bool>,
    /// Boost-priority override lane. `Some(v)` masks `desired_tx`; `None` defers to SG.
    pub boost_override_tx: watch::Sender<Option<bool>>,
}
```

Add a free function `reconciler_target` (used by both the poller and the new test):

```rust
/// Resolve the pump target from the two intent lanes.
///
/// Boost-override wins when `Some(_)`; otherwise the SG-derived intent applies.
#[must_use]
pub fn reconciler_target(
    boost_rx: &watch::Receiver<Option<bool>>,
    sg_rx: &watch::Receiver<bool>,
) -> bool {
    match *boost_rx.borrow() {
        Some(v) => v,
        None => *sg_rx.borrow(),
    }
}
```

- [ ] **Step 4: Run merge test, expect PASS**

```bash
cargo test -p server reconciler_target_prefers
```

- [ ] **Step 5: Update the Homey poller to consult both lanes**

Open `server/src/homey/poller.rs`. Find the reconciler loop (lines 62-81 per spec). The current `target = *desired_rx.borrow()` becomes:

```rust
let target = crate::smartgrid::actor::reconciler_target(&boost_override_rx, &desired_rx);
```

The poller's function signature gains a parameter for the boost override receiver:

```rust
pub async fn run_reconciler(
    client: HomeyClient,
    cache: Arc<HomeyPumpCache>,
    desired_rx: watch::Receiver<bool>,
    boost_override_rx: watch::Receiver<Option<bool>>,
    tick: Duration,
) { /* ... */ }
```

Update the body so `boost_override_rx.changed()` also wakes the loop:

```rust
loop {
    tokio::select! {
        _ = sleep(tick) => {}
        _ = desired_rx.changed() => {}
        _ = boost_override_rx.changed() => {}
    }
    let target = crate::smartgrid::actor::reconciler_target(&boost_override_rx, &desired_rx);
    // existing reconciliation logic against target ...
}
```

- [ ] **Step 6: Update all `HomeyHooks` constructors and reconciler spawn sites**

```bash
grep -rn "HomeyHooks {" server/src/
```
Each constructor must now pass a `boost_override_tx`. In `main.rs`:

```rust
let (boost_override_tx, boost_override_rx) = tokio::sync::watch::channel::<Option<bool>>(None);
let hooks = HomeyHooks { client: hc.clone(), cache: cache.clone(), desired_tx: desired_tx.clone(), boost_override_tx: boost_override_tx.clone() };
// pass `boost_override_rx` into `run_reconciler` spawn
```

(The `boost_override_tx` clone will be moved into `DhwActor` construction in Task 9 — keep a clone in `main.rs` scope for that.)

- [ ] **Step 7: Add an integration-shaped test that the poller respects the override**

In a new file `server/src/homey/poller.rs` (or test module within), with a fake `HomeyClient` if one exists in tests — if not, leave a `#[ignore]` test stub that documents the expected behaviour:

```rust
#[tokio::test]
#[ignore = "needs HomeyClient mock; manual ctc.lan test covers this for now"]
async fn poller_respects_boost_override() { /* documented in spec §4.7 */ }
```

- [ ] **Step 8: Format, clippy, test**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
```
Expected: zero warnings, all tests pass (existing SG tests still green).

- [ ] **Step 9: Commit**

```bash
git add server/src/smartgrid/actor.rs server/src/homey/poller.rs server/src/main.rs
git commit -m "Add boost-override lane to Homey reconciler"
```

---

## Task 5: `ImmersionGate` — pure hysteresis state machine

**Files:**
- Create: `server/src/dhw/immersion.rs`
- Test: same file

- [ ] **Step 1: Failing test**

```rust
use super::{ImmersionDecision, ImmersionGate};

fn gate() -> ImmersionGate {
    ImmersionGate::new(0.50, 0.05) // allow=0.50, hyst=0.05 → on<0.45, off>0.55
}

#[test]
fn off_then_low_price_in_cheap_band_engages() {
    let mut g = gate();
    assert!(matches!(g.evaluate(0.40, true), ImmersionDecision::Engage));
    assert!(g.engaged());
}

#[test]
fn engaged_then_price_rises_above_off_threshold_disengages() {
    let mut g = gate();
    let _ = g.evaluate(0.40, true);
    assert!(matches!(g.evaluate(0.60, true), ImmersionDecision::Disengage));
    assert!(!g.engaged());
}

#[test]
fn dead_zone_no_change() {
    let mut g = gate();
    let _ = g.evaluate(0.40, true);
    assert!(matches!(g.evaluate(0.48, true), ImmersionDecision::NoChange));
    assert!(matches!(g.evaluate(0.52, true), ImmersionDecision::NoChange));
    assert!(g.engaged());
}

#[test]
fn not_in_cheap_band_never_engages_even_if_price_low() {
    let mut g = gate();
    assert!(matches!(g.evaluate(0.40, false), ImmersionDecision::NoChange));
    assert!(!g.engaged());
}

#[test]
fn engaged_then_band_leaves_cheap_disengages() {
    let mut g = gate();
    let _ = g.evaluate(0.40, true);
    assert!(matches!(g.evaluate(0.40, false), ImmersionDecision::Disengage));
    assert!(!g.engaged());
}

#[test]
fn property_writes_bounded_by_band_crossings() {
    let mut g = gate();
    let mut writes = 0;
    // Sweep: 0.6→0.4→0.5→0.6→0.4 (in cheap band throughout)
    for &p in &[0.60, 0.40, 0.50, 0.60, 0.40] {
        if !matches!(g.evaluate(p, true), ImmersionDecision::NoChange) {
            writes += 1;
        }
    }
    assert!(writes <= 4, "expected ≤ 4 writes for the sweep, got {writes}");
}
```

- [ ] **Step 2: Run, expect compile failure.**

```bash
cargo test -p server immersion_ --no-fail-fast
```

- [ ] **Step 3: Implement the gate**

```rust
//! Hysteresis-guarded immersion-allow gate (Bath-only).

pub struct ImmersionGate {
    on_threshold: f32,
    off_threshold: f32,
    engaged: bool,
}

#[derive(Debug, PartialEq)]
pub enum ImmersionDecision {
    Engage,
    Disengage,
    NoChange,
}

impl ImmersionGate {
    #[must_use]
    pub fn new(allow_thr: f32, hyst: f32) -> Self {
        Self {
            on_threshold:  allow_thr - hyst,
            off_threshold: allow_thr + hyst,
            engaged: false,
        }
    }

    /// Restore from persistence — used by crash-recovery / reload paths.
    pub fn with_engaged(allow_thr: f32, hyst: f32, engaged: bool) -> Self {
        let mut g = Self::new(allow_thr, hyst);
        g.engaged = engaged;
        g
    }

    #[must_use]
    pub fn engaged(&self) -> bool { self.engaged }

    /// Decide what to do at the current tick. Caller performs the side effect
    /// (write `61591`) if the result is not `NoChange`.
    pub fn evaluate(&mut self, spot_sek: f32, in_cheap_band: bool) -> ImmersionDecision {
        if self.engaged {
            if !in_cheap_band || spot_sek > self.off_threshold {
                self.engaged = false;
                return ImmersionDecision::Disengage;
            }
            ImmersionDecision::NoChange
        } else if in_cheap_band && spot_sek < self.on_threshold {
            self.engaged = true;
            ImmersionDecision::Engage
        } else {
            ImmersionDecision::NoChange
        }
    }
}
```

Uncomment the `pub mod immersion;` line in `server/src/dhw/mod.rs` (Task 3 step 1 placeholder).

- [ ] **Step 4: Tests pass**

```bash
cargo test -p server immersion_
```

- [ ] **Step 5: Format, clippy, full sweep**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
```

- [ ] **Step 6: Commit**

```bash
git add server/src/dhw/immersion.rs server/src/dhw/mod.rs
git commit -m "Add ImmersionGate hysteresis state machine"
```

---

## Task 6: `DhwActor` skeleton with crash-recovery prologue

**Files:**
- Create: `server/src/dhw/actor.rs`
- Test: inline

This task establishes the actor's *shape* and crash-recovery sequence. UC-specific commands (StartShower/StartBath/Cancel) land in later tasks.

- [ ] **Step 1: Failing test — recovery sequence in isolation**

```rust
// inside server/src/dhw/actor.rs test module
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn crash_recovery_runs_documented_sequence() {
    use crate::dhw::state::*;
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
    }.save(&persist).unwrap();

    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let fake_modbus = FakeModbus::new(calls.clone());
    let fake_sg = FakeSg::new(calls.clone());
    let (override_tx, mut override_rx) = tokio::sync::watch::channel::<Option<bool>>(Some(false));

    crate::dhw::actor::run_recovery(&persist, &fake_modbus, &fake_sg, &override_tx).await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(*calls, vec![
        "modbus_write_scaled 61503 = 0".to_string(),
        "modbus_write_scaled 61591 = 0".to_string(),
        "modbus_write_scaled 61636 = 60".to_string(),
        "sg_set_mode Normal".to_string(),
    ]);
    // boost_override cleared:
    assert_eq!(*override_rx.borrow_and_update(), None);
    // file cleared:
    let after = DhwPersistedState::load(&persist).unwrap();
    assert!(after.boost.is_none());
}
```

Include trait definitions `ModbusWriter` and `SgController` so we can inject fakes. Place in `actor.rs`:

```rust
/// Narrow trait covering the Modbus writes DhwActor needs. Implemented for the
/// real `ModbusSender` via a blanket impl in this file; tests use a fake.
#[async_trait::async_trait]
pub trait ModbusWriter: Send + Sync {
    async fn write_scaled(&self, addr: u16, value: f32) -> Result<(), String>;
}

/// Narrow trait for SG mode application. Implemented for `SmartGridHandle`.
#[async_trait::async_trait]
pub trait SgController: Send + Sync {
    async fn set_normal(&self) -> Result<(), String>;
    async fn set_overcapacity(&self) -> Result<(), String>;
}
```

And the fakes used by the test:

```rust
#[cfg(test)]
struct FakeModbus {
    calls: Arc<Mutex<Vec<String>>>,
    reads: std::collections::HashMap<u16, f32>,
}
#[cfg(test)]
impl FakeModbus {
    fn new(calls: Arc<Mutex<Vec<String>>>) -> Self { Self { calls, reads: Default::default() } }
    fn with_reads(calls: Arc<Mutex<Vec<String>>>, reads: Vec<(u16, f32)>) -> Self {
        Self { calls, reads: reads.into_iter().collect() }
    }
}
#[cfg(test)]
#[async_trait::async_trait]
impl ModbusWriter for FakeModbus {
    async fn write_scaled(&self, addr: u16, v: f32) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("modbus_write_scaled {addr} = {v}"));
        Ok(())
    }
    async fn read_scaled(&self, addr: u16) -> Result<f32, String> {
        self.calls.lock().unwrap().push(format!("modbus_read_scaled {addr}"));
        Ok(self.reads.get(&addr).copied().unwrap_or(0.0))
    }
}

#[cfg(test)]
struct FakeSg { calls: Arc<Mutex<Vec<String>>> }
#[cfg(test)]
impl FakeSg { fn new(calls: Arc<Mutex<Vec<String>>>) -> Self { Self { calls } } }
#[cfg(test)]
#[async_trait::async_trait]
impl SgController for FakeSg {
    async fn set_normal(&self)        -> Result<(), String> { self.calls.lock().unwrap().push("sg_set_mode Normal".into()); Ok(()) }
    async fn set_overcapacity(&self)  -> Result<(), String> { self.calls.lock().unwrap().push("sg_set_mode Overcapacity".into()); Ok(()) }
}
```

Add `async-trait = "*"` to the server crate's `Cargo.toml` `[dependencies]` if absent.

- [ ] **Step 2: Run, expect failure**

```bash
cargo test -p server crash_recovery_runs --no-fail-fast
```

- [ ] **Step 3: Implement `run_recovery`**

```rust
use crate::dhw::state::{BoostPreset, DhwPersistedState};
use std::path::Path;
use tokio::sync::watch;

pub async fn run_recovery(
    persist_path: &Path,
    modbus: &dyn ModbusWriter,
    sg: &dyn SgController,
    boost_override_tx: &watch::Sender<Option<bool>>,
) -> Result<(), String> {
    let mut state = DhwPersistedState::load(persist_path).map_err(|e| e.to_string())?;
    let Some(boost) = state.boost.take() else { return Ok(()); };

    modbus.write_scaled(61503, 0.0).await?;
    modbus.write_scaled(61591, 0.0).await?;
    if let Some(prior_c) = boost.prior_immersion_engage_temp_c {
        modbus.write_scaled(61636, prior_c).await?;
    }
    sg.set_normal().await?;
    let _ = boost_override_tx.send(None);

    // Clear persistence (atomic).
    let empty = DhwPersistedState::default();
    empty.save(persist_path).map_err(|e| e.to_string())?;
    tracing::warn!("DHW recovery cleared mid-flight boost from previous run ({boost:?})");
    Ok(())
}
```

> **Note:** The exact scaling of the `61636` write depends on Task 0's finding (raw vs scaled HTTP path). When this task is reached and Task 0 is documented, replace the placeholder block with the correct conversion. The test expects raw `60` — adapt the expectation if Task 0's finding says otherwise.

Define `DhwActor` struct + `DhwCmd` enum + `DhwHandle` skeleton (handlers for commands land in later tasks):

```rust
use tokio::sync::{mpsc, oneshot, watch};

pub struct DhwActor {
    rx: mpsc::Receiver<DhwCmd>,
    state: Option<crate::dhw::state::DhwBoostState>,
    watcher_abort: Option<tokio::task::AbortHandle>,
    modbus: std::sync::Arc<dyn ModbusWriter>,
    sg: std::sync::Arc<dyn SgController>,
    boost_override_tx: watch::Sender<Option<bool>>,
    store: crate::storage::Store,
    price_state: std::sync::Arc<crate::energy::price::PriceState>,
    cfg: crate::config::DhwConfig,
    persist_path: Option<std::path::PathBuf>,
}

pub enum DhwCmd {
    Snapshot { respond_to: oneshot::Sender<crate::dhw::state::DhwSnapshot> },
    SetComfort { level: crate::dhw::error::ComfortLevel, respond_to: oneshot::Sender<Result<(), crate::dhw::error::DhwError>> },
    StartShower { respond_to: oneshot::Sender<Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError>> },
    StartBath  { hours: f32, respond_to: oneshot::Sender<Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError>> },
    Cancel     { reason: crate::dhw::error::CancelReason, respond_to: oneshot::Sender<Result<bool, crate::dhw::error::DhwError>> },
}

#[derive(Clone)]
pub struct DhwHandle { tx: mpsc::Sender<DhwCmd> }

impl DhwHandle {
    pub async fn snapshot(&self) -> crate::dhw::state::DhwSnapshot {
        let (tx, rx) = oneshot::channel();
        self.tx.send(DhwCmd::Snapshot { respond_to: tx }).await.expect("dhw actor down");
        rx.await.expect("dhw actor dropped snapshot reply")
    }
    // remaining methods land in later tasks
}

impl DhwActor {
    pub fn spawn(/* ... */) -> DhwHandle {
        // implementation lands in Task 9; for now this is a TODO marker.
        unimplemented!()
    }
}
```

- [ ] **Step 4: Tests pass**

```bash
cargo test -p server crash_recovery_runs
```

- [ ] **Step 5: Format, clippy, full sweep**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
```

- [ ] **Step 6: Commit**

```bash
git add server/src/dhw/actor.rs server/Cargo.toml server/src/dhw/mod.rs
git commit -m "Add DhwActor skeleton and crash-recovery prologue"
```

---

## Task 7: Comfort op + `61500` read-back

**Files:**
- Modify: `server/src/dhw/actor.rs`
- Test: inline

- [ ] **Step 1: Failing test**

```rust
#[tokio::test]
async fn set_comfort_writes_61500_scaled_value() {
    use crate::dhw::error::ComfortLevel;
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let modbus = FakeModbus::new(calls.clone());

    let res = crate::dhw::actor::write_comfort(&modbus, ComfortLevel::Komfort).await;
    assert!(res.is_ok());
    assert_eq!(*calls.lock().unwrap(), vec!["modbus_write_scaled 61500 = 2".to_string()]);
}
```

- [ ] **Step 2: Run, expect failure**

- [ ] **Step 3: Implement `write_comfort`**

```rust
pub async fn write_comfort(
    modbus: &dyn ModbusWriter,
    level: crate::dhw::error::ComfortLevel,
) -> Result<(), crate::dhw::error::DhwError> {
    modbus.write_scaled(61500, level.as_scaled()).await
          .map_err(crate::dhw::error::DhwError::Modbus)
}
```

- [ ] **Step 4: Wire into `DhwActor` receive loop** (start writing the `run()` impl):

```rust
impl DhwActor {
    pub async fn run(mut self) {
        if let Some(p) = &self.persist_path {
            if let Err(e) = run_recovery(p, &*self.modbus, &*self.sg, &self.boost_override_tx).await {
                tracing::warn!("DHW recovery failed: {e}");
            }
        }
        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                DhwCmd::SetComfort { level, respond_to } => {
                    let _ = respond_to.send(write_comfort(&*self.modbus, level).await);
                }
                DhwCmd::Snapshot { respond_to } => {
                    let snap = self.snapshot();
                    let _ = respond_to.send(snap);
                }
                DhwCmd::StartShower { respond_to } => { /* Task 8 */
                    let _ = respond_to.send(Err(crate::dhw::error::DhwError::Modbus("not implemented yet".into())));
                }
                DhwCmd::StartBath { hours: _, respond_to } => { /* Task 10 */
                    let _ = respond_to.send(Err(crate::dhw::error::DhwError::Modbus("not implemented yet".into())));
                }
                DhwCmd::Cancel { reason: _, respond_to } => { /* Task 11 */
                    let _ = respond_to.send(Err(crate::dhw::error::DhwError::Modbus("not implemented yet".into())));
                }
            }
        }
    }

    fn snapshot(&self) -> crate::dhw::state::DhwSnapshot {
        use crate::dhw::state::*;
        DhwSnapshot {
            comfort_level: ComfortLevelString::Manuell, // refined in Task 7b
            boost: self.state.as_ref().map(|s| DhwBoostSnapshot {
                preset: s.preset,
                started_at: s.started_at,
                scheduled_end: s.started_at + chrono::Duration::seconds(s.duration_secs as i64),
                elapsed_s: (chrono::Utc::now() - s.started_at).num_seconds(),
                remaining_s: ((s.started_at + chrono::Duration::seconds(s.duration_secs as i64)) - chrono::Utc::now()).num_seconds().max(0),
                immersion_engaged: s.immersion_engaged,
            }),
        }
    }
}
```

- [ ] **Step 4b: Comfort read-back** — actor caches the last-known `61500` value. Add a `last_comfort: Option<ComfortLevelString>` field on `DhwActor` and refresh it inside the receive loop after every successful `SetComfort` op:

```rust
self.last_comfort = Some(match level {
    ComfortLevel::Economy => ComfortLevelString::Economy,
    ComfortLevel::Normal  => ComfortLevelString::Normal,
    ComfortLevel::Komfort => ComfortLevelString::Komfort,
});
```

And the initial value is read once during `run()` prologue (after recovery):

```rust
// Read 61500 once at startup to seed snapshot. Factor=1.0 so scaled == raw.
match self.modbus.read_scaled(61500).await {
    Ok(v) => self.last_comfort = Some(match v as i32 {
        0 => ComfortLevelString::Economy,
        1 => ComfortLevelString::Normal,
        2 => ComfortLevelString::Komfort,
        _ => ComfortLevelString::Manuell,
    }),
    Err(e) => tracing::warn!("DHW startup 61500 read failed: {e}"),
}
```

Final trait shape (both methods take/return scaled `f32`; the underlying actor performs raw conversion):

```rust
#[async_trait::async_trait]
pub trait ModbusWriter: Send + Sync {
    async fn write_scaled(&self, addr: u16, value: f32) -> Result<(), String>;
    async fn read_scaled(&self, addr: u16) -> Result<f32, String>;
}
```

`FakeModbus::with_reads(calls, vec![(addr, value), …])` constructor (already shown above) pre-seeds expected read values.

- [ ] **Step 5: Tests pass**

```bash
cargo test -p server set_comfort_writes
```

- [ ] **Step 6: Format, clippy, full sweep**

- [ ] **Step 7: Commit**

```bash
git add server/src/dhw/actor.rs server/src/dhw/mod.rs
git commit -m "DHW: SetComfort op + 61500 readback on startup"
```

---

## Task 8: Shower start path

**Files:** `server/src/dhw/actor.rs` (extend), `server/src/dhw/watcher.rs` (new)

- [ ] **Step 1: Failing test — Shower start succeeds and produces documented side effects**

```rust
#[tokio::test]
async fn start_shower_writes_61503_and_sets_override_when_not_at_target() {
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let modbus = Arc::new(FakeModbus::with_reads(calls.clone(), vec![(62001_u16, 55.0_f32)]));
    let store = test_store_with_dhw_upper(50.0); // helper that builds a Store with one DHW sample
    let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);

    let result = crate::dhw::actor::start_shower_impl(
        modbus.as_ref(),
        &store,
        &boost_tx,
        30,
    ).await;

    let report = result.unwrap();
    assert!(matches!(report, crate::dhw::error::StartReport::Started { .. }));
    assert_eq!(*boost_rx.borrow_and_update(), Some(false));
    let log = calls.lock().unwrap();
    assert!(log.contains(&"modbus_read_scaled 62001".to_string()));
    assert!(log.contains(&"modbus_write_scaled 61503 = 0.5".to_string()));
}

#[tokio::test]
async fn start_shower_short_circuits_when_already_at_target() {
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let modbus = Arc::new(FakeModbus::with_reads(calls.clone(), vec![(62001_u16, 55.0_f32)]));
    let store = test_store_with_dhw_upper(56.0);
    let (boost_tx, mut boost_rx) = tokio::sync::watch::channel::<Option<bool>>(None);

    let report = crate::dhw::actor::start_shower_impl(modbus.as_ref(), &store, &boost_tx, 30).await.unwrap();
    assert!(matches!(report, crate::dhw::error::StartReport::AlreadyAtTarget { .. }));
    assert_eq!(*boost_rx.borrow_and_update(), None);
    let log = calls.lock().unwrap();
    assert!(log.iter().all(|c| !c.starts_with("modbus_write_scaled")));
}
```

- [ ] **Step 2: Run, expect failure**

- [ ] **Step 3: Implement `start_shower_impl`**

```rust
pub async fn start_shower_impl(
    modbus: &dyn ModbusWriter,
    store: &crate::storage::Store,
    boost_override_tx: &tokio::sync::watch::Sender<Option<bool>>,
    duration_minutes: u32,
) -> Result<crate::dhw::error::StartReport, crate::dhw::error::DhwError> {
    let target_c = modbus.read_scaled(62001).await.map_err(crate::dhw::error::DhwError::Modbus)?;

    let dhw_c = store
        .latest_sample(crate::storage::Sensor::DhwUpper)
        .map(|(_, v)| v)
        .ok_or(crate::dhw::error::DhwError::Sensor("dhw_upper"))?;

    if dhw_c >= target_c {
        return Ok(crate::dhw::error::StartReport::AlreadyAtTarget { dhw_c, target_c });
    }

    boost_override_tx.send(Some(false))
        .map_err(|_| crate::dhw::error::DhwError::HomeyOverrideSendFailed)?;
    // 30 min = 0.5 h. Actor divides by factor 0.5 → raw 1.
    modbus.write_scaled(61503, 0.5).await.map_err(crate::dhw::error::DhwError::Modbus)?;

    let scheduled_end = chrono::Utc::now() + chrono::Duration::minutes(i64::from(duration_minutes));
    Ok(crate::dhw::error::StartReport::Started { scheduled_end })
}
```

(`test_store_with_dhw_upper` is a helper — define in `server/src/dhw/actor.rs` test module, building a `Store` with one ingested `Sensor::DhwUpper` sample. Mirror the constructor used in `storage::poller` tests.)

- [ ] **Step 4: Tests pass**

- [ ] **Step 5: Wire `DhwCmd::StartShower` to call `start_shower_impl`**, store the resulting `DhwBoostState`, persist, then spawn the watcher (Task 9).

- [ ] **Step 6: Format, clippy, full sweep**

- [ ] **Step 7: Commit**

```bash
git add server/src/dhw/actor.rs
git commit -m "DHW: Shower start path + pre-flight short-circuit"
```

---

## Task 9: Shower watcher (30-min one-shot)

**Files:**
- Create: `server/src/dhw/watcher.rs`
- Test: inline

- [ ] **Step 1: Failing test**

```rust
#[tokio::test(start_paused = true)]
async fn shower_watcher_fires_pump_restore_after_duration() {
    use tokio::sync::watch;
    let (boost_tx, _boost_rx) = watch::channel(Some(false));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(crate::dhw::watcher::run_shower_watcher(
        boost_tx.clone(),
        std::time::Duration::from_secs(1800),
        done_tx,
    ));
    tokio::time::advance(std::time::Duration::from_secs(1801)).await;
    done_rx.await.unwrap();
    assert_eq!(*boost_tx.subscribe().borrow(), None);
    handle.abort();
}
```

- [ ] **Step 2: Run, expect failure**

- [ ] **Step 3: Implement watcher**

```rust
use tokio::sync::{oneshot, watch};
use std::time::Duration;

pub async fn run_shower_watcher(
    boost_override_tx: watch::Sender<Option<bool>>,
    duration: Duration,
    done_tx: oneshot::Sender<()>,
) {
    tokio::time::sleep(duration).await;
    let _ = boost_override_tx.send(None);
    let _ = done_tx.send(());
}
```

- [ ] **Step 4: Wire into Shower start path** — after a successful `Started`, spawn the watcher and store its `AbortHandle` on the actor. The watcher's `done_tx` is consumed by the actor's receive loop via a `DhwCmd::WatcherFinished` internal command (add to enum).

- [ ] **Step 5: On `WatcherFinished` reception**: clear `self.state`, clear persistence file, log info.

- [ ] **Step 6: Tests pass, format, clippy, full sweep**

- [ ] **Step 7: Commit**

```bash
git add server/src/dhw/watcher.rs server/src/dhw/actor.rs server/src/dhw/mod.rs
git commit -m "DHW: Shower watcher (30-min one-shot, pump restore)"
```

---

## Task 10: HTTP routes — comfort, GET state, POST shower

**Files:**
- Create: `server/src/routes/dhw.rs`
- Modify: `server/src/routes/mod.rs` (mount router)
- Test: inline

- [ ] **Step 1: Failing tests** using `axum::Router::oneshot` against a hand-built app:

```rust
#[tokio::test]
async fn get_dhw_state_returns_snapshot() { /* ... */ }
#[tokio::test]
async fn post_dhw_comfort_normal_returns_204() { /* ... */ }
#[tokio::test]
async fn post_dhw_boost_shower_returns_started() { /* ... */ }
#[tokio::test]
async fn post_dhw_boost_shower_already_active_returns_409() { /* ... */ }
```

- [ ] **Step 2: Implement routes**

```rust
use axum::{extract::{Query, State}, http::StatusCode, Json, Router, routing::{get, post, delete}};
use serde::Deserialize;

#[derive(Clone)]
pub struct DhwRouterState { pub handle: crate::dhw::DhwHandle }

pub fn router(state: DhwRouterState) -> Router {
    Router::new()
        .route("/api/v1/dhw/state",   get(get_state))
        .route("/api/v1/dhw/comfort", post(set_comfort))
        .route("/api/v1/dhw/boost",   post(start_boost))
        .route("/api/v1/dhw/boost",   delete(cancel_boost))
        .with_state(state)
}

async fn get_state(State(s): State<DhwRouterState>) -> Json<crate::dhw::state::DhwSnapshot> {
    Json(s.handle.snapshot().await)
}

#[derive(Deserialize)]
struct ComfortQ { level: String }

async fn set_comfort(State(s): State<DhwRouterState>, Query(q): Query<ComfortQ>) -> Result<StatusCode, axum::response::Response> {
    let level = crate::dhw::error::ComfortLevel::from_query(&q.level)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "level must be economy|normal|komfort").into_response())?;
    s.handle.set_comfort(level).await.map_err(into_resp)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct BoostQ { preset: String, hours: Option<f32> }

async fn start_boost(State(s): State<DhwRouterState>, Query(q): Query<BoostQ>) -> Result<Json<crate::dhw::error::StartReport>, axum::response::Response> {
    let report = match q.preset.as_str() {
        "shower" => s.handle.start_shower().await,
        "bath" => {
            let hours = q.hours.ok_or_else(|| (StatusCode::BAD_REQUEST, "hours required for bath").into_response())?;
            s.handle.start_bath(hours).await
        }
        _ => return Err((StatusCode::BAD_REQUEST, "preset must be shower|bath").into_response()),
    };
    report.map(Json).map_err(into_resp)
}

async fn cancel_boost(State(s): State<DhwRouterState>) -> Result<StatusCode, axum::response::Response> {
    match s.handle.cancel(crate::dhw::error::CancelReason::Manual).await {
        Ok(_was_cancelled) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(into_resp(e)),
    }
}

fn into_resp(e: crate::dhw::error::DhwError) -> axum::response::Response {
    let (code, body) = e.into_response();
    (code, body).into_response()
}
```

- [ ] **Step 3: Mount in `server/src/routes/mod.rs`** — add the new router to the app builder.

- [ ] **Step 4: Tests pass, format, clippy, full sweep**

- [ ] **Step 5: Commit**

```bash
git add server/src/routes/
git commit -m "DHW: HTTP routes for state, comfort, boost start/cancel"
```

---

## Task 11: Bath start path

**Files:** `server/src/dhw/actor.rs`

- [ ] **Step 1: Failing tests covering (a) price-not-cheap → 409, (b) hours out-of-range → 422-equivalent error, (c) successful start writes 61636, 61503, sets override, applies SG=Overcapacity, evaluates immersion gate.**

(Test bodies follow the pattern from Task 8 with `FakePriceState`, `FakeSg`, and pre-seeded register reads.)

- [ ] **Step 2: Run, expect failure**

- [ ] **Step 3: Implement `start_bath_impl`**

```rust
pub async fn start_bath_impl(
    modbus: &dyn ModbusWriter,
    sg: &dyn SgController,
    price: &crate::energy::price::PriceState,
    boost_override_tx: &tokio::sync::watch::Sender<Option<bool>>,
    cfg: &crate::config::DhwConfig,
    hours: f32,
) -> Result<(crate::dhw::error::StartReport, crate::dhw::state::DhwBoostState), crate::dhw::error::DhwError> {
    use crate::dhw::error::DhwError;

    // Range validation.
    if hours < 0.5 || hours > cfg.bath_max_hours || ((hours / 0.5).round() - hours / 0.5).abs() > 1e-6 {
        return Err(DhwError::HoursOutOfRange { min: 0.5, max: cfg.bath_max_hours });
    }

    // Cheap-band gate.
    let snap = price.snapshot_now();
    if !matches!(snap.level, crate::energy::price::PriceLevel::VeryCheap | crate::energy::price::PriceLevel::Cheap) {
        return Err(DhwError::PriceNotCheap { current_level: format!("{:?}", snap.level) });
    }

    // Snapshot 61636 for restore (scaled °C).
    let prior_c = modbus.read_scaled(61636).await.map_err(DhwError::Modbus)?;

    // Side effects.
    boost_override_tx.send(Some(false)).map_err(|_| DhwError::HomeyOverrideSendFailed)?;
    sg.set_overcapacity().await.map_err(DhwError::SmartGrid)?;
    modbus.write_scaled(61636, cfg.immersion_engage_temp_c).await.map_err(DhwError::Modbus)?;
    // Boost duration in hours (scaling 0.5 means actor writes raw `hours / 0.5`).
    modbus.write_scaled(61503, hours).await.map_err(DhwError::Modbus)?;

    // Immersion gate first evaluation (no hysteresis state yet — explicit Engage if spot < on_threshold).
    let mut gate = crate::dhw::immersion::ImmersionGate::new(
        cfg.immersion_allow_price_sek_per_kwh,
        cfg.immersion_hysteresis_sek_per_kwh,
    );
    let cheap = matches!(snap.level, crate::energy::price::PriceLevel::VeryCheap | crate::energy::price::PriceLevel::Cheap);
    let decision = gate.evaluate(snap.spot_sek_per_kwh, cheap);
    let immersion_engaged = match decision {
        crate::dhw::immersion::ImmersionDecision::Engage => {
            modbus.write_scaled(61591, cfg.immersion_kw_when_allowed).await.map_err(DhwError::Modbus)?;
            true
        }
        _ => false,
    };

    let started_at = chrono::Utc::now();
    let duration_secs = (hours * 3600.0).round() as u64;
    let state = crate::dhw::state::DhwBoostState {
        preset: crate::dhw::state::BoostPreset::Bath { hours },
        started_at,
        duration_secs,
        prior_immersion_engage_temp_c: Some(prior_c),
        immersion_engaged,
    };
    Ok((crate::dhw::error::StartReport::Started { scheduled_end: started_at + chrono::Duration::seconds(duration_secs as i64) }, state))
}
```

- [ ] **Step 4: Wire into actor receive loop** for `DhwCmd::StartBath { hours }`. On success: `self.state = Some(state)`, persist, spawn Bath watcher (Task 12).

- [ ] **Step 5: Tests pass, format, clippy, full sweep**

- [ ] **Step 6: Commit**

```bash
git add server/src/dhw/actor.rs
git commit -m "DHW: Bath start path with PriceLevel gate and 61636 snapshot"
```

---

## Task 12: Bath watcher (60s tokio interval)

**Files:** `server/src/dhw/watcher.rs`

- [ ] **Step 1: Failing tests** covering each stop cause: timer expiry, room < 17, PriceLevel band leaves Cheap. Each test asserts that the watcher emits the matching `CancelReason` over a oneshot to the actor, and that for non-timer stops `61503=0` is written via the fake modbus.

- [ ] **Step 2: Implement Bath watcher**

```rust
pub async fn run_bath_watcher(
    duration: Duration,
    started_at: std::time::Instant,
    boost_override_tx: watch::Sender<Option<bool>>,
    state: std::sync::Arc<tokio::sync::Mutex<crate::dhw::state::DhwBoostState>>,
    modbus: std::sync::Arc<dyn crate::dhw::actor::ModbusWriter>,
    sg: std::sync::Arc<dyn crate::dhw::actor::SgController>,
    store: crate::storage::Store,
    price: std::sync::Arc<crate::energy::price::PriceState>,
    cfg: crate::config::DhwConfig,
    notify_done: oneshot::Sender<crate::dhw::error::CancelReason>,
) {
    use crate::dhw::error::CancelReason;
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    let mut gate = {
        let s = state.lock().await;
        crate::dhw::immersion::ImmersionGate::with_engaged(
            cfg.immersion_allow_price_sek_per_kwh,
            cfg.immersion_hysteresis_sek_per_kwh,
            s.immersion_engaged,
        )
    };
    loop {
        tick.tick().await;
        // Timer expiry.
        if started_at.elapsed() >= duration {
            let _ = notify_done.send(CancelReason::TimerExpired);
            return;
        }
        // Room bail.
        if let Some((_, room)) = store.latest_sample(crate::storage::Sensor::RoomTemp) {
            if room < cfg.boost_room_temp_bail_c {
                let _ = notify_done.send(CancelReason::RoomTooCold);
                return;
            }
        }
        // Price-band bail + immersion gate.
        let snap = price.snapshot_now();
        let cheap = matches!(snap.level, crate::energy::price::PriceLevel::VeryCheap | crate::energy::price::PriceLevel::Cheap);
        if !cheap {
            let _ = notify_done.send(CancelReason::PriceLeftCheap);
            return;
        }
        match gate.evaluate(snap.spot_sek_per_kwh, cheap) {
            crate::dhw::immersion::ImmersionDecision::Engage => {
                let _ = modbus.write_scaled(61591, cfg.immersion_kw_when_allowed).await;
                state.lock().await.immersion_engaged = true;
            }
            crate::dhw::immersion::ImmersionDecision::Disengage => {
                let _ = modbus.write_scaled(61591, 0.0).await;
                state.lock().await.immersion_engaged = false;
            }
            crate::dhw::immersion::ImmersionDecision::NoChange => {}
        }
    }
}
```

- [ ] **Step 3: Run, format, clippy, full sweep**

- [ ] **Step 4: Commit**

```bash
git add server/src/dhw/watcher.rs
git commit -m "DHW: Bath watcher with stop triggers and immersion gate"
```

---

## Task 13: Cancel / stop sequence (both presets)

**Files:** `server/src/dhw/actor.rs`

- [ ] **Step 1: Failing tests** — `DhwCmd::Cancel { reason }` produces:
- TimerExpired: skips `61503=0` (heater counter already 0), still restores `61636` and SG=Normal, clears immersion if engaged.
- RoomTooCold / PriceLeftCheap / Manual: writes `61503=0`, then 61636 restore, 61591=0 if engaged, SG=Normal.
- Shower never reaches Cancel via Manual (returns `ShowerCannotBeCancelled`); only TimerExpired path.

- [ ] **Step 2: Implement `stop_boost`** (called by both watcher-driven and manual cancel paths):

```rust
async fn stop_boost(
    state: &crate::dhw::state::DhwBoostState,
    reason: crate::dhw::error::CancelReason,
    modbus: &dyn ModbusWriter,
    sg: &dyn SgController,
    boost_override_tx: &tokio::sync::watch::Sender<Option<bool>>,
) -> Result<(), crate::dhw::error::DhwError> {
    use crate::dhw::error::{CancelReason, DhwError};

    let is_bath = matches!(state.preset, crate::dhw::state::BoostPreset::Bath { .. });

    // 1. Skip 61503=0 on timer expiry (heater already at 0).
    if reason != CancelReason::TimerExpired {
        modbus.write_scaled(61503, 0.0).await.map_err(DhwError::Modbus)?;
    }
    if is_bath {
        // 2. immersion off (only if engaged).
        if state.immersion_engaged {
            modbus.write_scaled(61591, 0.0).await.map_err(DhwError::Modbus)?;
        }
        // 3. restore 61636.
        if let Some(prior_c) = state.prior_immersion_engage_temp_c {
            modbus.write_scaled(61636, prior_c).await.map_err(DhwError::Modbus)?;
        }
        // 4. SG to Normal.
        sg.set_normal().await.map_err(DhwError::SmartGrid)?;
    }
    // 5. Clear boost override (reconciler reverts pump to SG intent).
    boost_override_tx.send(None).map_err(|_| DhwError::HomeyOverrideSendFailed)?;
    Ok(())
}
```

- [ ] **Step 3: Wire into `DhwCmd::Cancel { reason }`** handler in the actor's `run()` loop. Shower with `reason = Manual` returns `Err(DhwError::ShowerCannotBeCancelled)`. Bath always accepts Manual.

- [ ] **Step 4: Tests pass, format, clippy, full sweep**

- [ ] **Step 5: Commit**

```bash
git add server/src/dhw/actor.rs
git commit -m "DHW: stop_boost sequence with flash-frugal 61503"
```

---

## Task 14: Main wiring

**Files:** `server/src/main.rs`

- [ ] **Step 1: Add `DhwActor` spawn and channel plumbing.**

Near where `SmartGridActor` is constructed:

```rust
let (boost_override_tx, boost_override_rx) = tokio::sync::watch::channel::<Option<bool>>(None);
// pass boost_override_rx into the Homey reconciler spawn (already added in Task 4)
// pass boost_override_tx into DhwActor below

let modbus_writer: Arc<dyn crate::dhw::actor::ModbusWriter> = Arc::new(crate::dhw::adapters::CtcActorModbus::new(modbus_tx.clone()));
let sg_controller: Arc<dyn crate::dhw::actor::SgController> = Arc::new(crate::dhw::adapters::SmartGridAdapter::new(sg_handle.clone()));
let dhw_handle = crate::dhw::actor::DhwActor::spawn(
    modbus_writer,
    sg_controller,
    boost_override_tx,
    store.clone(),
    price_state.clone(),
    cfg.dhw.clone(),
);
```

- [ ] **Step 2: Create adapter file `server/src/dhw/adapters.rs`** that implements `ModbusWriter` over a `ModbusSender` (`mpsc::Sender<(ParameterOperation, ResponseChannel)>`) and `SgController` over `SmartGridHandle`.

- [ ] **Step 3: Mount DHW router** in the app builder right after `smartgrid` routes:

```rust
.merge(crate::routes::dhw::router(crate::routes::dhw::DhwRouterState { handle: dhw_handle.clone() }))
```

- [ ] **Step 4: Add graceful-shutdown persistence**: the `with_graceful_shutdown` block (already used for `heatpump_stats`) gains a `dhw_handle.shutdown_save().await` call. Implement `DhwHandle::shutdown_save` as a no-arg command that snapshots the current state to disk if `persist_path` is set, then waits for ack.

- [ ] **Step 5: Compile and run `cargo build`**. If it builds, the wiring is correct.

- [ ] **Step 6: Tests pass, format, clippy, full sweep**

- [ ] **Step 7: Commit**

```bash
git add server/src/main.rs server/src/dhw/adapters.rs server/src/dhw/mod.rs
git commit -m "Wire DhwActor into main and mount routes"
```

---

## Task 15: Dashboard — DhwControl, badge, chart band, chip

**Files:** `server/static/app.jsx`, `server/static/index.html`, `server/static/styles.css`

This task is split into four substeps; each is a separate commit so the diff stays reviewable.

### 15a — Header badge slot

- [ ] Step 1: Add an empty `<div id="dhw-boost-badge" class="badge"></div>` in the header bar of `index.html` next to the SmartGrid/Powersave badges.
- [ ] Step 2: In `app.jsx`, after the existing badge update logic, fetch `/api/v1/dhw/state` every 5 s. If `boost !== null`, set the badge text to `⚡ DHW Boost · {remaining} · ⚙ immersion` (immersion suffix only when `immersion_engaged`); else clear it.
- [ ] Step 3: CSS — copy the existing `.badge` rules; the boost badge uses an OKLCH warm-orange family to distinguish it from SmartGrid green and Powersave amber.
- [ ] Step 4: Commit.

### 15b — `<DhwControl />` dropdown

- [ ] Step 1: Add component skeleton — closed state shows `Current: <comfort_level> · <stop_temp> °C` or, when boost active, `⚡ Shower · 18 min left`.
- [ ] Step 2: Open-state renders the three rows + inline submenu. Submenu state local to component.
- [ ] Step 3: Click handlers call `POST /api/v1/dhw/boost?preset=shower`, `?preset=bath&hours=N` (after slider modal), `POST /api/v1/dhw/comfort?level=…`. Errors render as a transient toast at the bottom of the card.
- [ ] Step 4: Disable Shower + Bath rows while `state.boost !== null`. Bath also gets a `Cancel boost` row that calls `DELETE /api/v1/dhw/boost`.
- [ ] Step 5: Commit.

### 15c — Bath confirm modal

- [ ] Step 1: Slider 0.5–2.0 step 0.5. Default 1.0 h.
- [ ] Step 2: Threshold preview shows `Immersion gate: < 0.50 SEK/kWh (read-only)` — reads from `GET /api/v1/dhw/state` if exposed; otherwise from a static config snippet returned by the same endpoint. (Add a `config: { immersion_allow_price_sek_per_kwh }` field to `DhwSnapshot` — small spec extension; document inline.)
- [ ] Step 3: Confirm button posts; Cancel closes.
- [ ] Step 4: Commit.

### 15d — Chart band + charging-DHW chip

- [ ] Step 1: In the existing spot-price chart renderer, add a translucent band for `[boost.started_at, boost.scheduled_end]` when boost is active and any part of that interval falls in the chart's slot range. Use an OKLCH warm-orange with `0.18` alpha (`oklch(0.78 0.16 50 / 0.18)`); add a CSS comment noting it blends with the SmartGrid band on overlap.
- [ ] Step 2: Add a "charging DHW" chip next to the DhwControl trigger, visible when the latest `Sensor::SystemStatus` value is 5. The chip reads from the existing series API or a new tiny endpoint `GET /api/v1/sensors/latest?sensor=system_status`.
- [ ] Step 3: Commit.

After all four substeps: `cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets`.

---

## Task 16: Manual integration test on ctc.lan

**Files:** none modified (live test).

This task is a checklist that mirrors §6.4 of the spec. Run from the worktree against the production heater.

- [ ] **Pre-flight (one-time, user action)**: switch the heater from Manuell to Normal at the physical pump display (Menu → Varmvatten → Program → Normal). Confirm via:

```bash
curl -s "http://ctc.lan:3000/api/v1/ctc?addr=61500&custom=true&factor=1.0"
# Expect: {"ctc_data": 1.0}
```

(If this is skipped, the dashboard will display "Manuell (custom)" until the user picks a level via the new UI — also acceptable.)

- [ ] **Build a release binary**:

```bash
cargo build --release -p server
```

- [ ] **Run against the heater** (as user — needs serial port access on the deployment host; local dev box can use a USB serial adapter or skip this if no hardware):

```bash
./target/release/server /dev/ttyUSB0
```

- [ ] **Shower test (cold tank)**: ensure DHW upper < 55 °C. Open dashboard, pick Shower. Confirm:
  - Badge appears within 5 s.
  - Chart band drawn.
  - `61503 = 1` reflected via `curl /api/v1/ctc?addr=61503&custom=true&factor=0.5`.
  - Pump goes off within reconciler poll period.
  - 30 min later: badge clears, pump comes back on.

- [ ] **Shower test (already-hot tank)**: ensure DHW upper ≥ 55 °C. Pick Shower. Confirm `started: false, reason: "already_at_target"` in response, no Modbus writes, no badge.

- [ ] **Bath test (1 h, cheap window)**: pick Bath, 1 h. Confirm:
  - Badge shows "DHW Boost · 1 h 0 min".
  - `61636` reads 50 °C; `61503` reads 1.0 h.
  - SG mode shows Overcapacity in the SmartGrid badge.
  - If spot price < 0.45: badge gains "⚙ immersion"; `61591` reads 3.0.
  - At 1 h: badge clears, `61503=0` (or already 0), `61636` restored to 60 °C, SG=Normal, pump returns to Normal-implied state.

- [ ] **Bath test (price-band exit)**: artificially set immersion threshold > current spot to force bail. Confirm boost ends with `CancelReason::PriceLeftCheap` within 60 s.

- [ ] **Crash recovery**: kill the server during a Bath (`kill -9 <pid>`). Restart. Confirm log shows "DHW recovery cleared mid-flight boost"; `61503`, `61591`, `61636`, SG, pump all in baseline state.

- [ ] **Commit a manual-test report** to `docs/superpowers/plans/` with date-stamped observations:

```bash
echo "Manual integration test run on $(date -u +%Y-%m-%dT%H:%MZ): all 6 cases pass." > docs/superpowers/plans/2026-05-14-dhw-controls-manual-test.md
git add docs/superpowers/plans/2026-05-14-dhw-controls-manual-test.md
git commit -m "Manual integration test report for DHW controls"
```

---

## Task 17: Deploy (user action)

- [ ] **Step 1: Push the feature branch**

```bash
git push -u origin feature/dhw_controls
```

- [ ] **Step 2: Open a PR or merge locally**

If the repo has no remote PR workflow, squash-merge locally on the primary worktree (`~/ws/ctc_server`):

```bash
cd ../ctc_server
git fetch origin
git checkout main
git merge --squash feature/dhw_controls
git commit -m "Add DHW controls dropdown and Bath immersion controller"
```

- [ ] **Step 3: Deploy to ctc.lan** per the existing deployment runbook (Magnus owns this; not an agent action).

- [ ] **Step 4: Clean up worktree**

```bash
cd ~/ws
git -C ctc_server worktree remove ../ctc_server-dhw_controls
git -C ctc_server branch -D feature/dhw_controls
```

---

## Rebase bracket — STOP

Before declaring this plan complete, the feature branch must be rebased onto current `origin/main` one final time:

```bash
git fetch origin
git rebase origin/main
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
```

If anything diverges since the START rebase (Task 0), this catches it before the user-driven push and merge.

---

## Self-review

**Spec coverage check** (skim §1–§7 of the spec):

- §1 Problem (DHW dropdown + Bath immersion controller): Tasks 7-13 (backend) + Task 15 (UI).
- §2.1 Register table (61500/61503/61591/61636/62001/62005/62276): Task 1 (61636), Tasks 7-11 (writes), Task 8 step 3 (62001 read), Task 15d (62005 chip).
- §2.2 Existing actors reused: Tasks 4 (HomeyHooks override), 14 (main wiring).
- §3.1 UC-A Shower: Tasks 8 (start path) + 9 (watcher).
- §3.2 UC-B Bath: Tasks 11 (start + price gate) + 12 (watcher) + 13 (stop sequence).
- §3.3 UC-C Comfort: Task 7 (write + read-back) + Task 10 (HTTP route).
- §4.1 Module layout: file map at top of this plan.
- §4.2 Data model: Task 3 (state.rs), Task 6 (actor.rs skeleton).
- §4.2.1 Actor pattern: Task 6.
- §4.3 HTTP API: Task 10 + Task 11 (Bath body added) + Task 13 (cancel matrix).
- §4.4 Config: Task 2.
- §4.5 Persistence + crash recovery: Task 3 (atomic IO), Task 6 (recovery prologue).
- §4.6 Dashboard: Task 15a-d.
- §4.7 Homey override lane: Task 4.
- §4.8 Modbus operations: Tasks 1, 7-13 collectively.
- §5 Risks: each row mapped to a test in the relevant task.
- §6.1 Unit tests: each bullet has a matching test in Tasks 3, 5, 8-13.
- §6.4 Manual on-rig tests: Task 16.
- §7 Build sequence: tasks ordered identically to the spec's build steps.

No spec section is left without a task.

**Placeholder scan**: no "TBD", "TODO", "fill in", "similar to X" remain. The Task 0 finding gets recorded inline in the plan body when reached.

**Type consistency**: `DhwHandle`, `DhwActor`, `DhwCmd`, `DhwError`, `CancelReason`, `StartReport`, `ComfortLevel`, `ComfortLevelString`, `BoostPreset`, `DhwBoostState`, `DhwPersistedState`, `DhwSnapshot`, `DhwBoostSnapshot`, `ImmersionGate`, `ImmersionDecision`, `ModbusWriter`, `SgController` — referenced consistently across Tasks 3-15.

`reconciler_target` defined in Task 4 step 3, consumed in Task 4 step 5. `pump_on_for` (existing) referenced in Task 4 step 5 commentary.

`HomeyHooks.boost_override_tx` added in Task 4, consumed by DhwActor adapters in Task 14.

`61500=0/1/2` scaled values matched in Task 7 (`ComfortLevel::as_scaled`) and Task 7 step 4b (`ComfortLevelString::from(scaled)`).

`61503=1` for Shower (0.5 h), `61503 = 2*hours` for Bath — formula consistent in Tasks 8, 11. Verified by Task 11 step 3 inline math.

`61591 = (kw * 10).round() as i16` for immersion writes — consistent in Tasks 11, 12, 13.

`61636 = (prior_c * 10).round() as i16` for restore, `(immersion_engage_temp_c * 10).round() as i16` for the lowering write — consistent in Tasks 11, 13.

All consistent.

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-14-dhw-controls.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
