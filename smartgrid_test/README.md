# SmartGrid Relay Test Tool

## Overview

`smartgrid_test` is a command-line tool for testing GPIO relay control of CTC SmartGrid terminals K25/K26. It can:

> **Note**: This is a standalone hardware testing tool that reads CTC Modbus registers
> directly for verification. The `ctc_server` uses GPIO-only for SmartGrid control
> and does not use register 1100.

- Query and display the current SmartGrid mode from the CTC heating system
- Discover relay board configuration (active-high/low, GPIO-to-terminal mapping)
- Cycle through all SmartGrid modes and measure CTC response times

## Prerequisites

- Raspberry Pi 4B with GPIO access
- `ctc_server` running and accessible
- AZDelivery 4-relay module (or compatible optocoupler relay board)
- User in `gpio` group or root access

## Hardware Setup

### Wiring Diagram

Connect the Raspberry Pi to the relay module:

| RPi Pin | Signal   | Relay Module |
|---------|----------|--------------|
| Pin 2   | 5V       | VCC          |
| Pin 34  | GND      | GND          |
| Pin 38  | GPIO 20  | IN3          |
| Pin 40  | GPIO 21  | IN4          |

### CTC Terminal Connections

Connect relay outputs to CTC heating system terminals:

| Relay Output | CTC Terminal | Function |
|--------------|--------------|----------|
| Relay 3 (NO) | K25          | Smart A  |
| Relay 4 (NO) | K26          | Smart B  |

Use the relay's Normally Open (NO) contacts. The exact GPIO-to-terminal mapping is determined by the `discover` command.

### SmartGrid Mode Reference

| Mode         | K25 (Smart A) | K26 (Smart B) | Effect                              |
|--------------|---------------|---------------|-------------------------------------|
| Normal       | Open          | Open          | Standard operation                  |
| Blocking     | Closed        | Open          | Minimize heating (peak prices)      |
| LowPrice     | Open          | Closed        | +1C setpoints (cheap electricity)   |
| Overcapacity | Closed        | Closed        | +2C setpoints (very cheap/negative) |

> The `ctc_server` controls SmartGrid via GPIO pins connected to K25/K26 terminals.
> Register 1100 (virtual digital inputs) is not used as it's not supported on all CTC models.

## Building

### Native Build (on Raspberry Pi)

```bash
cargo build -p smartgrid_test --release
```

The binary will be at `target/release/smartgrid_test`.

### Cross-Compile for ARM64 (from x86_64)

```bash
# Install the ARM64 target
rustup target add aarch64-unknown-linux-gnu

# Install the cross-linker (Debian/Ubuntu)
sudo apt install gcc-aarch64-linux-gnu

# Build for ARM64
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build -p smartgrid_test --release --target aarch64-unknown-linux-gnu
```

The binary will be at `target/aarch64-unknown-linux-gnu/release/smartgrid_test`.

**Using Claude Code container:** If running in the Claude Code development container (which has Zig installed), use `cargo-zigbuild` for simpler cross-compilation:

```bash
# Install cargo-zigbuild (one-time)
cargo install cargo-zigbuild

# Cross-compile using Zig as the linker
cargo zigbuild --target aarch64-unknown-linux-gnu --release -p smartgrid_test
```

## Usage

### Commands

```bash
smartgrid_test status     # Show current SmartGrid mode
smartgrid_test discover   # Detect relay board configuration
smartgrid_test cycle      # Test all mode transitions
```

### Options

```
-s, --server <URL>     CTC server URL [default: http://localhost:3000]
-t, --timeout <SECS>   Timeout for mode changes [default: 30]
```

### Status Command

Query the current SmartGrid mode from the CTC heating system:

```bash
$ smartgrid_test status
Register 1100: 0x0000
SmartGrid Mode: Normal (bits 6-7 = 0b00)
GPIO 20 (Pin 38): HIGH
GPIO 21 (Pin 40): HIGH
```

### Discover Command

Interactively determine the relay board configuration:

```bash
$ smartgrid_test discover

=== SmartGrid Relay Discovery ===

Current register 1100: 0x0000 (Normal)

Step 1: Detecting relay board logic level

Setting GPIO 20 = HIGH, GPIO 21 = HIGH
  Register 1100: Normal after 2s

Setting GPIO 20 = LOW, GPIO 21 = LOW
  Register 1100: Overcapacity after 2s

Result: Relay board is ACTIVE-LOW

Step 2: Mapping GPIOs to CTC terminals

Testing GPIO 20 alone (set to LOW)...
  Register 1100: Blocking after 2s
  GPIO 20 controls K25 (Smart A)

Testing GPIO 21 alone (set to LOW)...
  Register 1100: LowPrice after 2s
  GPIO 21 controls K26 (Smart B)

Resetting to Normal mode...

=== Discovery Complete ===
Relay board: ACTIVE-LOW
GPIO 20 (Pin 38) -> K25 (Smart A)
GPIO 21 (Pin 40) -> K26 (Smart B)
```

### Cycle Command

Test all SmartGrid mode transitions and measure response times:

```bash
$ smartgrid_test cycle

Testing all SmartGrid mode transitions:

Normal -> Blocking: 1.05s OK
Blocking -> LowPrice: 0.98s OK
LowPrice -> Overcapacity: 1.12s OK
Overcapacity -> Normal: 0.95s OK

Average response time: 1.03s
All transitions successful.
```

## Troubleshooting

### Permission denied accessing GPIO

Add your user to the `gpio` group:

```bash
sudo usermod -a -G gpio $USER
# Log out and back in for changes to take effect
```

Or run with sudo (not recommended for regular use):

```bash
sudo ./smartgrid_test status
```

### GPIO device not found

Verify `/dev/gpiochip0` exists:

```bash
ls -la /dev/gpiochip*
```

If missing, ensure you're running on a Raspberry Pi with GPIO support enabled.

### Timeout waiting for mode change

- Verify `ctc_server` is running: `curl http://localhost:3000/api/v1/temperature/outdoor`
- Check CTC heating system has SmartGrid feature enabled
- Increase timeout: `smartgrid_test --timeout 60 cycle`

### No mode change detected during discovery

Check:
- Relay board has power (VCC connected to 5V)
- Relay board wiring to GPIO pins is correct
- Relay outputs are connected to CTC K25/K26 terminals
- CTC SmartGrid feature is enabled in the heating system settings

### Connection refused to server

Ensure `ctc_server` is running and accessible:

```bash
# Check if server is listening
curl http://localhost:3000/api/v1/temperature/outdoor

# Use custom server address if needed
smartgrid_test --server http://192.168.1.100:3000 status
```

## Technical Details

### API Endpoint

This tool queries the CTC server's generic Modbus API to read registers for verification:

- **Register 62301 (SGMode)**: Primary status register showing current SmartGrid mode
- **Register 1100**: Control register (for reference, not supported on all models)
- **Register 62017**: Heat pump status

> **Note**: The `ctc_server` itself uses GPIO-only for SmartGrid control.
> This tool reads the raw Modbus registers to verify hardware behavior.

### GPIO Control

Uses the Linux GPIO character device interface (`/dev/gpiochip0`) via the `gpiocdev` crate. This is the modern approach replacing the deprecated sysfs interface.

### Relay Board Compatibility

Most optocoupler relay modules (like AZDelivery) are **active-low**: the relay energizes when the GPIO output is LOW. The `discover` command automatically detects this.
