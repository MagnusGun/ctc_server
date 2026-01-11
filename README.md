# ctc_server

## Overview
`ctc_server` is a Rust-based server application designed to interact with a CTC heating system via Modbus RTU. It provides a RESTful API for monitoring and controlling various parameters of the heating system, such as room temperature, outdoor temperature, and power-saving modes.

## Features
- **Web Dashboard**: Real-time status page with temperatures, heat pump data, power consumption, and alarms.
- **Heat Pump Statistics**: Compressor cycle tracking with min/max/avg times, starts per window, operating hours, and temperature correlation charts.
- **Tibber Integration**: Real-time energy consumption tracking via WebSocket with monthly peak averaging.
- Retrieve and set room temperature setpoints.
- Monitor outdoor, flow, and return temperatures.
- Enable or disable power-saving modes (with clickable toggle on dashboard).
- SmartGrid control via GPIO relays.
- Alarm and info message monitoring with English/Swedish translations.
- Parameter visibility checking.
- Generic Modbus parameter querying.
- Modular and extensible design.

## Project Structure
The project is organized as a Rust workspace with the following structure:
```
ctc_server/
├── server/                   # Main server implementation
│   ├── src/
│   │   ├── main.rs           # Application entry point
│   │   ├── config.rs         # Configuration management
│   │   ├── error.rs          # Error types
│   │   ├── energy/           # Energy management modules
│   │   │   ├── mod.rs        # Module exports
│   │   │   ├── elpris.rs     # Nord Pool spot prices via elprisetjustnu.se
│   │   │   ├── grid.rs       # Grid state for peak tracking
│   │   │   ├── price.rs      # Price state with dual-source comparison
│   │   │   ├── tariff.rs     # Swedish electricity tariff schedule
│   │   │   ├── tibber.rs     # Tibber WebSocket client for real-time data
│   │   ├── heatpump/         # Heat pump statistics tracking
│   │   │   ├── mod.rs        # Module exports
│   │   │   ├── poller.rs     # Background polling for compressor status
│   │   │   ├── stats.rs      # Cycle statistics and history tracking
│   │   ├── messages/         # Alarm and info message handling
│   │   │   ├── mod.rs        # Module exports
│   │   │   ├── translations.rs # Swedish/English alarm translations
│   │   │   ├── types.rs      # Alarm types and bitmask parsing
│   │   ├── modbus/           # Modbus parameter definitions and utilities
│   │   │   ├── mod.rs        # Core Modbus logic
│   │   │   ├── actor.rs      # Actor-based Modbus request handling
│   │   │   ├── bms_parameters.rs # Predefined Modbus parameters
│   │   │   ├── operations.rs # Modbus operation types
│   │   ├── routes/           # API route handlers
│   │   │   ├── mod.rs        # Route module definitions
│   │   │   ├── alarms.rs     # Alarm and info message endpoints
│   │   │   ├── ctc.rs        # Generic CTC and powersave endpoints
│   │   │   ├── grid.rs       # Grid status and Tibber integration endpoints
│   │   │   ├── heatpump_stats.rs # Heat pump statistics endpoints
│   │   │   ├── smartgrid.rs  # SmartGrid GPIO control endpoints
│   │   │   ├── temperatures.rs # Temperature endpoints
│   │   │   ├── visibility.rs # Parameter visibility endpoints
│   │   ├── smartgrid/        # SmartGrid control modules
│   │   │   ├── mod.rs        # Module exports
│   │   │   ├── gpio.rs       # GPIO relay control for K25/K26
│   │   │   ├── mode.rs       # SmartGrid mode enum
│   │   ├── static/           # Web dashboard files
│   │   │   ├── index.html    # Dashboard HTML
│   │   │   ├── app.js        # Dashboard JavaScript
│   │   │   ├── style.css     # Dashboard styling
│   ├── Cargo.toml            # Server crate dependencies
├── smartgrid_test/           # SmartGrid relay test tool
│   ├── README.md             # Tool documentation
│   └── src/main.rs           # CLI implementation
├── Cargo.toml                # Workspace definition and dependencies
├── LICENSE                   # License information
├── README.md                 # Project documentation
```

## API Endpoints
### Temperature Routes
- `GET /api/v1/temperature/room`: Get the current room temperature.
- `GET /api/v1/temperature/room/setpoint`: Get the room temperature setpoint.
- `POST /api/v1/temperature/room/setpoint/`: Set the room temperature setpoint.
- `GET /api/v1/temperature/outdoor`: Get the outdoor temperature.
- `GET /api/v1/temperature/flow`: Get the outgoing flow temperature.
- `GET /api/v1/temperature/flow/return`: Get the return flow temperature.

### CTC Routes
- `GET /api/v1/ctc`: Query generic Modbus parameters.
- `POST /api/v1/ctc`: Write generic Modbus parameters.
- `POST /api/v1/ctc/powersave`: Enable or disable power-saving mode.
- `GET /api/v1/ctc/powersave`: Get the current power-saving state.

### SmartGrid Routes
- `GET /api/v1/smartgrid`: Get the current SmartGrid mode.
- `POST /api/v1/smartgrid?mode=<mode>`: Set SmartGrid mode. Valid modes: `normal`, `blocking`, `lowprice`, `overcapacity`.

### Alarm Routes
- `GET /api/v1/alarms/status`: Quick status check returning alarm/info counts.
- `GET /api/v1/alarms`: Full alarm details with codes, messages, and translations.

### Visibility Routes
- `GET /api/v1/visibility`: Read all 49 visibility registers (62500-62548) in one request.
- `GET /api/v1/visibility/{register}`: Read visibility bitmask for a specific register.
- `GET /api/v1/visibility/parameter/{addr}`: Check if a parameter is visible on the connected hardware.

### Grid Routes (Tibber Integration)
- `GET /api/v1/grid`: Get grid status including tariff, current hour consumption, and monthly peak data.
- `GET /api/v1/grid/tariff`: Get current tariff mode only (lightweight polling).

### Heat Pump Statistics Routes
- `GET /api/v1/heatpump/stats`: Get current compressor statistics (cycle times, starts per window, operating hours).
- `GET /api/v1/heatpump/stats/history?days=N`: Get historical data for charts (default: 30 days, max: 365).

### Price Routes (Spot Prices)
- `GET /api/v1/prices`: Get all electricity prices (today + tomorrow if available after ~13:00 CET).
- `GET /api/v1/prices/current`: Get current hour's price with statistics.
- `GET /api/v1/prices/optimal`: Get optimal hours for scheduling based on lowest prices.

### Web Dashboard
- `GET /`: Serves the real-time status dashboard (static files).

## Getting Started
### Prerequisites
- Rust (edition 2024)
- A Modbus-compatible CTC heating system
- Serial connection to the heating system

### Raspberry Pi Hardware Setup

This server is designed to run on a Raspberry Pi 4B connected to a CTC heating system via direct TTL serial connection.

#### UART Configuration

The default serial port is `/dev/ttyAMA4` (UART4). To enable UART4 on Raspberry Pi 4B, add the following to `/boot/firmware/config.txt`:

```
dtoverlay=uart4
```

Then reboot the Raspberry Pi. See the [official UART configuration guide](https://www.raspberrypi.com/documentation/computers/configuration.html#configuring-uarts) for more details.

#### GPIO Pin Mapping

For Raspberry Pi 4B using UART4:

| Signal | GPIO | Pin | Description |
|--------|------|-----|-------------|
| TXD4   | GPIO 8  | 24 | Transmit data |
| RXD4   | GPIO 9  | 21 | Receive data |
| CTS4   | GPIO 10 | 19 | Clear to send (hardware flow control) |
| RTS4   | GPIO 11 | 23 | Request to send (hardware flow control) |

These pins use the ALT4 alternate function for UART4. See section 5.1.2 "GPIO Alternate Functions" (Table 5) in the [Raspberry Pi 4 Datasheet](https://datasheets.raspberrypi.com/rpi4/raspberry-pi-4-datasheet.pdf) for the complete alternate function mapping, and the [GPIO pinout documentation](https://www.raspberrypi.com/documentation/computers/raspberry-pi.html#gpio) for the physical pin reference.

#### Serial Settings

Default Modbus RTU serial settings (configurable via `config.toml`):

| Parameter    | Value    |
|--------------|----------|
| Baud rate    | 9600     |
| Data bits    | 8        |
| Parity       | Even     |
| Stop bits    | 1        |
| Flow control | Hardware |

These settings match typical CTC heating system requirements.

#### SmartGrid Relay Control (Optional)

For SmartGrid functionality, a relay module can be connected to control CTC terminals K25/K26:

| RPi Pin | GPIO    | Relay Module | CTC Terminal |
|---------|---------|--------------|--------------|
| Pin 2   | 5V      | VCC          | -            |
| Pin 34  | GND     | GND          | -            |
| Pin 38  | GPIO 20 | IN3          | K25 (Smart A)|
| Pin 40  | GPIO 21 | IN4          | K26 (Smart B)|

See [smartgrid_test/README.md](smartgrid_test/README.md) for complete wiring details, mode reference, and the test tool.

### Installation
1. Clone the repository:
   ```sh
   git clone <repository-url>
   cd ctc_server
   ```
2. Build the project:
   ```sh
   cargo build --release
   ```

#### Cross-Compile for ARM64 (from x86_64)

To build on a development machine for deployment to Raspberry Pi:

```sh
# Install the ARM64 target
rustup target add aarch64-unknown-linux-gnu

# Install the cross-linker (Debian/Ubuntu)
sudo apt install gcc-aarch64-linux-gnu

# Build all workspace members for ARM64
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --release --target aarch64-unknown-linux-gnu
```

Binaries will be in `target/aarch64-unknown-linux-gnu/release/`.

**Using Claude Code container:** If running in the Claude Code development container (which has Zig installed), use `cargo-zigbuild` for simpler cross-compilation:

```sh
# Install cargo-zigbuild (one-time)
cargo install cargo-zigbuild

# Cross-compile using Zig as the linker
cargo zigbuild --target aarch64-unknown-linux-gnu --release
```

### Running the Server
Run the server with the default serial port:
```sh
cargo run --release -p server
```
Or specify a custom serial port:
```sh
cargo run --release -p server -- /dev/ttyUSB0
```

The server will start on `http://0.0.0.0:3000`.

### Docker Deployment

Build and run using Docker Compose:

```sh
# Build the image
docker compose build

# Run in background
docker compose up -d

# View logs
docker compose logs -f

# Stop
docker compose down
```

For Tibber integration, set environment variables before starting:

```sh
export TIBBER_ENABLED=true
export TIBBER_API_TOKEN=your-token-here
docker compose up -d
```

For SmartGrid GPIO control, ensure the container has access to `/dev/gpiochip0` (configured in `docker-compose.yml`).

See `docker-compose.override.yml.example` for local customization options.

## Testing with cURL
### Example Requests
#### Get Room Temperature
```sh
curl -X GET http://localhost:3000/api/v1/temperature/room
```
#### Set Room Temperature Setpoint
```sh
curl -X POST "http://localhost:3000/api/v1/temperature/room/setpoint/?value=22.5"
```
#### Get Outdoor Temperature
```sh
curl -X GET http://localhost:3000/api/v1/temperature/outdoor
```
#### Enable Power-Saving Mode
```sh
curl -X POST http://localhost:3000/api/v1/ctc/powersave \
     -H "Content-Type: application/json" \
     -d '{"enabled": true}'
```

#### Get SmartGrid Mode
```sh
curl -X GET http://localhost:3000/api/v1/smartgrid
```

#### Set SmartGrid Mode
```sh
curl -X POST "http://localhost:3000/api/v1/smartgrid?mode=lowprice"
```

#### Get Alarm Status
```sh
curl -X GET http://localhost:3000/api/v1/alarms/status
```

#### Get Full Alarm Details
```sh
curl -X GET http://localhost:3000/api/v1/alarms
```

## Web Dashboard

The server includes a real-time web dashboard accessible at `http://localhost:3000/`. The dashboard provides:

### Features
- **Temperature Monitoring**: Room, outdoor, DHW, and radiator water temperatures
- **Heat Pump Status**: Compressor state, HP in/out temps, pressures, brine circuit
- **Heat Pump Statistics**: Cycle times (min/max/avg), compressor starts per window, operating hours per window
- **Pump Status**: Charge pump and brine pump percentages with progress bars
- **Power Consumption**: Total power and per-phase current readings
- **System Status**: SmartGrid mode and powersave status in header
- **Alarms & Infos**: Active alarms and info messages with timestamps

### Interactive Controls
- **Powersave Toggle**: Click the powersave badge in the header to enable/disable power saving mode (with confirmation dialog)
- **Statistics Charts**: Click the Heat Pump Statistics panel to view charts (cycles, hours/day, starts/day, temperature correlations)
- Auto-refreshes every 5 seconds
- Responsive design for desktop and mobile

### Dashboard Screenshot
The dashboard displays a dark-themed interface with cards for each system component, real-time updates, and color-coded status indicators.

## Configuration

The server supports configuration via file, environment variables, and CLI arguments (in order of increasing priority).

### Configuration File

Place a `config.toml` in the working directory or specify a path. See `config.toml.example` for all options.

### Environment Variables

All settings can be overridden with environment variables using the `CTC_` prefix:

| Variable | Description | Default |
|----------|-------------|---------|
| `CTC_SERVER_HOST` | Server bind address | `0.0.0.0` |
| `CTC_SERVER_PORT` | Server port | `3000` |
| `CTC_SERIAL_DEFAULT_PORT` | Serial port path | `/dev/ttyAMA4` |
| `CTC_SERIAL_BAUD_RATE` | Baud rate | `9600` |
| `CTC_SERIAL_PARITY` | Parity (none/even/odd) | `even` |
| `CTC_MODBUS_SLAVE_ID` | Modbus slave ID | `1` |
| `CTC_MODBUS_MAX_RETRIES` | Max retry attempts | `2` |
| `CTC_MODBUS_OPERATION_TIMEOUT_SECS` | Operation timeout | `1` |
| `CTC_POWERSAVE` | Initial powersave mode on startup | `false` |
| `CTC_TIBBER_ENABLED` | Enable Tibber integration | `false` |
| `CTC_TIBBER_API_TOKEN` | Tibber API token (from developer.tibber.com) | - |
| `CTC_PRICE_ENABLED` | Enable spot price tracking | `true` |
| `CTC_PRICE_FETCH_INTERVAL_MINS` | Price fetch interval in minutes | `15` |
| `CTC_HEATPUMP_STATS_ENABLED` | Enable heat pump statistics tracking | `true` |
| `CTC_HEATPUMP_STATS_POLL_INTERVAL_SECS` | Compressor status polling interval in seconds | `10` |

### Energy Integration

The server supports energy tracking and pricing from multiple sources:

#### Tibber (WebSocket)
Real-time consumption and pricing when enabled:
1. **Real-time consumption**: Current hour's accumulated kWh from Tibber Pulse
2. **Monthly peak tracking**: Tracks top 3 highest consumption hours during high-tariff periods
3. **Historical sync**: Fetches and processes historical data hourly to catch any missed updates
4. **Price data**: Total price including markup, tax, and price level classification

```bash
# Enable Tibber integration
export CTC_TIBBER_ENABLED=true
export CTC_TIBBER_API_TOKEN=your-token-here
```

Get your API token from [Tibber Developer Portal](https://developer.tibber.com/).

#### elprisetjustnu.se (Spot Prices)
Raw Nord Pool spot prices without authentication:
- **15-minute resolution**: Up to 96 price points per day
- **Next-day prices**: Available after ~13:00 CET
- **Price zones**: SE1 (Luleå), SE2 (Sundsvall), SE3 (Stockholm), SE4 (Malmö)

```bash
# Set price zone (default: SE3)
export CTC_PRICE_ZONE=SE3
```

When both sources are enabled, the server calculates markup comparison between spot and Tibber total prices.

## License
This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Contributing
Contributions are welcome! Please open an issue or submit a pull request.

## Acknowledgments
- [Axum](https://github.com/tokio-rs/axum) for the web framework.
- [Tokio](https://tokio.rs/) for asynchronous runtime.
- [tokio-modbus](https://github.com/slowtec/tokio-modbus) for Modbus RTU support.

