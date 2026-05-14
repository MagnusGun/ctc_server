# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **SmartGrid Blocking-resume picks cheapest contiguous run, not single 15-min slot** (`server/src/energy/price.rs::cheapest_run_within`): walks chained today/tomorrow slots and, for each future start inside `auto_resume_window_hours`, accumulates strictly-adjacent neighbours (`slot[i].ends_at == slot[i+1].starts_at` exactly) until the run covers `auto_resume_min_duration_minutes`. Runs are scored by duration-weighted average `spot_sek`; ties resolve to the first run found. Falls back to `cheapest_within` (the previous single-slot helper) when no run of the required length fits anywhere in the window — guards sparse price data and fragmented today/tomorrow boundaries so a schedule is always produced. Wired through the new `compute_resume_target(price_state, config, mode)` free function in `smartgrid/actor.rs`, also called by `GET /api/v1/smartgrid/proposed_resume` so the dashboard preview matches the schedule the actor would actually apply.
- **`auto_resume_min_duration_minutes` config** under `[smartgrid]` (default 30, clamped to `[15, 240]`): minimum contiguous run length the Blocking-resume scan looks for. Existed implicitly as a hard-coded "15 min" before this change.
- **`run_minutes` field** on `GET /api/v1/smartgrid`, the POST response, and `GET /api/v1/smartgrid/proposed_resume`: configured run length, exposed so the dashboard can render an overlay band on the price chart whose width matches the schedule.
- **Scheduled-run overlay on the spot-price chart** (`server/static/chart.jsx`): translucent accent-coloured band + left edge line + `RESUME` label marking the start of the scheduled run, sized to `run_minutes`. Visibility is data-driven: derives `chartStart`/`chartEnd` from `slots[0].starts_at` / `slots[N-1].ends_at` and renders the band only when `scheduledResumeAt` falls in `[chartStart, chartEnd)`, so brief mismatches between calendar date and price-fetch state around midnight don't misalign the band — it disappears the moment the resume target is in tomorrow's range and reappears on the next dashboard fetch after the server's price loop rolls forward. Three new CSS classes (`.resume-band`, `.resume-edge`, `.resume-label`).
- **`PriceState::test_support` `pub(crate)` module** (`server/src/energy/price.rs`): hoists the `slot(offset_mins, spot_sek)` and `make_run(start_offset_mins, prices)` test helpers out of the local `mod tests` so `smartgrid::actor::tests` can `use crate::energy::price::test_support::{make_run, slot as isolated_slot}` instead of duplicating ~30 lines of timestamp-building code.
- **Mobile-responsive dashboard variant**: same URL, viewport meta switched to `width=device-width, initial-scale=1` and the previous `body { min-width: 1280px }` floor removed; three new `@media` blocks in `styles.css` (1024 / 768 / 480 px) collapse the 4-up heat-pump grid → 2 → 1, single-column the row-1 split, wrap the topbar and `.card-head` status chips, drop the SmartGrid `.sg-pop` from anchored popover to fixed bottom-sheet, drop the confirm `.dialog` from `min-width: 380px` to `100vw - 24px` with stacked actions, drop `.trend-modal` / `.stats-modal` to full-screen sheets with `border-radius: 0`, hide hover-only `.has-tip > .tip` (unreachable on touch + leaks `white-space: nowrap` through `position: absolute` to inflate scrollWidth), and floor every clickable (`.chip`, `.metric.clickable`, `.sb-link`, `.tm-close`, `.dialog .actions .btn`, `.sg-opt`) at `min-height: 44px`. All `grid-template-columns: 1fr` tracks rewritten to `minmax(0, 1fr)` at and below 768 px so an intrinsic-content child can't push the track wider than the viewport. A `useMediaQuery()` / `useIsNarrow()` hook (`hooks.jsx`) re-renders `App` on the 480 px boundary cross and threads compact `height` props into `EnergyChart` (140 vs 200), `ActivityTimeline` (48 vs 60), `HeatingTrend` (180 vs 220, accepts a new `height` prop), and `TrendChart` (180 vs 200) — CSS handles every other concern. Verified at 375 × 812 in Playwright: every panel and every control still present (SmartGrid mode change, trend modals, stats deep-dive, Tweaks panel), zero horizontal scroll, all tap targets ≥ 44 px.
- **Modbus telemetry endpoint** (`GET /api/v1/modbus/stats`): single JSON document exposing HDR-histogram percentiles (p50 / p90 / p99 / p99.9 / max / mean / count) for read and write durations, wire-op rates over rolling 10 s / 60 s / 5 min windows, per-register counters (reads / writes / retry_attempts / final_failures) sorted with trouble registers on top, and a cap-100 FIFO ring of recent retry-emitting requests with per-attempt `ms_since_prev_wire_op` and `ms_since_request_first_attempt` fields. Goal is bus tuning (`operation_timeout_secs`, `inter_request_gap_ms`), not monitoring.
- **`SupervisorStats`** (`modbus/mod.rs`): atomic counters shared between the supervisor task and the actor, surviving every actor respawn. Surfaces `respawns`, `port_open_failures`, `last_respawn_epoch_secs`, and `actor_uptime_secs`. Producer-side calls funnel through named helpers (`record_build_failure`, `record_respawn`).
- **`hdrhistogram` crate** added to `server/Cargo.toml` for the duration histograms.
- **Configurable server timezone** (`tz`, default `Europe/Stockholm`), validated at config-load time
- **Sensor cache + redb storage** (`storage/`): in-memory 24h ring per sensor, hourly flush to redb, graceful-shutdown flush; cycles keyed by `(started_at, seq)` so same-second collisions both survive a flush+reopen
- **Sensor history persistence** (`storage/mod.rs`): polled samples bucket into 1-minute means and persist to a new `SERIES_MINUTES` redb table (key `(sensor_id, minute_unix)`, value 4-byte LE `f32`). On `Store::open` the table is hydrated back into the in-memory 24h ring so dashboard graphs render the pre-restart history immediately instead of starting over. Schema bumped v2 → v3 (no data migration — new empty table on first open under v3). 24h retention enforced by an in-tx prune on every flush. Open buckets (current minute) stay in pending across flushes; the existing dirty-generation collision-detection pattern is preserved. New `Store::bucket_minutes(sensor, from, to)` helper aggregates the live 5-second ring + hydrated minute means into a uniform 1-minute series across the restart boundary.
- **Background-task supervisor** (`supervisor.rs`): catches panics in spawned loops, logs the task name, and cancels the global shutdown token so HTTP doesn't keep serving stale state
- **SmartGrid actor** (`smartgrid/actor.rs`): mode flips, schedules, and auto-resume timers serialised through a single mpsc (replaces the prior multi-mutex flow); mode-generation supersession rejects a scheduled resume if the mode was manually changed in the meantime
- **15-minute consumption pipeline**: rolling 24h window of 15-min `{starts_at, kwh}` entries alongside the hourly stream
- **Activity timeline** for compressor state, with a windowed regression test (H-9) for the cycle-precedes-window edge case
- **Step-response recorder** running as its own background task
- New endpoints: `/api/v1/heatpump/series`, `/api/v1/heatpump/activity`, `/api/v1/heatpump/step_response`, `/api/v1/pump`, `/api/v1/modbus/stats`
- **Configurable price-fetch hour** (`price.fetch_hour_local`, default 14:00 Swedish-local) with startup jitter so multiple instances don't stampede
- **Inline favicon** (no more `/favicon.ico` 404)
- **Homey REST integration** for slaving the Cirkulationspump smart plug to SmartGrid mode (`server/src/homey/`): the actor pushes pump on/off via `PUT /api/manager/devices/device/{id}/capability/onoff` on every successful mode write — `Blocking` → off, every other mode → on — covering manual `/smartgrid`, legacy `/powersave`, and the auto-resume timer through the single `push_pump_to_homey` chokepoint; a reconciliation poller (`homey/poller.rs`, wrapped by `supervisor::spawn_with_shutdown` with a `CancellationToken` and `MissedTickBehavior::Delay`) periodically reads the plug and pushes a corrective value on drift, so Homey restarts / manual app toggles / failed pushes self-heal within `poll_interval_secs` (default 60 s, `0` disables); `HomeyPumpCache` feeds a `GET /api/v1/pump` endpoint with `{on, stale, last_observed_unix_secs}` (stable timestamp so the dashboard's JSON-dedup skips re-renders while state is unchanged; age is computed client-side) and a dashboard pump badge that surfaces stale state with an age tooltip
- **`[homey]` config block** (`enabled`, `url`, `token`, `pump_device_id`, `poll_interval_secs`) routed through `CTC_HOMEY_*` env vars with explicit validation on enable; deployment files updated end-to-end (`config.toml.example`, `docker-compose.yml`, `docker-compose.override.yml.example`, `.env`, `DOCKER.md`, `api.http`); PAT redacted from `HomeyClient` `Debug` output so it never leaks into logs
- Shared `homey::test_support` module (`MockState`, `spawn_mock`, `make_client`) deduplicates the in-process Homey mock used by `homey::{mod,poller}` and `smartgrid::actor` tests — no external mock-server crate
- Tests: HDR percentile sanity (asserts p99.9 lands in the tail), `OpCounts::bump` per-variant exhaustive coverage, `RetryRing` FIFO + cap-100, `per_register` cap-256 with in-place bump past cap, `RateWindow` count + eviction with full-window equality, supervisor-stats helpers, and three retry-event grain tests covering first-shot success / success-after-retry / final-failure-after-exhaustion. Pure first-shot successes intentionally emit no event; failing requests emit exactly one event with the full attempt-by-attempt detail.
- Tests: 24 new across `homey::{mod,cache,poller}` (in-process axum mock), `smartgrid::actor` (helper isolation + push-on-mode + Homey-unreachable cache staleness), `routes::pump` (handler-level state matrix using `ApiError::ServiceUnavailable`), and `config::tests` (env routing + per-field validation)
- Tests: SmartGrid mode-generation supersession; H-9 activity-window regression; Easter parametric sweep across 2010–2030; `parse_date_yyyymmdd` calendar-invalid rejection; DST window-rollover and active-cycle-credit regressions (spring-forward + fall-back Sundays); supervisor pre-cancelled-token and multi-task fan-out; `set_power_save` GPIO-error and actor-gone branches; `step_response` limit clamping; SmartGrid actor error-recovery and shutdown propagation across all handle methods; `PriceState` concurrent reader/writer smoke test
- Tests: 7 new in `storage::tests` covering the 1-minute persistence path — same-minute averaging, minute-boundary separation, open-bucket non-finalization (uses a future timestamp to avoid clock-rollover flakes), hydration round-trip ordering, 24h pruning past `SERIES_RETENTION_SECS`, NaN/Inf rejection at the pending-minutes layer, and the `bucket_minutes` helper collapsing sub-minute samples. The existing `flush_round_trip` test's "samples are RAM-only" assertion was removed since samples now persist; the route test `returns_recorded_samples` rewritten to `returns_recorded_samples_as_minute_mean` asserting the new aggregation contract.

### Changed
- **`modbus.inter_request_gap_ms` default 10 → 25 ms**: live prod stats (`/api/v1/modbus/stats`) showed ~3 timeouts/hour at 10 ms with every timing-out attempt firing at the gap floor (`ms_since_prev_wire_op: 10-11`) — CTC firmware needs more post-response settle time than the Modbus RTU 3.5-character minimum. 25 ms eliminates that retry class; at ~3.8 ops/s mean rate the throughput cost is ~57 ms/s of additional gap time.
- **Config defaults via `Default` trait** (`server/src/config.rs`): every substruct (`ServerConfig`, `SerialConfig`, `ModbusConfig`, `TemperatureValidationConfig`, `GpioConfig`, `TibberConfig`, `PriceConfig`, `HeatPumpStatsConfig`, `SmartGridConfig`, `HomeyConfig`, `StorageConfig`) now defines its canonical defaults in an `impl Default` block. Field-level `#[serde(default)]` on each `Config` field plus struct-level `#[serde(default)]` on multi-field substructs handle missing-table and partial-table cases respectively; a `default_tz()` helper covers the one top-level non-trivial default. The ~60-line `Config::apply_defaults` `set_default` chain is gone, along with the `#[allow(clippy::too_many_lines)]` it required. `test_partial_config` collapses from ~90 lines of duplicated defaults to a single `set_override` call.
- **`CtcActorBuilder::new` signature** (`server/src/modbus/actor.rs`): now `new(tty_path, &SerialConfig, &ModbusConfig) -> Result<Self, ConfigError>`; serial enum parsing (`get_parity`, `get_data_bits`, `get_stop_bits`, `get_flow_control`) moves into the constructor, replacing the inline chain of setters at the call site. `main.rs` builder usage collapses from a 14-line chain to three lines. The 13 dead per-field setters (`.baud_rate()`, `.parity()`, ..., `.inter_request_gap()`) and the surrounding `#[allow(dead_code)]` annotations are gone now that no caller — production or test — uses them; only `.sup_stats()` remains.
- **`[smartgrid] auto_resume_window_hours` default 8 → 12 h**: 12 h gives the cheapest-run scan more material to choose from (especially overnight), which matters more now that runs need contiguous slots rather than a single 15-min tick. Existing `clamp(1, 48)` unchanged. Sample-config and three test fixtures (`smartgrid/actor.rs`, `routes/smartgrid.rs`, `routes/ctc.rs`) updated in lockstep.
- **`with_retry!` macro** (`modbus/actor.rs`): grew a `$kind: WireOpKind` argument so per-register reads/writes and the right histogram are routed without per-call-site duplication. Six call sites updated. Macro body samples `Instant::now()` once post-wire and reuses the value for `last_wire_op`, `rate_window.push`, and `wire_elapsed`. Per-attempt detail collected via `make_attempt` / `make_event` closures (Copy-only captures) so the three failure arms don't duplicate struct literals.
- **`ModbusResponse`** gained a `Stats(Box<ModbusStats>)` variant. The box keeps the enum's max variant size unchanged (~32 B); inline `ModbusStats` would balloon it ~6×. Every existing exhaustive match on `ModbusResponse` (alarms, visibility, operations helpers) grew a `Stats(_)` arm that logs and returns `InternalError` — unreachable in practice for those routes since they never send `GetStats`.
- **Storage**: dropped JSON-on-disk for redb; sensor samples persist as 1-minute means in `SERIES_MINUTES` alongside cycles + daily aggregates; flush is idempotent when clean
- **Dashboard sensor-trend line resolution: 1-minute** (was 1-hour means over 24h). `/api/v1/heatpump/series` now returns 1-minute means via `Store::bucket_minutes` (server-side aggregation) instead of raw samples — payload drops from ~17k points/sensor to ~1.4k and the line is uniform across a server restart. Frontend `bucketHourly(points, 24)` replaced with `bucketMinutely(points)` producing length-1440 arrays; `TrendChart` (`trends.jsx`) and `HeatingTrend` (`stats-charts.jsx`) derive x-axis hourly tick labels and hover labels from each slot's actual epoch ms via new `minuteSlotTime` / `formatSlotHour` / `formatSlotHourMinute` helpers, replacing hardcoded hour arithmetic that didn't generalise past 24 buckets. X-axis still ticks at the hour; only the line resolution changed.
- **Price binning**: value-based percentile cutoffs (not positional rank), so ties land in the same level; all-equal sets return `Normal`; tolerance is half-an-öre (`ORE_TOLERANCE_SEK_PER_KWH`)
- **Price fetch loop**: transient failures preserve the cached today/tomorrow vectors instead of wiping them; 15-min retry continues until both populate or local midnight; switched to a daily 14:00 Swedish-local schedule (was hourly polling)
- **Tibber**: prices matched by hour (not string equality); historical filter uses Swedish-local month; markup classifies via coefficient of variation; midnight-reset detection has a 10 Wh jitter tolerance before re-anchoring; WS bails on 401 with exponential backoff for transient errors; 120 s read timeout for zombie-connection detection; 15 s HTTP timeout for REST calls
- **Tariff/peak tracking**: all day boundaries route through a DST-aware Swedish-local helper; monthly peak buckets keyed by local date; 15-min boundaries align across DST (whole-hour offsets only)
- **SmartGrid resume**: re-checks the wall clock at fire time; honors `schedule=true` even when mode is unchanged; guards against manual-override races; auto-resume window clamped to 1..=48 hours
- **`smartgrid::actor::spawn`** signature gains an `Option<HomeyHooks>` so callers can wire the pump-control side channel; `do_set_mode` and `on_resume_fire` call `push_pump_to_homey()` after every successful GPIO write — fire-and-forget via `tokio::spawn` so a slow LAN never stalls the actor; desired state published to a `tokio::sync::watch::Sender<bool>` synchronously so the reconciliation poller always sees the current intent
- **Graceful shutdown**: SIGTERM also handled (was SIGINT-only); background loops cancel cleanly; tasks that overshoot the 5 s deadline now `.abort()` instead of being detached
- **Modbus**: register values serialise as JSON numbers; writes verify against the raw register value and reject values outside `u16`; concurrent disconnected requests counted in `total_operations`; serial-config getters return `Result` instead of panicking; `CTC_SERIAL_*` and modbus env vars honored
- **Alarms**: `alarm_count` matches `alarms.len()`; first-seen keyed by `A:{code}` / `I:{code}` to avoid namespace collisions; concurrent `get_alarms` requests serialised; first-seen persisted across transient Modbus failures and text-fetch failures; alarm-info reference range extracted into named constants; `parse_alarm_text` preserves tab/newline (intentional record separators); poisoned `ALARM_FIRST_SEEN` lock recovers via `into_inner`
- **Visibility API**: `/api/v1/visibility` no longer reports a redundant `count` field; invalid registers return 404; bulk responses serialise via serde; scan failure falls back to optimistic visibility; visibility gate bypassed for custom CTC reads
- **CTC route**: cache-hit branch builds JSON via `serde_json` (was Debug-format `{v:?}`); internal register addresses hidden from API responses
- **Dashboard / JSX**: `usePolledFetch` stabilised via `useCallback`; trend modal handles null gap hours, shows error placeholder on fetch failure; trend chart y-axis auto-fits to data while honoring a per-chart soft minimum span — outdoor / brine / DHW no longer clip when values fall outside the previously hardcoded ranges (also handles negative outdoor temps); stats modal heading follows the active tab (Cycle times / Compressor starts / Operating hours / Heating system) instead of always reading "Heat pump statistics"; charts handle null buckets and zero variance; daily heatmap rendered against server-provided local dates; activity-timeline lane heights clamp non-negative; `StepResponse` picker defaults to newest event; "unknown" rendered when latest bucket / SmartGrid state is null
- **Cycle accounting**: cycles split at local midnight; accumulation paused during Modbus outage; `current_day_date` derived from `new_day_start` for consistency
- **Default `Tomorrow` availability check** covers any slot, not just the first
- **Optimal-hours response** reports the actual count, not the (capped) request
- **History days** clamped to ≥1
- **Clippy pedantic cleanup**: zero warnings across the workspace. Mutex-poison recovery in `heatpump/stats.rs` and `storage/mod.rs` switched from the `|e| e.into_inner()` closure to the `PoisonError::into_inner` method reference (17 sites). `wait_for_inter_request_gap` in `modbus/actor.rs` uses `Duration::saturating_sub` (functionally identical given the existing `elapsed < gap` guard). `SmartGridActor::run`'s `tokio::select!` arm and `Store::record_step_event`'s timestamp clamp use `if let ... else` instead of single-pattern `match`. Tuning constants `RESET_JITTER_KWH` (`tibber.rs`) and `ORE_TOLERANCE_SEK_PER_KWH` (`main.rs`) hoisted to the top of their enclosing functions. Six doc-comment identifiers (`SmartGrid`, `SetMode`, `GpioController`, `tokio_modbus`) wrapped in backticks. The two pre-midnight credit DST tests in `heatpump/stats.rs` now use the in-file `assert_float_eq` helper on `operating_hours` directly, replacing an integer-cast `assert_eq!` that violated the project's float-compare rule. One justified `#[allow(clippy::too_many_lines)]` added to `Config::load_with_env` (a flat sequence of `.set_default()` builder calls; splitting would fragment a single declarative schema).

### Fixed
- **Spot-price chart now renders as a step function, not interpolated diagonals** (`server/static/chart.jsx`): each 15-min slot is drawn as a horizontal segment at its `spot_sek` height across `[xFor(i), xFor(i+1)]`, coloured by the slot's own `level`; transitions between adjacent slots are drawn as verticals at the slot boundary, coloured by the *incoming* slot's level so the new price-level colour starts cleanly at the wall-clock boundary. Previous diagonal rendering made cheap (green) segments slope upward into expensive (red) price territory while keeping the cheap colour, miscommunicating the actual price level at that height. The shaded area-fill (`buildArea`) follows the same staircase outline. Hover circle and now-indicator unchanged — they were already anchored at slot starts, which is geometrically correct for step rendering. `strokeLinecap` switched from `round` to `butt` on the price segments so rounded caps don't bulge across slot boundaries and re-introduce the visual smudging.
- `HEATSYSTEM_STATUS` declared unsigned (was signed → negative readouts on high values)
- elpris zone fetch uses Swedish-local "today" (was UTC → off by an hour in the evening)
- Spring-forward handled in tariff conversion; ISO 8601 timestamps parsed via chrono
- 15-min spot-price index off-by-one on the dashboard
- Price trend: zero-baseline guard; ≥8 points required; non-positive spot handled
- Step-response event closes immediately when its span is zero
- OOB index in Modbus validation read; visibility scan OOB
- `record_first_seen` race condition; concurrent writes lost across flush commit
- `cycle_seq_next` saturates instead of wrapping at `u32::MAX`
- `parse_date_yyyymmdd` calendar-validated (rejects Feb 31, non-leap Feb 29, etc.)
- `parse_alarm_text` no longer drops tab/newline separators
- `SmartGridMode` round-trip uses lowercase strings
- `series_range` writes don't mark the dirty flag for RAM-only sensors
- `proposedResume` resets on dialog close
- SmartGrid blurb matches the backend resume time
- "Now" shows unknown when latest bucket is null
- `StartsVsTemp` scatter/regression drops null-outdoor rows
- Heat-pump stats charts: `useState` hoisted above `EmptyChart` guards
- Heating-trend gap hours render as gaps, not zero-dives
- Activity segment opens at window start when the cycle began before the window
- Per-source errors surface so the dashboard Connected chip can flip on partial outages
- Compressor Starts caption matches the actual amber threshold (>14 starts/day); previously read "typical 5–10 starts/day envelope" while code only flagged bars at >14
- Mutex poison handling in `heatpump/stats.rs` (`mark_poll_failed`, `update_state`, `get_summary`, `get_history`) now matches the storage convention (`unwrap_or_else(|e| e.into_inner())`); a poisoned mutex no longer panics the poller
- `snapshot_current_day` routes corrupt-clock days (year < 0) to a `19700101` sentinel instead of collapsing onto an `MMDD`-shaped key that successive failures would overwrite
- `activity::iso()` epoch fallback logs a warning before substituting 1970, so silently-malformed timestamps surface in logs
- EnergyChart NOW marker uses the same `/24` denominator as the axis ticks (was `/23` against a `/24` axis, drifted ~1 h from wall clock)
- HeatingTrend and TrendChart anchor the rightmost x-axis tick (and hover tooltip) to the current local hour for the rolling 24 h window — labels were previously a fixed `00:00…23:00` sequence unrelated to the data
- StartsDaily / StartsVsTemp parse `daily[].date` via the existing `parseLocalDate` helper instead of `new Date(d.date)`, so day labels stay consistent with the calendar heatmap across negative-UTC-offset browsers

### Removed
- **Tibber as a price source** (elpris is now the only price source; Tibber WebSocket consumption stream retained)
- Unreachable warn-chip code path; unused `cancelSmartGridResume` export; unused `is_info` field from `parse_alarm_text`

### Refactored
- `gpio.rs` split into `smartgrid/gpio.rs` + `smartgrid/actor.rs`; `smartgrid/scheduler.rs` shrunk to tests as logic moved into the actor
- `GpioController` further simplified now that the actor is its sole owner: `Arc<Mutex<…>>` / `Arc<AtomicU64>` wrappers dropped in favour of plain fields, mutating methods promoted to `&mut self`, `Clone` removed; supersession invariants now enforced by Rust's borrow checker instead of a held mutex
- Smartgrid actor reuses the well-tested `tibber::parse_iso8601` for scheduled-resume timestamps (drops the local RFC-3339 wrapper that lacked tz-fallback / pre-epoch rejection)
- Price fetch loop runs today + tomorrow requests concurrently via `tokio::join!` (worst-case wall-clock halved)
- Alarms route namespaces first-seen cache keys via an `AlarmKind` enum + `cache_key` helper instead of raw `"A:"` / `"I:"` string prefixes
- Duplicate match arms in `routes/smartgrid::map_smartgrid_error` folded via `e @ (...)` binding
- Dashboard stats-tab title rendering switched from a 4-way ternary to a `STATS_TAB_TITLES` lookup
- Helper for info-reference offset arithmetic in the alarms route
- Route tests (`heatpump_stats`, `visibility`, `grid`) deserialize responses into typed JSON `Value` and assert on shape, replacing substring `contains` checks that masked schema regressions
- Sleep-based test syncs replaced with deterministic primitives: `Barrier` for the supervisor panic-propagation test; `try_recv` after a yield loop for the alarms concurrent-call serialization probe; cancellation-then-bounded-await for the heatpump poller (was `handle.abort()` after a 150 ms sleep)
- Visibility-scan docstrings reference the configured register range (62500–62548 by default) instead of a hardcoded "49"
- Shared `series_window(hours) -> (from, now, to)` helper consolidating the unix-second math previously duplicated in `routes/series` and `routes/activity`
- Shared `serialize_response` helper in `routes/visibility` replacing three copies of the serialise + push-newline boilerplate
- Shared `lookup` helper in `routes/alarms` collapses three near-identical translation/description lookup wrappers
- Shared `energy::http_client()` `OnceLock` reused by Tibber + elpris (replaces three separately built `reqwest::Client` instances with 15 s timeouts)
- Shared `sparkSegments(data, w, h, min, range)` helper in `components.jsx` reused by `Sparkline` and `MultiSparkline`
- `parse_alarm_text` strips an optional `A1`–`A10` heat-pump unit prefix per the CTC protocol so HP-scoped E-codes resolve their translation instead of falling through with `code = null`
- Tibber historical sync parses each ISO 8601 timestamp once per node (was twice via `extract_year_month` + `parse_iso8601`), saving ~744 redundant parses per startup
- GPIO `set_mode_if_not_superseded` now guards the generation check and delegates to `set_mode`, eliminating ~40 lines of duplicated GPIO-write boilerplate
- Dashboard `usePolledFetch` skips state updates when the polled JSON matches the previous payload, stopping downstream `useMemo` recomputes on identical 5 s ticks; `getTemperatures` pre-stringifies error reasons so `JSON.stringify`-based detection sees message changes
- `getAlarms` dropped the `/alarms/status` pre-check — `/alarms` already early-returns when counts are zero, saving one Modbus round-trip per 5 s poll while alarms exist
- Dashboard mode/level/status ternary cascades replaced with module-level lookup tables (`BACKEND_TO_UI_MODE` / `UI_TO_BACKEND_MODE` derived from one, `PRICE_LEVEL_LABELS`, `HEATING_STATUS_CLASS`)
- `cargo fmt` + pedantic-clippy clean across the tree

## [0.3.0] - 2026-01-11

### Added
- **Heat Pump Statistics Module** (`heatpump/`): Compressor cycle tracking and analysis
  - `heatpump/stats.rs`: Tracks cycle times (min/max/avg), starts per window (hour/day/week/month/year), operating hours per window, outdoor temperature correlation
  - `heatpump/poller.rs`: Background polling loop reading heat pump status register
  - Configurable via `heatpump_stats.enabled` and `heatpump_stats.poll_interval_secs`
- **Heat Pump Stats API**: New endpoints for statistics and historical data
  - `GET /api/v1/heatpump/stats`: Current statistics summary with cycle times, starts, operating hours
  - `GET /api/v1/heatpump/stats/history?days=N`: Historical data for charts (default 30 days, max 365)
- **Dashboard Heat Pump Statistics Panel**: Interactive statistics display
  - Cycle times (min/max/avg) with duration formatting
  - Compressor starts per time window (hour/day/week/month/year)
  - Operating hours per time window
  - Click to open modal with Chart.js visualizations
- **Statistics Charts Modal**: Five chart types for analysis
  - Cycles: Bar chart of cycle durations over time
  - Hours/Day: Daily operating hours
  - Starts/Day: Daily compressor starts
  - Cycle vs Temp: Scatter plot correlating cycle duration with outdoor temperature
  - Hours vs Temp: Scatter plot correlating daily hours with average temperature
- Chart.js integration (loaded from CDN) for dashboard charts

### Changed
- Modbus actor: Exposed raw register reading via `ParameterOperation::ReadRaw` for heat pump status polling
- Configuration: Added `HeatPumpStatsConfig` struct with `enabled` (default: true) and `poll_interval_secs` (default: 10)

## [0.2.1] - 2026-01-11

### Fixed
- Timezone parsing in Tibber historical data: timestamps with offsets (+01:00) now correctly convert to UTC, fixing peak hour tracking and tariff detection
- WebSocket zombie connection detection: added 120s read timeout with automatic reconnection

### Changed
- Price chart: Added interactive hover/touch with crosshair and tooltip showing price at hovered time
- Dashboard UI: Merged Grid and Prices panels into single "Energy & Prices" section
- Dashboard localization: Changed to English (High/Low Tariff, en-GB date format)
- Modbus defaults: Operation timeout reduced from 5s to 1s, channel buffer size increased from 24 to 32
- Price state: Current price recalculated on each request for 15-minute period freshness
- Dockerfile: Added explicit `--platform=linux/arm64` for runtime stage

## [0.2.0] - 2026-01-05

### Added
- **Web Dashboard** (`static/`): Real-time status page with dark theme
  - Temperature cards, heat pump panel, power display, alarms section
  - Header badges for SmartGrid mode and powersave status
  - Clickable powersave toggle with confirmation dialog
  - Auto-refresh every 5 seconds, responsive mobile design
- **Energy Module** (`energy/`): Comprehensive energy management
  - `energy/tibber.rs`: Tibber WebSocket client with automatic reconnection and periodic historical sync
  - `energy/grid.rs`: Grid state management for tracking hourly consumption and monthly peaks
  - `energy/tariff.rs`: Swedish electricity tariff schedule (high/low tariff periods)
  - `energy/elpris.rs`: Nord Pool spot prices via elprisetjustnu.se API (free, no auth)
  - `energy/price.rs`: Price state management with dual-source comparison and markup analysis
  - `routes/grid.rs`: Grid status API endpoints (`/api/v1/grid`, `/api/v1/grid/tariff`)
- **Messages Module** (`messages/`): Alarm code handling extracted from routes
  - `messages/translations.rs`: Swedish and English alarm code translations
  - `messages/types.rs`: Alarm message types, bitmask scanning, text buffer decoding
- **SmartGrid Module** (`smartgrid/`): Reorganized GPIO control
  - `smartgrid/gpio.rs`: GPIO relay control (moved from root `gpio.rs`)
  - `smartgrid/mode.rs`: SmartGrid mode enum with terminal state mapping
- **Visibility API expansion**: New endpoints for bulk register reads and parameter lookups
  - `GET /api/v1/visibility`: Read all 49 visibility registers in one request
  - `GET /api/v1/visibility/parameter/{addr}`: Check if specific parameter is visible
- **Price API**: Electricity spot price endpoints via elprisetjustnu.se
  - `GET /api/v1/prices`: All prices for today (+ tomorrow after ~13:00 CET)
  - `GET /api/v1/prices/current`: Current hour price with statistics
  - `GET /api/v1/prices/optimal`: Optimal scheduling hours based on lowest prices
- Monthly peak tracking: Calculates average of top 3 high-tariff consumption hours
- Historical data sync: Fetches up to 744 hours of data, filters to current month
- Hourly sync task: Runs 5 minutes after each hour to catch consumption updates
- DST-safe month filtering using local time from ISO 8601 timestamps
- Price comparison: Spot vs Tibber total with markup calculation and analysis

### Changed
- Grid route now reads current hour consumption from WebSocket-populated state instead of HTTP API polling
- Simplified `TibberConfig` - removed unused `TibberHome` struct (uses first home from API)
- Docker cross-compilation: Build from x86_64 to ARM64 instead of QEMU emulation
- Dockerfile updated to Rust 1.92 for edition 2024 support
- Docker Compose: Added GPIO device mapping (`/dev/gpiochip0`) for SmartGrid relay control in containers
- Docker Compose: Added Tibber environment variable passthrough (`CTC_TIBBER_ENABLED`, `CTC_TIBBER_API_TOKEN`)
- Docker Compose: Health check changed from `curl` to `wget` (smaller image footprint)
- Docker Compose: Default log level changed from `debug` to `info`

### Removed
- HTTP API polling for Tibber (replaced by WebSocket real-time data in `energy/tibber.rs`)

### Refactored
- `gpio.rs` moved to `smartgrid/gpio.rs` with new `smartgrid/mode.rs` for mode enum
- Alarm translations and types extracted from routes into `messages/` module

### Fixed
- Current hour consumption now displays correctly on dashboard (was showing 0.00 kWh)
- Environment variable parsing for nested config fields (`CTC_TIBBER_API_TOKEN`)
- rustls crypto provider initialization (required for rustls 0.23+)

## [0.1.0] - Initial Release

### Added
- Modbus RTU communication with CTC heating systems via actor pattern
- Temperature monitoring and control endpoints
- SmartGrid GPIO relay control
- Alarm and info message monitoring with translations
- Web dashboard with real-time status display
- Power-save mode toggle
- Parameter visibility checking
- Configurable serial port settings
- Docker deployment support
