# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
