# ctc_server

## Overview
`ctc_server` is a Rust-based server application designed to interact with a CTC heating system via Modbus RTU. It provides a RESTful API for monitoring and controlling various parameters of the heating system, such as room temperature, outdoor temperature, and power-saving modes.

## Features
- Retrieve and set room temperature setpoints.
- Monitor outdoor, flow, and return temperatures.
- Enable or disable power-saving modes.
- Generic Modbus parameter querying.
- Modular and extensible design.

## Project Structure
The project is organized as a Rust workspace with the following structure:
```
ctc_server/
├── server/                   # Main server implementation
│   ├── src/
│   │   ├── main.rs           # Application entry point
│   │   ├── lib.rs            # Library module
│   │   ├── modbus/           # Modbus parameter definitions and utilities
│   │   │   ├── mod.rs        # Core Modbus logic
│   │   │   ├── bms_parameters.rs # Predefined Modbus parameters for the heating system
│   │   ├── routes/           # API route handlers
│   │   │   ├── mod.rs        # Route module definitions
│   │   │   ├── temperatures.rs # Temperature-related endpoints
│   │   │   ├── ctc.rs        # CTC-specific endpoints
│   │   │   ├── ctc_actor.rs  # Actor-based Modbus request handling
│   ├── Cargo.toml            # Server crate dependencies
├── test_client/              # Placeholder for testing client
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
- `GET /api/v1/ctc/`: Query generic Modbus parameters.
- `POST /api/v1/ctc/powersave`: Enable or disable power-saving mode.
- `GET /api/v1/ctc/powersave`: Get the current power-saving state.

## Getting Started
### Prerequisites
- Rust (edition 2024)
- A Modbus-compatible CTC heating system
- Serial connection to the heating system

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

## License
This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Contributing
Contributions are welcome! Please open an issue or submit a pull request.

## Acknowledgments
- [Axum](https://github.com/tokio-rs/axum) for the web framework.
- [Tokio](https://tokio.rs/) for asynchronous runtime.
- [tokio-modbus](https://github.com/slowtec/tokio-modbus) for Modbus RTU support.

