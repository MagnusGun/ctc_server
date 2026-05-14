# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`ctc_server` is a Rust-based server that interfaces with CTC heating systems via Modbus RTU (serial communication). It provides a RESTful API for monitoring and controlling heating system parameters like temperatures, heating modes, and power-saving settings.

## Build and Development Commands

### Building
```bash
# Build the entire workspace
cargo build

# Build release version
cargo build --release

# Build only the server package
cargo build -p server --release
```

### Running
```bash
# Run with default serial port (/dev/ttyAMA4)
cargo run -p server

# Run with custom serial port
cargo run -p server -- /dev/ttyUSB0

# Run release build
cargo run --release -p server
```

The server listens on `http://0.0.0.0:3000` by default.

### Testing
```bash
# Run all tests in workspace
cargo test

# Run tests for specific package
cargo test -p server

# Run specific test
cargo test test_get_scaled_value_pos

# Run tests with output
cargo test -- --nocapture
```

### Linting and Formatting
```bash
# Format code
cargo fmt

# Run clippy
cargo clippy --all-targets

# Run clippy with pedantic warnings
cargo clippy --all-targets -- -W clippy::pedantic
```

## Architecture

### Actor-Based Modbus Communication

The system uses an **actor pattern** for handling Modbus communication to ensure exclusive access to the serial port and sequential processing of requests:

- **CtcActor** (server/src/modbus/actor.rs:40): Main actor that owns the Modbus RTU context and processes parameter operations sequentially
- **Message Channel**: HTTP handlers send `(ParameterOperation, ResponseChannel)` tuples via an mpsc channel
- **Response Channel**: Each request includes a oneshot channel for receiving the result

This architecture prevents race conditions on the serial port and ensures reliable communication with the heating system.

### Modbus Parameter System

The codebase defines a comprehensive parameter system for CTC heating systems:

- **CTCModbusParameter** (server/src/modbus/mod.rs:13): Core struct defining parameter metadata including register addresses, scaling factors, access rights (R/RW), and min/max/step validation
- **bms_parameters.rs** (server/src/modbus/bms_parameters.rs): Defines 40+ predefined constants for heating system parameters using the `ctc_parameter!` macro
- **Scaling**: Raw u16 Modbus values are converted to/from physical units (e.g., 0.1 factor for temperatures)
- **Validation**: Write operations validate against min/max/step constraints read from the device

### HTTP API Routes

The Axum-based web server provides seven route modules:

1. **temperatures.rs** (server/src/routes/temperatures.rs): Temperature monitoring and control endpoints
   - Read room/outdoor/flow temperatures
   - Get/set room temperature setpoint

2. **ctc.rs** (server/src/routes/ctc.rs): Generic parameter access and convenience functions
   - Generic parameter read/write by address
   - Power-save mode (GPIO-based SmartGrid blocking)

3. **smartgrid.rs** (server/src/routes/smartgrid.rs): SmartGrid control via GPIO
   - Get/set SmartGrid mode (normal, blocking, lowprice, overcapacity)
   - Requires GPIO to be enabled in configuration

4. **visibility.rs** (server/src/routes/visibility.rs): Parameter visibility checking
   - Read visibility register bitmasks

5. **alarms.rs** (server/src/routes/alarms.rs): Alarm and info message endpoints
   - Quick status check (`/api/v1/alarms/status`) for polling
   - Full alarm details (`/api/v1/alarms`) with Swedish and English translations
   - Bitmask scanning and text buffer reading from CTC registers
   - In-memory caching of alarm text to minimize register writes

6. **grid.rs** (server/src/routes/grid.rs): Grid status and Tibber integration
   - Current tariff mode (high/low based on Swedish electricity schedule)
   - Real-time current hour consumption from Tibber WebSocket
   - Monthly peak tracking (top 3 high-tariff hours)

7. **heatpump_stats.rs** (server/src/routes/heatpump_stats.rs): Compressor cycle and run-time accumulators
   - `/api/v1/heatpump/stats` — totals, per-window counters, cycle stats
   - `/api/v1/heatpump/stats/history` — recent cycles + daily history
   - Persisted to JSON when `[heatpump_stats] persist_path` is configured (see Persistence below)

All Modbus-based modules use the same pattern: create a oneshot channel, send operation to actor via mpsc, await response.

### Tibber WebSocket Integration

The server includes real-time energy consumption tracking via Tibber's WebSocket API:

- **tibber.rs** (server/src/energy/tibber.rs): WebSocket client with automatic reconnection
  - Subscribes to `liveMeasurement` for real-time power and accumulated consumption
  - Periodic historical sync (every hour) to fetch and process completed hours
  - Filters data to current month only for peak tracking

- **grid.rs** (server/src/energy/grid.rs): Grid state management
  - Tracks hourly consumption during high-tariff periods only
  - Maintains top 3 peak hours for monthly average calculation
  - Thread-safe state shared between WebSocket handler and HTTP routes

- **tariff.rs** (server/src/energy/tariff.rs): Swedish electricity tariff schedule
  - High tariff: Weekdays 06:00-22:00 (winter) or 07:00-19:00 (summer)
  - Low tariff: All other times, weekends, and holidays

### Web Dashboard (Static Files)

The server serves a real-time status dashboard from `server/static/`:

- **index.html**: Dashboard structure with header badges (SmartGrid, Powersave), temperature cards, heat pump panel, power display, and alarms section
- **app.js**: JavaScript for fetching API data, updating the DOM, and handling user interactions
- **style.css**: Dark theme styling with responsive design for mobile

### Persistence

Heat-pump statistics survive restarts when `[heatpump_stats] persist_path` is set (or via `CTC_HEATPUMP_STATS_PERSIST_PATH`):

- Saved on every cycle completion, day rollover, and graceful shutdown (Ctrl-C handler in `main.rs` via `axum::serve(...).with_graceful_shutdown`).
- Atomic write: serialize to `<path>.tmp` then `rename` so a crash mid-write cannot corrupt the file.
- A schema-version field in the JSON guards future migrations; an unknown version is treated as a corrupt file.
- Missing or corrupt files log a warning and start fresh — never block startup.
- Docker: bind-mount `./data:/app/data` and set `CTC_HEATPUMP_STATS_PERSIST_PATH=/app/data/heatpump_stats.json`.

### SmartGrid Auto-Resume

The server can schedule an automatic flip back to `Normal` after a non-Normal mode is applied with `schedule_resume=true`. The resume target depends on the entering mode:

- **Blocking** (defer heating because prices are high): resume at the **start of the cheapest contiguous N-minute run** inside the configured window — let the heater catch up when prices are best across a full recovery cycle, not just a single 15-min tick. `N` is configurable (`auto_resume_min_duration_minutes`, default 30). Slots that form the run must be wall-clock adjacent (`slot[i].ends_at == slot[i+1].starts_at`); two cheap slots split by an expensive one do not combine. If no contiguous run of length `N` fits anywhere in the window (sparse price data, fragmented today/tomorrow boundary), the scheduler falls back to the single cheapest slot via `cheapest_within` so a schedule is still produced.
- **LowPrice / Overcapacity** (buffer extra heat now because prices are cheap): resume at the **start of the first non-cheap slot** inside the window. If the cheap run extends past the window, resume at the end of the window — never get stuck in a buffer mode indefinitely.
- **Normal**: never schedules.

Knobs and endpoints:

- Config: `[smartgrid] auto_resume_enabled` (default true), `auto_resume_window_hours` (default 12), `auto_resume_min_duration_minutes` (default 30, clamped to `[15, 240]`).
- `POST /api/v1/smartgrid?mode=blocking|lowprice|overcapacity&schedule_resume=true` — applies the mode AND schedules the resume.
- `POST /api/v1/ctc/powersave?active=true&schedule_resume=true` — same plumbing, but powersave is binary (Blocking ↔ Normal only).
- `GET /api/v1/smartgrid/proposed_resume` — previews the **Blocking-flavored** slot (cheapest in window). Read-only. There is no preview endpoint for the LowPrice/Overcapacity target — the value lands in the `scheduled_resume_at` field of the POST response.
- `DELETE /api/v1/smartgrid/scheduled_resume` — cancels a pending schedule **without** changing the mode. Idempotent (200 with `scheduled_resume_at: null` when nothing was pending).
- Any later mode change (manual or otherwise) calls `cancel_scheduled_resume()` first, so a stale schedule cannot fire after the user changes their mind.

Implementation:

- The actor's `do_set_mode` funnels every mode change through `compute_resume_target(...)` (in `server/src/smartgrid/actor.rs`), which dispatches by mode: Blocking → `PriceState::cheapest_run_within(window, run_duration)` with a fallback to `cheapest_within(window)`; LowPrice/Overcapacity → `PriceState::cheap_window_end(window)`.
- `cheapest_run_within` (`server/src/energy/price.rs`) walks slots in order, accumulating strictly-adjacent neighbours into a run, and ranks candidate run-starts by duration-weighted average `spot_sek`. Adjacency is exact: `slots[i].ends_at == slots[i+1].starts_at`. Unparseable timestamps end a run early; runs shorter than `run_duration` are skipped.
- The scheduled task uses `tokio::spawn`'s `AbortHandle`, stored on `GpioController` so any clone of the controller can cancel it. The task always sets `Normal` when it fires, regardless of the mode that scheduled it.
- "Cheap" is defined by the existing 5-band `PriceLevel` (`VeryCheap` or `Cheap`); the price-fetch loop fills `level` from spot percentiles even when Tibber is off (`main.rs:414-423`).

Key dashboard features:
- Auto-refresh every 5 seconds
- Clickable powersave badge to toggle power saving mode. When activating, the dashboard fetches `/api/v1/smartgrid/proposed_resume` and shows a confirm dialog with the start of the cheapest N-minute window the scheduler would pick — OK schedules an auto-resume, Cancel blocks without scheduling. The pending resume time is shown next to the badge.
- The spot-price chart renders a translucent vertical band over the scheduled run when `scheduled_resume_at` falls inside the chart's slot range. Visibility is data-driven (anchored to slot timestamps, not wall-clock "today"), so the band appears automatically a few seconds after midnight rollover when the resume target is in the new day.
- Color-coded status indicators (green=normal, amber=active/warning, red=alarm)
- Tooltips on all data fields explaining what each value represents

## Key Implementation Patterns

### Making Modbus Requests from Handlers

When adding new endpoints, follow this pattern (see server/src/routes/temperatures.rs:22-42):

```rust
async fn handler(State(tx): State<ModbusSender>) -> Result<String, (StatusCode, String)> {
    // 1. Create oneshot channel for this request
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    // 2. Send operation to actor
    tx.send((ParameterOperation::Read(PARAM_CONSTANT), response_tx))
        .await
        .unwrap();

    // 3. Wait for response
    match response_rx.await {
        Ok(Ok(value)) => Ok(format!("{{\"data\": {value}}}\n")),
        Ok(Err(e)) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
    }
}
```

### Adding New Modbus Parameters

To add new parameters, use the `ctc_parameter!` macro in server/src/modbus/bms_parameters.rs:

```rust
// Read-write parameter with min/max/step validation
ctc_parameter!(PARAM_NAME, register_id, "Description", scaling_factor, Access::RW, reg_base_for_minmax, visible_register, bit_position);

// Read-only parameter
ctc_parameter!(PARAM_NAME, register_id, "Description", scaling_factor, visible_register, bit_position);
```

### Actor Write Operations

Write operations (server/src/modbus/actor.rs:498) automatically:
1. Validate the parameter is writable
2. Read min/max/step constraints from the device
3. Validate the value fits within constraints
4. Write the value
5. Read back and verify the written value matches

## Serial Port Configuration

Default Modbus RTU settings (server/src/main.rs:37-44):
- Baud rate: 9600
- Data bits: 8
- Parity: Even
- Stop bits: 1
- Flow control: Hardware
- Timeout: 1 second
- Slave ID: 1

These settings match typical CTC heating system requirements.

## Workspace Structure

This is a Cargo workspace using Rust edition 2024:
- **server/**: Main server application with Axum web framework, tokio-modbus for Modbus RTU, and tokio-serial for serial communication
  - `smartgrid/gpio.rs`: SmartGrid relay control via GPIO pins (K24/K25)
  - `smartgrid/scheduler.rs`: `apply_mode` helper + auto-resume task
  - `energy/grid.rs`: Grid state management for peak tracking
  - `energy/tibber.rs`: Tibber WebSocket client for real-time consumption
  - `energy/tariff.rs`: Swedish electricity tariff schedule
  - `energy/price.rs`: Price-state cache and `cheapest_within(window)` helper
  - `time_utils.rs`: Time/date formatting utilities
  - `modbus/actor.rs`: Actor-based Modbus communication with retry and timeout
  - `modbus/operations.rs`: Parameter operation types and response handling
  - `routes/`: API endpoint handlers (temperatures, ctc, smartgrid, alarms, visibility, grid)
  - `static/`: Web dashboard files (HTML, JS, CSS)
- Workspace-level dependencies: tokio, tracing, serde shared across all members

## Important Notes

- The actor loop (server/src/modbus/actor.rs:838) runs indefinitely until the receiver channel closes
- All Modbus parameters use signed 16-bit values internally, even for unsigned physical values
- Temperature values typically use 0.1 scaling (raw value 221 = 22.1°C)
- The server uses single-threaded Tokio runtime (`current_thread` flavor) since Modbus is inherently sequential

## Coding Standards

### Code Quality Requirements

All code in this project must meet these standards before committing:

1. **Zero Clippy Warnings**
   ```bash
   cargo clippy --all-targets -- -W clippy::pedantic
   ```
   - Must produce zero warnings
   - Do not use `#[allow(...)]` without clear justification
   - Fix issues rather than suppressing warnings

2. **All Tests Pass**
   ```bash
   cargo test --all-targets
   ```
   - All existing tests must pass
   - Add tests for new functionality
   - Add tests for bug fixes to prevent regressions

3. **Code Coverage** (≥90%)
   ```bash
   cargo tarpaulin --all-targets --workspace --out Stdout
   ```
   - Minimum 90% code coverage across all targets
   - Ensure all critical paths are tested
   - Use `cargo tarpaulin --all-targets --workspace --out Html` for detailed reports

4. **Code Formatting**
   ```bash
   cargo fmt
   ```
   - All code must be formatted with rustfmt

5. **Float Comparisons in Tests**
   - Never use `assert_eq!` for float comparisons
   - Use epsilon-based comparison helper:
   ```rust
   fn assert_float_eq(a: f32, b: f32, msg: &str) {
       assert!((a - b).abs() < f32::EPSILON, "{msg}: expected {b}, got {a}");
   }
   ```

### Git Commit Message Guidelines

Follow these standards for all commits:

1. **Subject Line**
   - Maximum 50 characters
   - Start with imperative verb (Fix, Add, Update, Remove, Refactor)
   - Do not end with a period
   - Example: `Fix step validation and add proper float tests`

2. **Body** (optional, for complex changes)
   - Separate from subject with blank line
   - Wrap at 72 characters
   - Explain what and why, not how
   - Use bullet points for multiple changes

3. **Examples**
   ```
   Fix step validation from minimum value

   Add configurable retry logic for Modbus

   Refactor temperature endpoints to use helpers

   Update API response format to match spec
   ```

### Git Operation Policy

**Hard rule: never edit or commit while HEAD is `main`.** If you find yourself on `main` and the user asks for code changes, refuse and create a sibling worktree on a `feature/<snake_case>` branch first. This applies even to "trivial" one-line changes.

**Allowed Git writes (run them yourself when needed):**

- `git worktree add ../ctc_server-<snake_case> -b feature/<snake_case> origin/main` — create the sibling worktree before any code work
- `git worktree remove ../ctc_server-<snake_case>` and `git branch -D feature/<snake_case>` — cleanup after merge
- `git switch -c feature/<snake_case>` / `git switch feature/<snake_case>` — branch create / switch
- `git fetch origin` and `git rebase origin/main` — bracket rebases before starting work and before handing back
- `git add <files>` and `git commit -m "..."` — stage and commit work, but **only** when HEAD is a `feature/<name>` branch inside a worktree

**Reserved for the user (Claude does not run these):**

- `git push` (any form, incl. `-u origin feature/<name>`)
- `git merge` / squash-merge into `main`
- Any destructive rewrite of published history (`push --force`, `reset --hard` on `main`, etc.)

**Read-only Git is always fine:** `status`, `diff`, `log`, `show`, `blame`, `worktree list`, etc.

**Why:** Magnus deploys himself from feature worktrees onto `ctc.lan`. He keeps `main` clean as the deployable base and controls when work lands there. Agents owning the worktree/branch/rebase/commit loop unblocks routine work; reserving push and merge keeps the deployment gate with him.

**Pre-commit checklist still applies** — see [Pre-Commit Checklist](#pre-commit-checklist) below. Run `cargo fmt`, `cargo clippy -- -W clippy::pedantic`, and `cargo test --all-targets` before each commit. Commit subject ≤50 chars, imperative verb, no body unless complex.

### Pre-Commit Checklist

Before committing, verify:
- [ ] `cargo fmt` - Code is formatted
- [ ] `cargo clippy --all-targets -- -W clippy::pedantic` - Zero warnings
- [ ] `cargo test --all-targets` - All tests pass
- [ ] `cargo tarpaulin --all-targets --workspace` - Coverage ≥ 90%
- [ ] Commit message follows guidelines (≤50 chars, imperative verb)
