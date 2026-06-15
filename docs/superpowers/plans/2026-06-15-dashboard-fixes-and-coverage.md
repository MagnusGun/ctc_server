# Dashboard Fixes + Coverage Bump Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two dashboard display bugs (resume band not immediate; circ-pump shown On while heating off) and raise Rust test coverage best-effort toward ~86-88%, updating the CLAUDE.md gate to the realistic figure.

**Architecture:** Three independent workstreams on one feature branch. A & B are JS-only edits to `server/static/app.jsx` (no Rust impact, not measured by cargo coverage). C is Rust test additions plus a doc edit; it builds one reusable fake-Modbus-actor test helper, then fans tests across low-coverage route/logic modules and re-measures with `cargo llvm-cov`.

**Tech Stack:** Rust (Axum, tokio, tokio-modbus), `cargo-llvm-cov` for coverage (tarpaulin cannot build in this environment), React-via-CDN JSX dashboard (no JS test runner).

---

## Worktree (already created)

Work happens in the sibling worktree on a feature branch (never `main`):

```
/home/mbg/ws/ctc_server-dashboard_fixes   feature/dashboard_fixes   (based on local main 2d8a829)
```

If re-creating from scratch:
```bash
git worktree add ../ctc_server-dashboard_fixes -b feature/dashboard_fixes origin/main
cd ../ctc_server-dashboard_fixes && git rebase main   # local main is ahead of origin/main
```

## Rebase brackets

- **Before starting:** `git fetch origin && git rebase origin/main` (resolve against the latest base).
- **Before handing back:** `git fetch origin && git rebase origin/main` again, re-run the pre-commit checklist.

## Project rules pasted verbatim (do not rely on global load)

From `CLAUDE.md` — **Git Operation Policy**:

> **Hard rule: never edit or commit while HEAD is `main`.** ... create a sibling worktree on a `feature/<snake_case>` branch first.
> **Reserved for the user (Claude does not run these):** `git push` (any form), `git merge` / squash-merge into `main`, destructive history rewrites.

From `CLAUDE.md` — **Pre-Commit Checklist**:

> - [ ] `cargo fmt` - Code is formatted
> - [ ] `cargo clippy --all-targets -- -W clippy::pedantic` - Zero warnings
> - [ ] `cargo test --all-targets` - All tests pass
> - [ ] `cargo tarpaulin --all-targets --workspace` - Coverage ≥ 90%  *(see note below — use `cargo llvm-cov`; 90% is relaxed for this branch per the agreed best-effort decision)*
> - [ ] Commit message follows guidelines (≤50 chars, imperative verb)

From `CLAUDE.md` — **Float Comparisons in Tests**:

> Never use `assert_eq!` for float comparisons. Use an epsilon-based helper.

From global `~/.claude/CLAUDE.md` — **Commit Messages**: short one-liners, imperative, <72 (project says ≤50) chars, no body unless complex, no Claude attribution.

**Coverage gate note:** Per the user decision on 2026-06-15, the unconditional 90% gate is infeasible here — `main.rs` (507 missed lines, bootstrap) and `modbus/actor.rs` (736 missed lines, serial loop) are I/O wiring that can't be unit-tested without hardware mocking, and together they are 41% of all missed lines. Target is best-effort ~86-88% on testable modules; Task C9 updates the CLAUDE.md gate text accordingly.

---

# Workstream A — Resume band must appear immediately (#2)

**Problem:** After applying a SmartGrid block, the spot-price chart's scheduled-run band only appears on the next 5s `useSmartGrid` poll, because the apply handler POSTs and sets local `mode` but never refreshes `sgResp` (which feeds `scheduledResumeAt` / `run_minutes` into `EnergyChart`). The user must wait (or manually refresh).

**Fix:** After a successful `setSmartGridMode` POST, call `sgMeta.refetch()` so the new `scheduled_resume_at` lands immediately. `refetch` is already exposed by `usePolledFetch` meta (used at `app.jsx:1696` for `dhwMeta`).

### Task A1: Refetch SmartGrid state after apply

**Files:**
- Modify: `server/static/app.jsx` (enable apply handler ~1647-1662 and disable/return-to-normal handler ~1673-1681)

- [ ] **Step 1: Refetch after the Enable apply POST**

In the "Enable {meta.label}" button handler, after `setMode(target);`:

```jsx
                              try {
                                await window.api.setSmartGridMode(backendMode, schedule, resumeAt);
                                setMode(target);
                                sgMeta?.refetch?.();   // pull new scheduled_resume_at now, not on next 5s poll
                              } catch (e) {
                                console.error("SmartGrid POST failed:", e);
                              }
```

- [ ] **Step 2: Refetch after the Return-to-Normal POST**

In the "Return to Normal" button handler, after `setMode("normal");`:

```jsx
                              try {
                                await window.api.setSmartGridMode("normal", false);
                                setMode("normal");
                                sgMeta?.refetch?.();   // clear the band immediately
                              } catch (e) {
                                console.error("SmartGrid POST failed:", e);
                              }
```

- [ ] **Step 3: Confirm `sgMeta` is in scope**

`sgMeta` is destructured at `app.jsx:508` (`const [sgResp, sgMeta] = useSmartGrid();`) inside `App()`, and the confirm dialog JSX is rendered inside `App()`, so it is in scope. No new state needed.

- [ ] **Step 4: Verify (manual — no JS test runner)**

Serve the dashboard locally (read-only diagnosis box; production is ctc.lan) and confirm via Playwright/manual: clicking Block then OK makes the translucent band appear on the chart within one request round-trip, with no manual page refresh. Returning to Normal clears it immediately.

- [ ] **Step 5: Commit**

```bash
git add server/static/app.jsx
git commit -m "Refresh SmartGrid state after apply so resume band shows now"
```

---

# Workstream B — Circ pump must show Off when heating is off (#3)

**Problem:** The circ-pump badge (`app.jsx:941-962`) renders purely from the Homey smart-plug reading (`pumpResp.on`). When heating is off (outdoor above the heat-off threshold), the heater relay cuts power to the pump; the Homey plug then loses power and reports `stale`, but the badge still shows text "On" (only the colour changes to "warn"). The pump cannot physically run with no power.

**Fix:** Cross-check `heating.status`. `HEATING_STATUS` register 62246: `0 = Off`, `3 = On` (see `HEATING_STATUS_CLASS` / `whyHeating` at `app.jsx:68-95`). When `heating?.status === 0`, the heater has cut pump power, so show the pump as **Off** with an explanatory tip, regardless of the (stale) plug reading.

### Task B1: Cross-check heating state in the pump badge

**Files:**
- Modify: `server/static/app.jsx:941-962` (pump badge IIFE)

- [ ] **Step 1: Derive heating-off and override the displayed value**

Replace the body of the pump-badge IIFE (`app.jsx:944-960`) with:

```jsx
                    // Circulation pump state from the Homey integration. Hidden
                    // when /pump returns 503 (Homey integration disabled).
                    // When the heating system is off (status 0), the heater
                    // relay cuts power to the pump, so it cannot run regardless
                    // of the last (now stale) plug reading — show Off.
                    const heatingOff = heating?.status === 0;
                    const on = heatingOff ? false : pumpResp.on;
                    const stale = !!pumpResp.stale;
                    const valueLabel = on == null ? "?" : (on ? "On" : "Off");
                    const stamp = pumpResp.last_observed_unix_secs;
                    const ageText = stamp == null
                      ? "not yet observed"
                      : `updated ${Math.max(0, Math.floor(Date.now() / 1000 - stamp))} s ago`;
                    const tip = heatingOff
                      ? "Circulation Pump · Off — heating is off, heater cut pump power"
                      : stale
                        ? `Circulation Pump · Homey unreachable (${ageText})`
                        : `Circulation Pump · ${ageText}`;
                    const cls = heatingOff ? "off" : stale ? "warn" : on ? "on" : "off";
```

Leave the returned JSX (the `<div className={\`status ${cls}\`} title={tip}>` block) unchanged.

- [ ] **Step 2: Confirm `heating` is in scope**

`heating` is destructured at `app.jsx:497` (`const [heating] = useHeatingSystem();`) and already used for the Status badge two rows above (`heating?.status` at line 923/934), so it is in scope.

- [ ] **Step 3: Verify (manual)**

Confirm: when `heating.status === 0` (warm outdoor, heating off) the pump badge reads "Off" with the heating-off tip; when `status === 3` (heating on) it reflects `pumpResp.on` and stale handling as before.

- [ ] **Step 4: Commit**

```bash
git add server/static/app.jsx
git commit -m "Show circ pump Off when heating system is off"
```

---

# Workstream C — Best-effort coverage bump + gate update (#1)

**Baseline (2026-06-15, `cargo llvm-cov --workspace --summary-only`):** 78.11% lines, 80.20% regions, 475 tests passing.

**Target:** ~86-88% lines. Do **not** attempt `main.rs` or `modbus/actor.rs` serial loop (hardware I/O). Attack testable route handlers and pure-logic modules. Re-measure after each task; stop when the total reaches ~86-88%.

**Module priority (missed lines / current line cover) — testable subset:**

| Module | Missed | Cover | Notes |
|---|---|---|---|
| `routes/alarms.rs` | 174 | 71% | bitmask scan + text parse; already has inline fake-actor tests |
| `routes/ctc.rs` | 118 | 52% | read/write handlers need a responding fake actor |
| `routes/temperatures.rs` | 98 | 37% | read handlers need a responding fake actor |
| `smartgrid/actor.rs` | 107 | 82% | resume-target logic, pure |
| `routes/smartgrid.rs` | 87 | 79% | handlers |
| `energy/grid.rs` | 76 | 84% | peak tracking, pure |
| `modbus/operations.rs` | 73 | 82% | operation/response types, pure |
| `energy/elpris.rs` | 68 | 44% | price-API JSON parsing, pure |
| `storage/mod.rs` | 66 | 84% | redb store paths |
| `routes/visibility.rs` | 56 | 85% | bitmask reads |
| `dhw/error.rs` | 52 | 40% | error Display/From, trivial |
| `modbus/mod.rs` | 17 | 93% | scaling/validation edges |

**Keystone:** Many route gaps are *success paths* that need an actor that answers `ParameterOperation` requests with canned values. The existing pattern (see `routes/alarms.rs:817+`) spawns a task owning `rx` per-test. Task C1 extracts one reusable helper so subsequent tasks are short.

### Task C1: Reusable fake-Modbus-actor test helper

**Files:**
- Create: `server/src/modbus/test_support.rs`
- Modify: `server/src/modbus/mod.rs` (add `#[cfg(test)] pub mod test_support;`)

- [ ] **Step 1: Write the helper**

```rust
//! Test-only fake Modbus actor: answers ParameterOperation requests from a
//! canned map of register -> raw i16, so route handlers can exercise their
//! success paths without a serial port.
use crate::modbus::ModbusSender;
use crate::modbus::actor::ModbusRequest;
use crate::modbus::operations::ParameterOperation;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Spawn a fake actor. `reads` maps register address -> raw i16 value returned
/// for Read ops. Write ops echo the written value back. Unknown reads return 0.
pub fn spawn_fake_actor(reads: HashMap<u16, i16>) -> ModbusSender {
    let (tx, mut rx) = mpsc::channel::<ModbusRequest>(8);
    tokio::spawn(async move {
        while let Some((op, respond_to)) = rx.recv().await {
            let scaled = match &op {
                ParameterOperation::Read(p) => {
                    let raw = reads.get(&p.register).copied().unwrap_or(0);
                    f64::from(raw) * f64::from(p.scaling)
                }
                ParameterOperation::Write(p, v) => {
                    let _ = p;
                    *v
                }
            };
            let _ = respond_to.send(Ok(scaled));
        }
    });
    tx
}
```

> Adjust field names (`register`, `scaling`) and the `ResponseChannel` Ok type to match the real `CTCModbusParameter` / `ParameterOperation` definitions — read `server/src/modbus/mod.rs` and `operations.rs` first and mirror exactly. If `ParameterOperation` has more arms (e.g. min/max reads), handle them too.

- [ ] **Step 2: Wire the module**

In `server/src/modbus/mod.rs` add near the other `mod` decls:

```rust
#[cfg(test)]
pub mod test_support;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo test -p server --no-run`
Expected: builds with no errors.

- [ ] **Step 4: Commit**

```bash
git add server/src/modbus/test_support.rs server/src/modbus/mod.rs
git commit -m "Add fake Modbus actor test helper"
```

### Task C2: Cover `routes/temperatures.rs` success paths

**Files:**
- Modify: `server/src/routes/temperatures.rs` (add/extend `#[cfg(test)] mod tests`)

- [ ] **Step 1: Identify uncovered handlers**

Run: `cargo llvm-cov --workspace --summary-only` then for line detail on this file:
`cargo llvm-cov report --html` and open `target/llvm-cov/html/index.html`, or
`cargo llvm-cov --workspace -- routes::temperatures 2>/dev/null` to list. Note each handler with missed lines (read room/outdoor/flow, get/set setpoint).

- [ ] **Step 2: Write success-path tests using the fake actor**

For each read handler, build state with `spawn_fake_actor(HashMap::from([(REG, raw)]))`, call the handler, assert the JSON body contains the scaled value. Example shape:

```rust
#[tokio::test]
async fn get_outdoor_returns_scaled_value() {
    let sender = crate::modbus::test_support::spawn_fake_actor(
        std::collections::HashMap::from([(/* outdoor reg */ 0u16, 123i16)]),
    );
    let state = /* construct the route State with `sender` */;
    let body = get_outdoor_temperature(State(state)).await.expect("ok");
    assert!(body.contains("12.3"));   // 123 * 0.1
}
```

> Mirror the real `State` struct fields and handler names from the top of `temperatures.rs`. Add error-path tests with a dropped-channel sender for the `ServiceUnavailable` branch.

- [ ] **Step 3: Run + re-measure**

Run: `cargo test -p server routes::temperatures -- --nocapture` (PASS), then
`cargo llvm-cov --workspace --summary-only` and confirm `temperatures.rs` line cover rose toward ~85%+.

- [ ] **Step 4: Commit**

```bash
git add server/src/routes/temperatures.rs
git commit -m "Cover temperature route success paths"
```

### Task C3: Cover `routes/ctc.rs` read/write handlers

**Files:**
- Modify: `server/src/routes/ctc.rs` tests (extend existing `mod tests`)

- [ ] **Step 1:** Use `spawn_fake_actor` to test generic parameter read/write success (currently only error paths via `dummy_modbus_sender` are covered). Assert read returns scaled JSON and write echoes/validates.
- [ ] **Step 2:** Run `cargo test -p server routes::ctc` (PASS), re-measure.
- [ ] **Step 3:** Commit: `git commit -am "Cover ctc parameter read/write handlers"`

### Task C4: Cover `energy/elpris.rs` price-API parsing

**Files:**
- Modify: `server/src/energy/elpris.rs` tests

- [ ] **Step 1:** Add unit tests for the JSON deserialization + price-mapping functions with representative sample payloads (success, empty, malformed). These are pure functions — no I/O — so no actor needed. 44% → aim 85%+.
- [ ] **Step 2:** Run `cargo test -p server energy::elpris` (PASS), re-measure.
- [ ] **Step 3:** Commit: `git commit -am "Cover elpris price parsing"`

### Task C5: Cover `dhw/error.rs` and `modbus/operations.rs`

**Files:**
- Modify: `server/src/dhw/error.rs` tests, `server/src/modbus/operations.rs` tests

- [ ] **Step 1:** `dhw/error.rs` (40%): test each `Display` arm and `From`/`?` conversion — trivial, high yield per line.
- [ ] **Step 2:** `modbus/operations.rs` (82%): test the remaining response-handling / operation-construction branches.
- [ ] **Step 3:** Run targeted tests (PASS), re-measure.
- [ ] **Step 4:** Commit: `git commit -am "Cover dhw error display and modbus operations"`

### Task C6: Cover `routes/alarms.rs` and `routes/visibility.rs`

**Files:**
- Modify: `server/src/routes/alarms.rs` tests, `server/src/routes/visibility.rs` tests

- [ ] **Step 1:** alarms (71%): extend the existing inline fake-actor tests to cover untested bitmask-scan / text-buffer / caching branches.
- [ ] **Step 2:** visibility (85%): cover remaining bitmask-decode edges.
- [ ] **Step 3:** Run targeted tests (PASS), re-measure.
- [ ] **Step 4:** Commit: `git commit -am "Cover alarm scan and visibility decode branches"`

### Task C7: Cover `smartgrid/actor.rs`, `routes/smartgrid.rs`, `energy/grid.rs`, `storage/mod.rs`

**Files:**
- Modify: tests in each of the four modules

- [ ] **Step 1:** Add tests for the remaining uncovered branches in each (resume-target dispatch edges, handler error mappings, peak-tracking edge cases, store open/migrate/prune edges). All have existing test modules to extend.
- [ ] **Step 2:** Run targeted tests (PASS), re-measure after each.
- [ ] **Step 3:** Commit per module or grouped: `git commit -am "Cover smartgrid/grid/storage branches"`

### Task C8: Measure total and decide stop

- [ ] **Step 1:** Run: `cargo llvm-cov --workspace --summary-only`
- [ ] **Step 2:** If TOTAL line cover is in ~86-88%, stop. If short, return to the highest-missed testable module from the table and add more. Do **not** chase `main.rs` / `modbus/actor.rs` serial loop.
- [ ] **Step 3:** Record the final number for the commit message and the gate edit.

### Task C9: Update the CLAUDE.md coverage gate to reality

**Files:**
- Modify: `CLAUDE.md` (Code Coverage section ~"Minimum 90%" and the Pre-Commit Checklist line)

- [ ] **Step 1:** Change the coverage requirement from the unconditional `≥90%` / tarpaulin instruction to reflect reality: the tool is `cargo-llvm-cov` (tarpaulin can't build here), and the gate is "≥85% on testable code; `main.rs` bootstrap and the `modbus/actor.rs` serial loop are exempt as untestable I/O wiring." Use the actual final number from C8.

Example replacement for the coverage block:

```markdown
3. **Code Coverage** (≥85% on testable code)
   ```bash
   cargo llvm-cov --workspace --summary-only
   ```
   - Tool is `cargo-llvm-cov` (cargo-tarpaulin cannot build in this environment).
   - Minimum ~85% line coverage across testable modules.
   - Exempt as untestable I/O wiring: `main.rs` (bootstrap) and the
     `modbus/actor.rs` serial loop. Do not game the metric to hit a number.
   - HTML report: `cargo llvm-cov --workspace --html` → `target/llvm-cov/html/index.html`.
```

Also update the Pre-Commit Checklist line `cargo tarpaulin ... Coverage ≥ 90%` to `cargo llvm-cov --workspace --summary-only - Coverage ≥ 85% (testable)`.

- [ ] **Step 2:** Commit: `git commit -m "Align coverage gate with cargo-llvm-cov reality"`

---

## Final verification (before handoff)

- [ ] `cargo fmt`
- [ ] `cargo clippy --all-targets -- -W clippy::pedantic` → zero warnings
- [ ] `cargo test --all-targets` → all pass
- [ ] `cargo llvm-cov --workspace --summary-only` → ~86-88% (record number)
- [ ] `git fetch origin && git rebase origin/main`
- [ ] Hand back; user owns push + merge to main.

## Self-review notes

- A & B are not covered by cargo coverage (JS); verified manually/Playwright only — explicitly flagged, not a silent gap.
- C1 helper field/type names must be reconciled with the real `ParameterOperation` / `CTCModbusParameter` before C2-C7 compile — called out in C1 Step 1.
- 90% is intentionally relaxed to best-effort per the 2026-06-15 user decision; C9 records this in CLAUDE.md so the next contributor isn't misled.
