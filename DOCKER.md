# Docker Deployment Guide for CTC Server

This guide explains how to build and deploy the CTC server using Docker on ARM64 platforms (Raspberry Pi, AWS Graviton, Apple Silicon, etc.).

## Table of Contents

- [Prerequisites](#prerequisites)
- [Quick Start](#quick-start)
- [Building the Image](#building-the-image)
- [Running the Container](#running-the-container)
- [Configuration](#configuration)
- [Deployment Scenarios](#deployment-scenarios)
- [Troubleshooting](#troubleshooting)
- [Advanced Topics](#advanced-topics)

---

## Prerequisites

### Required

- **Docker** (version 20.10 or later) with ARM64 support
- **Docker Compose** (version 2.0 or later)
- **ARM64 platform**: Raspberry Pi 3/4/5, AWS Graviton, Apple M1/M2, or ARM64 server
- **Serial device**: Physical CTC heating system connected via serial port

### Optional

- **Docker Buildx** for multi-platform builds (included in Docker Desktop)
- **Serial port**: `/dev/ttyAMA4` (Raspberry Pi UART) or `/dev/ttyUSB0` (USB adapter)

---

## Quick Start

### 1. Clone and Navigate

```bash
cd /path/to/ctc_server
```

### 2. Build and Start

```bash
docker-compose up -d
```

### 3. Check Status

```bash
docker-compose ps
docker-compose logs -f
```

### 4. Test API

```bash
curl http://localhost:3000/api/v1/temperature/outdoor
```

### 5. Stop

```bash
docker-compose down
```

---

## Building the Image

### Standard Build (Native Architecture)

```bash
docker build -t ctc_server:latest .
```

### Using Docker Compose

```bash
docker-compose build
```

### Cross-Platform Build (ARM64 on AMD64)

If you're building on an AMD64/x86_64 machine for deployment on ARM64 hardware (Raspberry Pi, etc.), use Docker buildx:

#### Quick Build (Single Platform)

```bash
docker buildx build --platform linux/arm64 -t ctc_server:latest --load .
```

**Note:** `--load` loads the image into your local Docker for testing with QEMU emulation.

#### Setup buildx Builder (One-Time Setup)

For the first time or if you encounter cross-compilation errors:

```bash
# Check buildx availability
docker buildx version

# Create a multi-architecture builder
docker buildx create --name multiarch --driver docker-container --use

# Bootstrap the builder (downloads QEMU for ARM64 emulation)
docker buildx inspect --bootstrap

# Verify ARM64 support
docker buildx inspect --bootstrap | grep "linux/arm64"
```

#### Build and Push to Registry (Recommended for Production)

```bash
# Build for ARM64 and push to your registry
docker buildx build \
  --platform linux/arm64 \
  -t myregistry/ctc_server:latest \
  --push \
  .

# Or build for multiple platforms
docker buildx build \
  --platform linux/arm64,linux/amd64 \
  -t myregistry/ctc_server:latest \
  --push \
  .
```

**Important:** Multi-platform builds with `--push` upload directly to a registry. You cannot use `--load` with multiple platforms.

#### Using docker-compose with ARM64

Force ARM64 build via environment variable:

```bash
DOCKER_DEFAULT_PLATFORM=linux/arm64 docker-compose build
```

Or temporarily modify `docker-compose.yml`:

```yaml
services:
  ctc_server:
    platform: linux/arm64  # Add this line
```

#### Cross-Compilation Performance

- **First build on AMD64:** 20-30 minutes (QEMU emulation + Rust compilation)
- **Subsequent builds:** 2-5 minutes (cargo-chef caching helps)
- **Native ARM64 build:** 10-15 minutes on Raspberry Pi 4/5

**Tip:** For faster development, build natively on ARM64 hardware when possible, or use a build server.

### Build Time Optimization with cargo-chef

The Dockerfile uses **cargo-chef** for efficient dependency caching:

1. **Planner Stage**: Extracts dependency information
2. **Cacher Stage**: Builds and caches dependencies
3. **Builder Stage**: Compiles application code (reuses cached deps)
4. **Runtime Stage**: Creates minimal production image

**Benefits:**
- First build: ~10-20 minutes (depends on hardware)
- Subsequent builds (code changes only): ~2-5 minutes
- Dependency changes: Only rebuild dependency layer
- Final image size: ~120-150MB (vs ~2GB without multi-stage)

### Verifying Build

```bash
# Check image size
docker images ctc_server

# Inspect layers
docker history ctc_server:latest

# Test image
docker run --rm ctc_server:latest --help
```

---

## Running the Container

### Using Docker Compose (Recommended)

```bash
docker-compose up -d
```

### Using Docker CLI

```bash
docker run -d \
  --name ctc_server \
  --platform linux/arm64 \
  --device=/dev/ttyAMA4:/dev/ttyAMA4 \
  -p 3000:3000 \
  -e RUST_LOG=info \
  -e CTC_SERIAL_DEFAULT_PORT=/dev/ttyAMA4 \
  --restart unless-stopped \
  ctc_server:latest
```

### Override Serial Port

```bash
docker run -d \
  --device=/dev/ttyUSB0:/dev/ttyUSB0 \
  -e CTC_SERIAL_DEFAULT_PORT=/dev/ttyUSB0 \
  ctc_server:latest /dev/ttyUSB0
```

---

## Configuration

### Configuration Priority

1. **CLI arguments** (highest)
2. **Environment variables** (`CTC_*` prefix)
3. **Mounted config.toml**
4. **Built-in defaults** (lowest)

### Environment Variables

All configuration options can be set via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `CTC_SERVER_HOST` | `0.0.0.0` | HTTP bind address |
| `CTC_SERVER_PORT` | `3000` | HTTP port |
| `CTC_SERIAL_DEFAULT_PORT` | `/dev/ttyAMA4` | Serial device path |
| `CTC_SERIAL_BAUD_RATE` | `9600` | Baud rate |
| `CTC_SERIAL_PARITY` | `even` | Parity (none, even, odd) |
| `CTC_SERIAL_DATA_BITS` | `8` | Data bits (5-8) |
| `CTC_SERIAL_STOP_BITS` | `1` | Stop bits (1-2) |
| `CTC_SERIAL_FLOW_CONTROL` | `hardware` | Flow control |
| `CTC_MODBUS_SLAVE_ID` | `1` | Modbus slave ID |
| `CTC_MODBUS_CHANNEL_BUFFER_SIZE` | `24` | Queue size |
| `CTC_POWER_SAVE_LOW_TEMP` | `15.0` | Power save temp |
| `CTC_POWER_SAVE_HIGH_TEMP` | `21.5` | Normal temp |
| `RUST_LOG` | `info` | Log level (trace, debug, info, warn, error) |

### Using Environment Variables

In `docker-compose.yml`:

```yaml
services:
  ctc_server:
    environment:
      - CTC_SERVER_PORT=8080
      - CTC_SERIAL_BAUD_RATE=19200
      - RUST_LOG=debug
```

Or via `.env` file:

```bash
# .env
CTC_SERVER_PORT=8080
CTC_SERIAL_BAUD_RATE=19200
RUST_LOG=debug
```

### Using config.toml

Mount a custom configuration file:

```yaml
services:
  ctc_server:
    volumes:
      - ./config.toml:/app/config.toml:ro
```

See `config.toml.example` for all available options.

---

## Deployment Scenarios

### Scenario 1: Raspberry Pi with UART (Default)

**Hardware**: Raspberry Pi 3/4/5 with CTC device on GPIO UART

```bash
# Enable UART in /boot/config.txt:
# enable_uart=1
# dtoverlay=disable-bt

docker-compose up -d
```

### Scenario 2: USB Serial Adapter

**Hardware**: Any ARM64 device with USB-to-serial adapter

1. Create override file:
   ```bash
   cp docker-compose.override.yml.example docker-compose.override.yml
   ```

2. Edit `docker-compose.override.yml`:
   ```yaml
   services:
     ctc_server:
       devices:
         - /dev/ttyUSB0:/dev/ttyUSB0
       environment:
         - CTC_SERIAL_DEFAULT_PORT=/dev/ttyUSB0
   ```

3. Start:
   ```bash
   docker-compose up -d
   ```

### Scenario 3: Custom Port and Configuration

```bash
# Create custom config
cp config.toml.example config.toml
# Edit config.toml with your settings

# Create override
cat > docker-compose.override.yml << EOF
version: '3.8'
services:
  ctc_server:
    ports:
      - "8080:8080"
    environment:
      - CTC_SERVER_PORT=8080
    volumes:
      - ./config.toml:/app/config.toml:ro
EOF

docker-compose up -d
```

### Scenario 4: Multiple Instances

Run multiple instances with different serial ports:

```bash
# Instance 1 - UART
docker run -d --name ctc1 \
  --device=/dev/ttyAMA4:/dev/ttyAMA4 \
  -p 3001:3000 \
  ctc_server:latest /dev/ttyAMA4

# Instance 2 - USB
docker run -d --name ctc2 \
  --device=/dev/ttyUSB0:/dev/ttyUSB0 \
  -p 3002:3000 \
  ctc_server:latest /dev/ttyUSB0
```

---

## Troubleshooting

### Container Won't Start

**Check logs:**
```bash
docker-compose logs -f
```

**Common issues:**
- Serial device not found
- Permission denied on serial port
- Port 3000 already in use

### Serial Port Permission Denied

**Symptom:**
```
Error: Permission denied (os error 13)
```

**Solution 1: Add user to dialout group on host**
```bash
sudo usermod -aG dialout $USER
# Reboot or logout/login
```

**Solution 2: Set device permissions**
```bash
sudo chmod 666 /dev/ttyAMA4
# or
sudo chmod 666 /dev/ttyUSB0
```

**Solution 3: Use privileged mode (not recommended)**
```yaml
services:
  ctc_server:
    privileged: true
```

### Serial Device Not Found

**Check device exists:**
```bash
ls -l /dev/ttyAMA* /dev/ttyUSB*
```

**Find USB serial devices:**
```bash
dmesg | grep tty
lsusb
```

**Update device in compose:**
```yaml
devices:
  - /dev/ttyUSB0:/dev/ttyUSB0  # Adjust path
environment:
  - CTC_SERIAL_DEFAULT_PORT=/dev/ttyUSB0
```

### Health Check Failing

**Check endpoint directly:**
```bash
docker exec ctc_server curl -f http://localhost:3000/api/v1/temperature/outdoor
```

**Possible causes:**
- Serial port not communicating
- Modbus device not responding
- Wrong Modbus slave ID or baud rate

**Disable health check temporarily:**
```yaml
services:
  ctc_server:
    healthcheck:
      disable: true
```

### Cross-Platform Build Issues

#### Error: "multiple platforms feature is currently not supported"

**Cause:** Default Docker builder doesn't support multi-platform builds

**Solution:** Create a buildx builder (see "Setup buildx Builder" section above)

```bash
docker buildx create --name multiarch --driver docker-container --use
docker buildx inspect --bootstrap
```

#### Error: "exec format error" when running ARM64 image on AMD64

**Cause:** Trying to run ARM64 binary on AMD64 without emulation

**Solutions:**
1. **Use QEMU emulation (automatic with buildx):**
   ```bash
   docker buildx build --platform linux/arm64 -t ctc_server:latest --load .
   docker run --platform linux/arm64 ctc_server:latest
   ```

2. **Push to registry and pull on ARM64 device (recommended):**
   ```bash
   docker buildx build --platform linux/arm64 -t myregistry/ctc_server:latest --push .
   # On Raspberry Pi:
   docker pull myregistry/ctc_server:latest
   ```

#### Cross-Compilation Very Slow

**Expected behavior:** First ARM64 build on AMD64 can take 20-30 minutes due to:
- QEMU emulation overhead
- Rust cross-compilation
- cargo-chef downloading/compiling dependencies

**Optimizations:**
1. **Use cargo-chef caching** (already configured in Dockerfile)
2. **Subsequent builds much faster** (2-5 minutes with code changes only)
3. **Consider building on actual ARM64 hardware** (Raspberry Pi 4/5 with 4GB+ RAM)
4. **Use GitHub Actions or CI with ARM64 runners**

#### Warning: "Requested image's platform does not match host platform"

**This is normal** when building ARM64 on AMD64 with buildx. Docker will use QEMU to emulate ARM64.

To suppress the warning, explicitly set platform:
```bash
docker buildx build --platform linux/arm64 -t ctc_server:latest --load .
```

### Dependency Cache Not Working

**Symptom:** Every build recompiles all dependencies

**Cause:** Source code copied before cargo-chef cook

**Solution:** Ensure Dockerfile stages are correct (check layer order)

**Force rebuild without cache:**
```bash
docker-compose build --no-cache
```

---

## Advanced Topics

### Multi-Platform Builds

Build for both ARM64 and AMD64:

```bash
docker buildx build \
  --platform linux/arm64,linux/amd64 \
  -t myregistry/ctc_server:latest \
  --push \
  .
```

### Pushing to Registry

```bash
# Tag for registry
docker tag ctc_server:latest myregistry/ctc_server:v1.0

# Push
docker push myregistry/ctc_server:v1.0

# Pull on target
docker pull myregistry/ctc_server:v1.0
```

### Viewing Logs

```bash
# Follow logs
docker-compose logs -f

# Last 100 lines
docker-compose logs --tail=100

# Specific service
docker-compose logs ctc_server

# With timestamps
docker-compose logs -f --timestamps
```

### Accessing Container Shell

```bash
# Root shell (for debugging)
docker-compose exec --user root ctc_server /bin/bash

# Non-root shell
docker-compose exec ctc_server /bin/bash

# Run command
docker-compose exec ctc_server ls -la /app
```

### Resource Limits

Limit CPU and memory usage:

```yaml
services:
  ctc_server:
    deploy:
      resources:
        limits:
          cpus: '1.0'
          memory: 512M
        reservations:
          cpus: '0.5'
          memory: 256M
```

### Systemd Integration

Create `/etc/systemd/system/ctc-server.service`:

```ini
[Unit]
Description=CTC Server Docker Container
Requires=docker.service
After=docker.service

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=/path/to/ctc_server
ExecStart=/usr/bin/docker-compose up -d
ExecStop=/usr/bin/docker-compose down
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo systemctl daemon-reload
sudo systemctl enable ctc-server
sudo systemctl start ctc-server
```

---

## Performance Considerations

### Raspberry Pi Optimization

- Use `linux/arm64` platform explicitly
- Enable hardware flow control for serial
- Limit to single-threaded runtime (already configured)
- Monitor CPU/memory with `docker stats`

### Serial Port Latency

- Hardware flow control recommended
- Lower baud rates more reliable on Pi (9600 default)
- USB adapters may have higher latency than GPIO UART

### Network Performance

- Bind to `127.0.0.1` instead of `0.0.0.0` for localhost-only access
- Use reverse proxy (nginx) for TLS termination if needed

---

## Security Considerations

- ✅ Non-root user (`ctc:ctc`) runs the application
- ✅ Minimal runtime image (Debian slim)
- ✅ No secrets in image
- ✅ Read-only config mount recommended
- ⚠️ Avoid `privileged: true` mode
- ⚠️ Only map required serial devices
- ⚠️ Use firewall to restrict port 3000 access

---

## Summary

This Docker setup provides:
- ✅ Optimized ARM64 builds with cargo-chef caching
- ✅ Minimal ~120-150MB production image
- ✅ Flexible configuration via env vars or config file
- ✅ Health checks and restart policies
- ✅ Non-root security model
- ✅ Support for multiple serial devices

For additional help, see the main README.md or open an issue on GitHub.
