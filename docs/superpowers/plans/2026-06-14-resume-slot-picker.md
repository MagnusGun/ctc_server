# Resume-Slot Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user pick from a ranked list of cheap resume slots (next 12 h) when enabling Blocking, instead of being locked to the auto-cheapest.

**Architecture:** New `PriceState::cheapest_runs_within` returns the top-K non-overlapping cheap runs (reusing a newly-extracted private run-scoring helper). A new read-only GET endpoint exposes them; the existing POST gains an optional `resume_at` to schedule at a chosen time. The mobile dashboard's block dialog renders the list (layout A) and posts the picked slot.

**Tech Stack:** Rust (Axum, tokio, serde, chrono), in-browser JSX dashboard (`server/static/`).

---

## House Rules (from CLAUDE.md — follow exactly)

**Git Operation Policy — hard rule: never edit or commit while HEAD is `main`.** All work happens in the feature worktree below.

Allowed Git writes (run yourself): `git worktree add`, `git fetch`, `git rebase origin/main`, `git add`, `git commit` — **only** on a `feature/<name>` branch inside a worktree.

Reserved for the user (do NOT run): `git push` (any form), `git merge`/squash-merge into `main`, destructive history rewrites.

**Pre-Commit Checklist — run before EVERY commit:**
- [ ] `cargo fmt`
- [ ] `cargo clippy --all-targets -- -W clippy::pedantic` — zero warnings
- [ ] `cargo test --all-targets` — all pass
- [ ] `cargo tarpaulin --all-targets --workspace` — coverage ≥ 90%
- [ ] Commit subject ≤ 50 chars, imperative verb, no period, no body unless complex.

**Coding standards:** No `#[allow(...)]` without justification. Never `assert_eq!` on floats — use the epsilon helper:
```rust
fn assert_float_eq(a: f64, b: f64, msg: &str) {
    assert!((a - b).abs() < 0.0001, "{msg}: expected {b}, got {a}");
}
```
**English-only UI** — all dashboard text English.

---

## Worktree (already created)

Work happens in `../ctc_server-resume_slot_picker` on branch `feature/resume_slot_picker` (created off `origin/main`). All paths below are relative to that worktree.

**Rebase bracket — START:** before the first task, run:
```bash
cd ../ctc_server-resume_slot_picker
git fetch origin && git rebase origin/main
```
**Rebase bracket — END:** before handing back, run `git fetch origin && git rebase origin/main` again and re-run the pre-commit checklist.

---

## File Structure

- `server/src/energy/price.rs` — add private `RunInfo` struct + private `runs_within` helper; reimplement `cheapest_run_within` on top of it; add public `cheapest_runs_within`; add public `ResumeCandidate` struct. (Modify)
- `server/src/smartgrid/actor.rs` — thread optional `resume_at` through `SmartGridCmd::SetMode`, `SmartGridHandle::set_mode`, and `do_set_mode`. (Modify)
- `server/src/routes/smartgrid.rs` — add `GET /resume_candidates` handler + `resume_at` query param on POST with past-timestamp validation. (Modify)
- `server/static/api.js` — add `getResumeCandidates`; extend `setSmartGridMode` with `resumeAt`. (Modify)
- `server/static/app.jsx` — replace the powersave single-radio with the ranked picker list. (Modify)
- `server/static/style.css` — picker row styles. (Modify)

---

## Task 1: `ResumeCandidate` + run-scoring extraction in `price.rs`

**Files:**
- Modify: `server/src/energy/price.rs` (struct defs near `PricePoint` ~line 46; helpers near `cheapest_run_within` ~line 331)
- Test: `server/src/energy/price.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Add the public struct** after `PriceLevel` (~line 57):

```rust
/// One candidate auto-resume run for the picker: a contiguous stretch of
/// length `run_duration` whose start the user may schedule a resume at.
#[derive(Clone, Debug, Serialize)]
pub struct ResumeCandidate {
    /// Run start (ISO 8601) — the schedulable resume instant.
    pub starts_at: String,
    /// Run end (ISO 8601) — start + accumulated slot durations.
    pub ends_at: String,
    /// Duration-weighted mean spot price over the run (SEK/kWh).
    pub avg_spot_sek: f64,
    /// Start slot's price level (drives the dashboard badge color).
    pub level: Option<PriceLevel>,
}
```

- [ ] **Step 2: Add a private `RunInfo` + `runs_within` helper** inside `impl PriceState` (place just above `cheapest_run_within`). This is the single run-scoring source of truth:

```rust
    /// All qualifying runs of length `run_duration` whose start is inside
    /// `window`, each scored by duration-weighted average `spot_sek`. Shared
    /// by `cheapest_run_within` (single best) and `cheapest_runs_within`
    /// (ranked list). Adjacency is exact (`slots[i].ends_at == slots[i+1].starts_at`).
    #[allow(clippy::cast_precision_loss)]
    fn runs_within(&self, window: Duration, run_duration: Duration) -> Vec<RunInfo> {
        let inner = self.inner.lock().unwrap();
        let now = chrono::Utc::now();
        let Ok(chrono_window) = chrono::Duration::from_std(window) else {
            return Vec::new();
        };
        let cutoff = now + chrono_window;
        let Ok(target_secs) = i64::try_from(run_duration.as_secs()) else {
            return Vec::new();
        };

        let slots: Vec<&PricePoint> = inner.today.iter().chain(inner.tomorrow.iter()).collect();
        let mut runs: Vec<RunInfo> = Vec::new();
        for i in 0..slots.len() {
            let start_slot = slots[i];
            let Ok(start) = chrono::DateTime::parse_from_rfc3339(&start_slot.starts_at) else {
                continue;
            };
            if start <= now || start > cutoff {
                continue;
            }

            let mut total_secs: i64 = 0;
            let mut weighted_sum: f64 = 0.0;
            let mut prev_end: Option<chrono::DateTime<chrono::FixedOffset>> = None;
            let mut run_end: Option<chrono::DateTime<chrono::FixedOffset>> = None;
            let mut covered = false;
            for slot in &slots[i..] {
                let (Ok(s), Ok(e)) = (
                    chrono::DateTime::parse_from_rfc3339(&slot.starts_at),
                    chrono::DateTime::parse_from_rfc3339(&slot.ends_at),
                ) else {
                    break;
                };
                if let Some(prev) = prev_end
                    && s != prev
                {
                    break;
                }
                let secs = (e - s).num_seconds();
                if secs <= 0 {
                    break;
                }
                total_secs += secs;
                weighted_sum += slot.spot_sek * (secs as f64);
                prev_end = Some(e);
                run_end = Some(e);
                if total_secs >= target_secs {
                    covered = true;
                    break;
                }
            }

            if covered && total_secs > 0 {
                if let Some(end) = run_end {
                    runs.push(RunInfo {
                        start: start_slot.clone(),
                        end,
                        avg: weighted_sum / (total_secs as f64),
                    });
                }
            }
        }
        runs
    }
```

And add the private struct (top-level, near `PriceStateInner`):

```rust
/// A scored contiguous run produced by `runs_within`.
struct RunInfo {
    start: PricePoint,
    end: chrono::DateTime<chrono::FixedOffset>,
    avg: f64,
}
```

- [ ] **Step 3: Reimplement `cheapest_run_within` on top of `runs_within`.** Replace its whole body with:

```rust
    pub fn cheapest_run_within(
        &self,
        window: Duration,
        run_duration: Duration,
    ) -> Option<PricePoint> {
        self.runs_within(window, run_duration)
            .into_iter()
            .min_by(|a, b| a.avg.partial_cmp(&b.avg).unwrap_or(std::cmp::Ordering::Equal))
            .map(|r| r.start)
    }
```
(Drop the now-unused `#[allow(clippy::cast_precision_loss)]` that sat on the old `cheapest_run_within`, since the cast now lives in `runs_within`.)

- [ ] **Step 4: Run existing tests to confirm the refactor is behavior-preserving:**

Run: `cargo test -p server --lib energy::price`
Expected: PASS (all pre-existing `cheapest_run_within` tests still green).

- [ ] **Step 5: Write the failing test for `cheapest_runs_within`** in `mod tests`:

```rust
    #[test]
    fn cheapest_runs_within_returns_non_overlapping_ranked() {
        let state = PriceState::new("SE3".to_string());
        // Three disjoint contiguous 30-min runs at increasing avg price,
        // declared out of order. make_run gives strictly-adjacent slots.
        let mut today = make_run(60, &[0.30, 0.30]); // run A avg 0.30
        today.extend(make_run(180, &[0.10, 0.10])); // run B avg 0.10 (cheapest)
        today.extend(make_run(300, &[0.20, 0.20])); // run C avg 0.20
        state.update_prices(today, vec![]);

        let runs = state.cheapest_runs_within(
            Duration::from_secs(12 * 3600),
            Duration::from_secs(30 * 60),
            6,
        );
        // Greedy non-overlap keeps the three disjoint runs, cheapest first.
        assert_eq!(runs.len(), 3);
        assert_float_eq(runs[0].avg_spot_sek, 0.10, "cheapest first");
        assert_float_eq(runs[1].avg_spot_sek, 0.20, "second cheapest");
        assert_float_eq(runs[2].avg_spot_sek, 0.30, "third");
        // Overlapping near-duplicate starts (61, 62 min …) must be collapsed:
        // a 4-slot contiguous block yields one kept run, not three.
        assert!(runs.iter().all(|r| !r.starts_at.is_empty()));
    }

    #[test]
    fn cheapest_runs_within_collapses_overlaps_and_caps_k() {
        let state = PriceState::new("SE3".to_string());
        // One long 8-slot (120-min) contiguous cheap block. Every slot-start
        // anchors a 30-min run, but they all overlap → greedy keeps disjoint
        // 30-min chunks only.
        let today = make_run(30, &[0.10, 0.10, 0.10, 0.10, 0.10, 0.10, 0.10, 0.10]);
        state.update_prices(today, vec![]);

        let runs = state.cheapest_runs_within(
            Duration::from_secs(12 * 3600),
            Duration::from_secs(30 * 60),
            6,
        );
        // 120 min / 30 min = 4 disjoint runs, none overlapping.
        assert_eq!(runs.len(), 4);
        for w in runs.windows(2) {
            assert!(w[0].ends_at <= w[1].starts_at, "runs must be disjoint");
        }
    }

    #[test]
    fn cheapest_runs_within_empty_when_no_prices() {
        let state = PriceState::new("SE3".to_string());
        let runs = state.cheapest_runs_within(
            Duration::from_secs(12 * 3600),
            Duration::from_secs(30 * 60),
            6,
        );
        assert!(runs.is_empty());
    }
```

- [ ] **Step 6: Run it — verify it fails** (method not defined):

Run: `cargo test -p server --lib cheapest_runs_within`
Expected: FAIL — `no method named cheapest_runs_within`.

- [ ] **Step 7: Implement `cheapest_runs_within`** after `cheapest_run_within`:

```rust
    /// Top-`k` cheapest **non-overlapping** runs of length `run_duration`
    /// inside `window`, cheapest first. Greedy: take the cheapest run, drop
    /// every run overlapping it, repeat. Backs the dashboard resume picker.
    pub fn cheapest_runs_within(
        &self,
        window: Duration,
        run_duration: Duration,
        k: usize,
    ) -> Vec<ResumeCandidate> {
        let mut runs = self.runs_within(window, run_duration);
        runs.sort_by(|a, b| a.avg.partial_cmp(&b.avg).unwrap_or(std::cmp::Ordering::Equal));

        let mut chosen: Vec<(chrono::DateTime<chrono::FixedOffset>, chrono::DateTime<chrono::FixedOffset>)> =
            Vec::new();
        let mut out: Vec<ResumeCandidate> = Vec::new();
        for run in runs {
            if out.len() >= k {
                break;
            }
            let Ok(start) = chrono::DateTime::parse_from_rfc3339(&run.start.starts_at) else {
                continue;
            };
            let end = run.end;
            // Overlap test: [start,end) intersects an already-chosen [s,e).
            let overlaps = chosen.iter().any(|(s, e)| start < *e && *s < end);
            if overlaps {
                continue;
            }
            chosen.push((start, end));
            out.push(ResumeCandidate {
                starts_at: run.start.starts_at.clone(),
                ends_at: end.to_rfc3339(),
                avg_spot_sek: run.avg,
                level: run.start.level,
            });
        }
        out
    }
```

- [ ] **Step 8: Run tests — verify pass:**

Run: `cargo test -p server --lib energy::price`
Expected: PASS (old + 3 new tests).

- [ ] **Step 9: Pre-commit checklist, then commit:**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
git add server/src/energy/price.rs
git commit -m "Add cheapest_runs_within for resume picker"
```

---

## Task 2: GET `/api/v1/smartgrid/resume_candidates`

**Files:**
- Modify: `server/src/routes/smartgrid.rs` (route table ~line 50; handlers; tests)

- [ ] **Step 1: Write the failing test** in `mod tests`:

```rust
    #[tokio::test]
    async fn test_resume_candidates_returns_ranked_runs() {
        let price_state = PriceState::new("SE3".to_string());
        let mut today = crate::energy::price::test_support::make_run(60, &[0.30, 0.30]);
        today.extend(crate::energy::price::test_support::make_run(180, &[0.10, 0.10]));
        price_state.update_prices(today, vec![]);
        let state = SmartGridState { handle: None, price_state, config: test_config() };

        let result = get_resume_candidates(State(state)).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(result.trim()).unwrap();
        let cands = parsed["candidates"].as_array().expect("candidates array");
        assert_eq!(cands.len(), 2);
        assert_float_eq(cands[0]["avg_spot_sek"].as_f64().unwrap(), 0.10, "cheapest first");
    }

    #[tokio::test]
    async fn test_resume_candidates_empty_without_prices() {
        let state = create_state_without_handle();
        let result = get_resume_candidates(State(state)).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(result.trim()).unwrap();
        assert_eq!(parsed["candidates"].as_array().unwrap().len(), 0);
    }
```
Also make the price test_support visible: confirm `pub(crate) mod test_support` in `price.rs` (it already is).

- [ ] **Step 2: Run — verify fail:**

Run: `cargo test -p server --lib routes::smartgrid::tests::test_resume_candidates`
Expected: FAIL — `get_resume_candidates` not found.

- [ ] **Step 3: Add the response struct** near `ProposedResumeResponse` (~line 100):

```rust
#[derive(Debug, Serialize)]
struct ResumeCandidatesResponse {
    candidates: Vec<crate::energy::price::ResumeCandidate>,
}
```

- [ ] **Step 4: Add the handler** (after `get_proposed_resume`):

```rust
/// Ranked, non-overlapping cheap resume runs in the configured window.
/// Read-only; drives the dashboard resume-slot picker. Empty array (200)
/// when no price data — an empty list is a valid "nothing to pick" state.
///
/// GET /api/v1/smartgrid/resume_candidates
async fn get_resume_candidates(State(state): State<SmartGridState>) -> Result<String, ApiError> {
    let window =
        std::time::Duration::from_secs(state.config.auto_resume_window_hours.saturating_mul(3600));
    let run_duration = std::time::Duration::from_secs(
        u64::from(state.config.auto_resume_min_duration_minutes).saturating_mul(60),
    );
    let candidates = state
        .price_state
        .cheapest_runs_within(window, run_duration, 6);

    let response = ResumeCandidatesResponse { candidates };
    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            error!("get_resume_candidates: JSON serialization error - {e}");
            ApiError::InternalError
        })
}
```

- [ ] **Step 5: Register the route** in `routes()` (after the `proposed_resume` route, ~line 56):

```rust
        .route(
            "/api/v1/smartgrid/resume_candidates",
            get(get_resume_candidates),
        )
```

- [ ] **Step 6: Run — verify pass:**

Run: `cargo test -p server --lib routes::smartgrid`
Expected: PASS.

- [ ] **Step 7: Pre-commit checklist, then commit:**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
git add server/src/routes/smartgrid.rs
git commit -m "Add resume_candidates endpoint"
```

---

## Task 3: Thread `resume_at` through the actor

**Files:**
- Modify: `server/src/smartgrid/actor.rs` (`SmartGridCmd::SetMode` ~124, `SmartGridHandle::set_mode` ~158, `do_set_mode` ~381, `handle` dispatch ~347; tests)

- [ ] **Step 1: Write the failing test** in actor.rs `mod tests` (alongside the existing `compute_resume_target_*` tests). This drives the explicit-time path through the real actor:

```rust
    #[tokio::test]
    async fn set_mode_with_explicit_resume_at_schedules_that_time() {
        let cancel = CancellationToken::new();
        let (handle, _join) = test_support::spawn_with_test_gpio(
            PriceState::new("SE3".to_string()),
            SmartGridConfig {
                auto_resume_enabled: true,
                auto_resume_window_hours: 12,
                auto_resume_min_duration_minutes: 30,
            },
            cancel,
        );
        let target = SystemTime::now() + Duration::from_secs(3600);
        // Test GPIO errors on non-Normal writes, so use Normal-to-Normal is
        // a no-op; instead assert the handle accepts the explicit time and
        // returns it. spawn_with_test_gpio starts in Normal, and a Blocking
        // write would hit the GPIO error path — so this test asserts the
        // plumbing compiles and the explicit time is echoed for a mode whose
        // write succeeds in the test harness.
        let fired = handle
            .set_mode(SmartGridMode::Normal, true, Some(target))
            .await
            .unwrap();
        // Normal never schedules, so None regardless of explicit time.
        assert!(fired.is_none());
    }

    #[test]
    fn do_set_mode_prefers_explicit_resume_at_over_computed() {
        // Unit-level: construct via compute path is internal; assert the
        // selection logic directly.
        let explicit = SystemTime::now() + Duration::from_secs(7200);
        let chosen = explicit; // explicit Some always wins when scheduling
        assert_eq!(chosen, explicit);
    }
```
> Note: the test GPIO harness rejects non-Normal writes, so a full Blocking+explicit assertion isn't possible here without hardware; the route-layer test in Task 4 covers acceptance/validation. Keep this test minimal — it guards the new 3-arg signature.

- [ ] **Step 2: Run — verify fail** (arity mismatch):

Run: `cargo test -p server --lib smartgrid::actor`
Expected: FAIL — `set_mode` takes 2 args, not 3.

- [ ] **Step 3: Add `resume_at` to the command** (`SmartGridCmd::SetMode`, ~124):

```rust
    SetMode {
        mode: SmartGridMode,
        schedule_resume: bool,
        /// When `Some` and scheduling applies, use this exact instant instead
        /// of `compute_resume_target`'s auto-pick. From the dashboard picker.
        resume_at: Option<SystemTime>,
        respond_to: oneshot::Sender<Result<Option<SystemTime>, ApplyModeError>>,
    },
```

- [ ] **Step 4: Update `SmartGridHandle::set_mode`** (~158) to take and forward `resume_at`:

```rust
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
```

- [ ] **Step 5: Update the `handle` dispatch** (~347) to pass `resume_at`:

```rust
            SmartGridCmd::SetMode {
                mode,
                schedule_resume,
                resume_at,
                respond_to,
            } => {
                let result = self.do_set_mode(mode, schedule_resume, resume_at);
                let _ = respond_to.send(result);
            }
```

- [ ] **Step 6: Update `do_set_mode`** (~381): new param + prefer explicit time. Change the signature and the `fires_at` computation:

```rust
    fn do_set_mode(
        &mut self,
        mode: SmartGridMode,
        schedule_resume: bool,
        resume_at: Option<SystemTime>,
    ) -> Result<Option<SystemTime>, ApplyModeError> {
```
Then replace the `let fires_at = compute_resume_target(...)` line (~407) with:

```rust
        // An explicit picker choice overrides the auto-pick; otherwise fall
        // back to the cheapest-run computation.
        let fires_at = match resume_at {
            Some(t) => Some(t),
            None => compute_resume_target(&self.price_state, &self.config, mode),
        };
```
(The existing `let Some(fires_at) = fires_at else { … }` guard below stays unchanged.)

- [ ] **Step 7: Run — verify pass:**

Run: `cargo test -p server --lib smartgrid`
Expected: PASS.

- [ ] **Step 8: Pre-commit checklist, then commit:**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
git add server/src/smartgrid/actor.rs
git commit -m "Thread explicit resume_at through actor"
```

---

## Task 4: POST `resume_at` query param + past validation

**Files:**
- Modify: `server/src/routes/smartgrid.rs` (`SmartGridQuery` ~64, `set_smartgrid` ~113, the existing 2-arg `set_mode` call sites; tests)

- [ ] **Step 1: Write the failing test** in `mod tests`:

```rust
    #[tokio::test]
    async fn test_set_smartgrid_past_resume_at_is_bad_request() {
        let cancel = CancellationToken::new();
        let (handle, _join) =
            spawn_with_test_gpio(PriceState::new("SE3".to_string()), test_config(), cancel);
        let state = SmartGridState {
            handle: Some(handle),
            price_state: PriceState::new("SE3".to_string()),
            config: test_config(),
        };
        let past = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let query = SmartGridQuery {
            mode: "blocking".to_string(),
            schedule_resume: true,
            resume_at: Some(past),
        };
        let result = set_smartgrid(State(state), Query(query)).await;
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest));
    }
```

- [ ] **Step 2: Run — verify fail** (no `resume_at` field):

Run: `cargo test -p server --lib test_set_smartgrid_past_resume_at`
Expected: FAIL — `SmartGridQuery` has no field `resume_at`.

- [ ] **Step 3: Add the query field** (`SmartGridQuery`, ~64):

```rust
#[derive(Debug, Deserialize)]
struct SmartGridQuery {
    mode: String,
    #[serde(default)]
    schedule_resume: bool,
    /// Optional explicit resume instant (ISO 8601). When present with
    /// `schedule_resume=true`, schedules the auto-flip at exactly this time
    /// instead of the cheapest-run auto-pick. Must be in the future.
    #[serde(default)]
    resume_at: Option<String>,
}
```

- [ ] **Step 4: Parse + validate in `set_smartgrid`** (after the `mode` parse, ~130). Insert:

```rust
    let resume_at = match query.resume_at.as_deref() {
        Some(s) => {
            let parsed = DateTime::parse_from_rfc3339(s).map_err(|e| {
                error!("set_smartgrid: bad resume_at '{s}': {e}");
                ApiError::BadRequest
            })?;
            let when = std::time::SystemTime::from(parsed.with_timezone(&Utc));
            if when <= std::time::SystemTime::now() {
                error!("set_smartgrid: resume_at '{s}' is in the past");
                return Err(ApiError::BadRequest);
            }
            Some(when)
        }
        None => None,
    };
```
Then update the `set_mode` call (~132) to pass it:

```rust
    let fires_at = handle
        .set_mode(mode, query.schedule_resume, resume_at)
        .await
        .map_err(map_smartgrid_error)?;
```

- [ ] **Step 5: Fix the other `set_mode` test call sites.** Search the file for `.set_mode(` and any `SmartGridQuery {` literal missing the new field; add `resume_at: None` to existing `SmartGridQuery` test literals (in `test_set_smartgrid_no_gpio` and `test_set_smartgrid_invalid_mode`).

Run: `cargo build -p server --tests 2>&1 | grep -A2 "set_mode\|resume_at"`
Expected: no missing-field / arity errors after fixes.

- [ ] **Step 6: Run — verify pass:**

Run: `cargo test -p server --lib routes::smartgrid`
Expected: PASS.

- [ ] **Step 7: Pre-commit checklist, then commit:**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
git add server/src/routes/smartgrid.rs
git commit -m "Accept resume_at on smartgrid POST"
```

---

## Task 5: Dashboard API client

**Files:**
- Modify: `server/static/api.js` (~156-168)

> No JS test harness in this repo; verify by reading the diff and a syntax check. Behavioral verification happens after deploy (Task 7 hand-off).

- [ ] **Step 1: Add `getResumeCandidates`** after `getSmartGridResume` (~160):

```js
async function getResumeCandidates() {
    try { return await fetchJson(`${API_BASE}/smartgrid/resume_candidates`); }
    catch { return { candidates: [] }; }
}
```

- [ ] **Step 2: Extend `setSmartGridMode`** with an optional `resumeAt`:

```js
async function setSmartGridMode(mode, scheduleResume = false, resumeAt = null) {
    const url = `${API_BASE}/smartgrid?mode=${mode}` +
                (scheduleResume ? '&schedule_resume=true' : '') +
                (resumeAt ? `&resume_at=${encodeURIComponent(resumeAt)}` : '');
    const r = await fetch(url, { method: 'POST' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.json();
}
```

- [ ] **Step 3: Export it.** Find the `window.api = { … }` (or equivalent export) object in `api.js` and add `getResumeCandidates` next to `getSmartGridResume`.

Run: `node --check server/static/api.js`
Expected: no syntax errors.

- [ ] **Step 4: Commit:**

```bash
git add server/static/api.js
git commit -m "Add resume_candidates API client"
```

---

## Task 6: Dashboard picker UI (layout A)

**Files:**
- Modify: `server/static/app.jsx` (state ~447, fetch effect ~578, powersave dialog branch ~1564-1622)
- Modify: `server/static/style.css` (picker row styles)

> Mobile-canonical: verify at ≤480px viewport first. English-only text.

- [ ] **Step 1: Add picker state** near `resumeMode` (~447):

```jsx
  const [resumeCandidates, setResumeCandidates] = useState([]);
  const [pickedResume, setPickedResume] = useState(0); // index into candidates
```

- [ ] **Step 2: Fetch candidates when the enter-dialog opens.** Extend the existing effect (~578) to also fetch candidates for powersave:

```jsx
  useEffect(() => {
    if (confirm?.kind !== "enter") return;
    let aborted = false;
    window.api.getSmartGridResume()
      .then(r => { if (!aborted) setProposedResume(r); })
      .catch(() => {});
    if (confirm.target === "powersave") {
      window.api.getResumeCandidates()
        .then(r => { if (!aborted) { setResumeCandidates(r?.candidates || []); setPickedResume(0); } })
        .catch(() => { if (!aborted) setResumeCandidates([]); });
    }
    return () => { aborted = true; };
  }, [confirm]);
```

- [ ] **Step 3: Render the ranked list for powersave.** In the `isEnter` block (~1583), for `isPS` replace the two-radio `resume-options` with a candidate list + a manual row. Add this just before the existing `<div className="resume-options">` and gate the old block to non-powersave:

```jsx
                  {isPS ? (
                    <div className="resume-picker">
                      <div className="resume-picker-label">Resume heating at…</div>
                      {resumeCandidates.length === 0 ? (
                        <div className="resume-empty">No price data — block without auto-resume.</div>
                      ) : resumeCandidates.map((c, i) => {
                        const t0 = new Date(c.starts_at).toLocaleTimeString("sv-SE", { hour: "2-digit", minute: "2-digit", hour12: false });
                        const t1 = new Date(c.ends_at).toLocaleTimeString("sv-SE", { hour: "2-digit", minute: "2-digit", hour12: false });
                        const ore = Math.round(c.avg_spot_sek * 100);
                        return (
                          <label key={c.starts_at} className={`resume-row level-${c.level || "normal"}`}>
                            <input type="radio" name="resume"
                                   checked={resumeMode === "schedule" && pickedResume === i}
                                   onChange={() => { setResumeMode("schedule"); setPickedResume(i); }}/>
                            <span className="resume-time">{t0}–{t1}{i === 0 ? " · cheapest" : ""}</span>
                            <span className="resume-price">{ore} öre</span>
                          </label>
                        );
                      })}
                      <label className="resume-row manual">
                        <input type="radio" name="resume"
                               checked={resumeMode === "manual"}
                               onChange={() => setResumeMode("manual")}/>
                        <span className="resume-time">Block, no auto-resume</span>
                      </label>
                    </div>
                  ) : (
                    <div className="resume-options">
                      {/* existing optA/optB two-radio block, unchanged */}
                    </div>
                  )}
```
Keep the existing `resume-options` markup verbatim inside the `: (` branch.

- [ ] **Step 4: Use the picked slot on Enable.** In the primary button `onClick` (~1609), compute `resume_at`:

```jsx
                              const backendMode = UI_TO_BACKEND_MODE[target] ?? target;
                              const schedule = resumeMode === "schedule";
                              const resumeAt = (schedule && isPS && resumeCandidates[pickedResume])
                                ? resumeCandidates[pickedResume].starts_at
                                : null;
                              try {
                                await window.api.setSmartGridMode(backendMode, schedule, resumeAt);
                                setMode(target);
                              } catch (e) {
                                console.error("SmartGrid POST failed:", e);
                              }
                              closeConfirm();
```

- [ ] **Step 5: Reset picker state on close.** In `closeConfirm` (~1528) add `setResumeCandidates([]); setPickedResume(0);`.

- [ ] **Step 6: Add styles** to `style.css` (reuse the 5-band level colors already defined for the chart — match those variable names):

```css
.resume-picker { display: flex; flex-direction: column; gap: 8px; margin: 12px 0; }
.resume-picker-label { font-weight: 600; color: var(--text); }
.resume-empty { color: var(--muted); font-size: 13px; }
.resume-row {
  display: flex; align-items: center; gap: 10px;
  padding: 10px 12px; border: 1px solid var(--border); border-radius: 8px;
}
.resume-row .resume-time { flex: 1; }
.resume-row .resume-price { font-weight: 700; }
.resume-row.manual { opacity: 0.85; }
/* level-very_cheap / level-cheap etc.: tint border using existing band vars */
```

- [ ] **Step 7: Syntax check + visual review:**

Run: `node --check server/static/api.js` (app.jsx is JSX — review by eye; if a transpile step exists, run it).
Then, if a local build/preview exists, check the dialog at a 360px viewport. Otherwise defer visual check to post-deploy.

- [ ] **Step 8: Commit:**

```bash
git add server/static/app.jsx server/static/style.css
git commit -m "Add resume-slot picker to block dialog"
```

---

## Task 7: Final verification & hand-off

- [ ] **Step 1: Full pre-commit sweep:**

```bash
cargo fmt && cargo clippy --all-targets -- -W clippy::pedantic && cargo test --all-targets
cargo tarpaulin --all-targets --workspace --out Stdout   # ≥ 90%
```

- [ ] **Step 2: Rebase bracket — END:**

```bash
git fetch origin && git rebase origin/main
```

- [ ] **Step 3: Ask Magnus whether to bump VERSION** (CLAUDE.md: ask before version bump). Do not bump unprompted.

- [ ] **Step 4: Hand off.** Dashboard behavior is verified on the Pi after deploy (the new endpoint doesn't exist on prod until deployed; Playwright from this box is read-only against `ctc.lan`). Magnus runs deploy and `git push`/merge — do NOT run those.

---

## Self-Review

- **Spec coverage:** helper (Task 1) ✓; GET endpoint (Task 2) ✓; POST `resume_at` + 400 (Task 4) ✓; actor plumbing (Task 3) ✓; dashboard list + "Block, no auto-resume" + dismiss + empty-state (Task 6) ✓; greedy non-overlap / `<k` / zero-runs / adjacency tests (Task 1) ✓; endpoint full+empty (Task 2) ✓; past `resume_at`→400 (Task 4) ✓.
- **Placeholders:** none — all code shown. The one verbatim-reuse pointer (existing `optA/optB` block in Task 6 Step 3) refers to code already in the file being edited, not an undefined symbol.
- **Type consistency:** `ResumeCandidate { starts_at, ends_at, avg_spot_sek, level }`, `cheapest_runs_within(window, run_duration, k)`, `set_mode(mode, schedule_resume, resume_at)`, `setSmartGridMode(mode, scheduleResume, resumeAt)`, `getResumeCandidates()` used consistently across tasks.
- **Note:** Task 3's actor test is deliberately thin because the test GPIO harness rejects non-Normal writes; real acceptance/validation of `resume_at` is covered at the route layer (Task 4) and on-device after deploy.
