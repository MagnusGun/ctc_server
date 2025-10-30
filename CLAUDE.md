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

- **CtcActor** (server/src/routes/ctc_actor.rs:22): Main actor that owns the Modbus RTU context and processes parameter operations sequentially
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

The Axum-based web server provides two route modules:

1. **temperatures.rs** (server/src/routes/temperatures.rs): Temperature monitoring and control endpoints
   - Read room/outdoor/flow temperatures
   - Get/set room temperature setpoint

2. **ctc.rs** (server/src/routes/ctc.rs): Generic parameter access and convenience functions
   - Generic parameter read/write by address
   - Power-save mode (sets vacation days + room temp)

Both modules use the same pattern: create a oneshot channel, send operation to actor via mpsc, await response.

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

Write operations (server/src/routes/ctc_actor.rs:149-170) automatically:
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
- Workspace-level dependencies: tokio, tracing, serde shared across all members

## Important Notes

- The actor loop (server/src/routes/ctc_actor.rs:174-242) runs indefinitely until the receiver channel closes
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

3. **Code Formatting**
   ```bash
   cargo fmt
   ```
   - All code must be formatted with rustfmt

4. **Float Comparisons in Tests**
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

### Pre-Commit Checklist

Before committing, verify:
- [ ] `cargo fmt` - Code is formatted
- [ ] `cargo clippy --all-targets -- -W clippy::pedantic` - Zero warnings
- [ ] `cargo test --all-targets` - All tests pass
- [ ] Commit message follows guidelines (≤50 chars, imperative verb)
