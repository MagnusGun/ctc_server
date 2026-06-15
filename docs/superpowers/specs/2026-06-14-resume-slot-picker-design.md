# Design — SmartGrid Resume-Slot Picker

**Date:** 2026-06-14
**Branch:** `feature/resume_slot_picker`
**Status:** Approved design — pending implementation plan

## Problem

When the user enables **Blocking** (SmartGrid / powersave), the server auto-schedules
a resume at the single cheapest contiguous run in the next 12 h
(`cheapest_run_within`). The dashboard's block flow shows only that one computed
window in a confirm dialog. The user wants to **see a ranked selection of cheap
resume slots across the upcoming 12 h and pick one**, instead of being locked to
the auto-cheapest.

## Goal

Preview a ranked list of candidate resume runs in the next 12 h; the user picks
one and that becomes the scheduled auto-resume target. Ship **API + mobile
dashboard picker** in this iteration. Layout **A (ranked list)**.

## Non-goals

- Timeline-band picker (layout B) — possible follow-up on the same endpoint.
- Manual future *block* windows (scheduling a future block) — different feature.
- Changing the auto-pick default behavior — it stays as the top candidate.

## Decisions (locked)

- List thinning: **greedy non-overlapping**, cheapest first, **top 6**.
- Run duration per candidate = configured `auto_resume_min_duration_minutes`
  (default 30 min), identical to the auto-pick.
- Extra picker action: **"Block, no auto-resume"** (enter Blocking, schedule
  nothing). Dismissing the dialog makes no state change (does not enter Blocking).
- Morning data-horizon corner (12:00–14:00, `now+12h` up to ~2h past the loaded
  horizon before the next fetch): **just cap the list to loaded data**, no
  on-demand fetch.

## Architecture

### 1. Price helper — `server/src/energy/price.rs`

New `PriceState::cheapest_runs_within(window, run_duration, k) -> Vec<ResumeCandidate>`,
beside the existing `cheapest_run_within` (which stays for the auto-pick).

- Reuse the run-scoring loop: for every future slot-start in `(now, now+window]`,
  accumulate strictly-adjacent slots (`slots[i].ends_at == slots[i+1].starts_at`,
  exact) until `run_duration` is covered. Score = duration-weighted average
  `spot_sek`.
- Collect **all** qualifying runs, sort ascending by score, then **greedy
  non-overlapping**: take the cheapest, drop every run whose `[start, end)`
  intersects it, repeat until `k` runs chosen or candidates exhausted.
- `end` of a run = the accumulated end time (start + summed slot durations until
  `run_duration` is covered).

New serde-serializable struct:

```rust
struct ResumeCandidate {
    starts_at: String,      // ISO-8601, slot start
    ends_at: String,        // ISO-8601, accumulated run end
    avg_spot_sek: f64,      // duration-weighted mean spot price
    level: Option<PriceLevel>, // start slot's level → badge color
}
```

Inherits the existing `start > now` past-skip, so it is correct during the
00:00–14:00 price-staleness window (today bucket = yesterday is all past →
skipped; future candidates come from the tomorrow bucket).

### 2. API — `server/src/routes/smartgrid.rs` + actor

**New GET `/api/v1/smartgrid/resume_candidates`**
- Returns `{ "candidates": [ResumeCandidate, …] }` (≤ 6), cheapest first.
- Empty array when no price data (not 503 — empty is a valid "nothing to pick").
- Read-only. Coexists with `/proposed_resume` (single-value, still backs the
  auto-pick preview).

**Extend POST `/api/v1/smartgrid?mode=blocking&schedule_resume=true`**
- Add optional `resume_at=<ISO-8601>`. When present **and** `schedule_resume=true`:
  apply Blocking and schedule the resume at exactly that timestamp, bypassing
  `compute_resume_target`'s auto-pick.
- `resume_at` in the past → **400**, mode unchanged.
- `resume_at` without `schedule_resume=true` → ignored (auto-pick path).
- Without `resume_at` → unchanged: auto-picks cheapest run.
- Same `cancel_scheduled_resume()`-first plumbing, so a pick supersedes any prior
  schedule.

New actor helper `schedule_resume_at(explicit_time)` parallel to the existing
compute-then-schedule path. The fired task still always sets `Normal`.

### 3. Dashboard (mobile-canonical, layout A) — `server/static/`

- Tapping the **Powersave/Block badge** fetches `/resume_candidates` (replacing
  the current `/proposed_resume` confirm dialog) and renders the **ranked list**.
- Row: `HH:MM–HH:MM` · level badge (reuse the 5-band chart palette) · avg price
  in öre/kWh. Top row tagged as the cheapest/default.
- Tap a row → `POST …?mode=blocking&schedule_resume=true&resume_at=<row.starts_at>`.
  Dialog closes; pending-resume time shows next to the badge (existing affordance).
- Button **"Block, no auto-resume"** → `POST …?mode=blocking` (no schedule).
- Dismiss (tap-outside / X) → no state change, does not enter Blocking.
- Empty candidate list → dialog shows only the "Block, no auto-resume" button and
  a "no price data" note.

## Error handling / edges

- `< 6` distinct runs (short data horizon) → return what exists; list shorter.
  Late evening already pulls next-day runs (loaded in the `tomorrow` bucket after
  the 14:00 fetch), so the 12 h window spans midnight naturally.
- `resume_at` in the past → 400.
- No price data → empty candidates array; picker degrades to block-only.

## Testing

- `price.rs` unit tests: greedy non-overlap correctness; fewer than `k` runs;
  zero runs; adjacency boundary (gap ends a run); ordering by avg; run extends
  across the today/tomorrow bucket seam.
- `routes/smartgrid.rs`: candidates endpoint full + empty; POST with valid
  `resume_at` schedules at that exact time; past `resume_at` → 400; `resume_at`
  without `schedule_resume` ignored.
- Standards: zero clippy (`-W clippy::pedantic`), `cargo fmt`, all tests pass,
  ≥ 90% coverage, epsilon float comparisons (no `assert_eq!` on floats).
