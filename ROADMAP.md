# CTC Server - Development Roadmap

**Document Version**: 1.4
**Last Updated**: 2025-11-01 (CRITICAL: Production incident - actor deadlock, created Phase 1.5)
**Project Status**: 🔴 **INCIDENT MODE** - Phase 1 ✅ Complete, Phase 2B ✅ Complete, **Phase 1.5 URGENT**

## Table of Contents

- [INCIDENT ALERT](#incident-alert-2025-11-01) - 🔴 **CRITICAL**
- [Current State Assessment](#current-state-assessment)
- [Phase 0: Foundation](#phase-0-foundation-complete)
- [Phase 1: Critical Fixes (Week 1)](#phase-1-critical-fixes-week-1) - ✅ **COMPLETE**
- [Infrastructure Improvements](#infrastructure-improvements-2025-10-30) - ✅ **COMPLETE**
- [Phase 1.5: Production Stability (URGENT)](#phase-15-production-stability-urgent) - 🔴 **BLOCKING**
- [Phase 2: High Priority Improvements (Week 2)](#phase-2-high-priority-improvements-week-2)
- [Phase 3: Medium Priority Enhancements (Weeks 3-4)](#phase-3-medium-priority-enhancements-weeks-3-4)
- [Phase 4: Low Priority Polish (Weeks 5-8)](#phase-4-low-priority-polish-weeks-5-8)
- [Phase 5: Advanced Features (Future)](#phase-5-advanced-features-future)
- [Completed Items](#completed-items)

---

## INCIDENT ALERT (2025-11-01)

🔴 **CRITICAL PRODUCTION INCIDENT**

**Status**: Service Down - Actor Deadlock
**Impact**: 100% service degradation for ~15 hours
**Root Cause**: No timeout on Modbus operations; actor hung indefinitely
**Action Required**: Implement Phase 1.5 before service restart

### Incident Summary

On 2025-10-31 at 17:19:14 UTC, the actor became permanently deadlocked when a Modbus read operation failed to return a response. This caused all subsequent API requests to hang indefinitely. The service remained in this state for approximately 15 hours.

**Key Details**:
- Actor stuck waiting for `read_holding_registers(62000, 1)` response
- No timeout configured on Modbus operations
- No timeout configured on actor request/response channels
- All write operations since incident hung indefinitely
- 194 operations succeeded before deadlock; 0 completed after

**See**: [INCIDENT_2025-11-01_ACTOR_DEADLOCK.md](./INCIDENT_2025-11-01_ACTOR_DEADLOCK.md) for complete analysis

### Immediate Actions Required

🚨 **DO NOT RESTART SERVICE WITHOUT IMPLEMENTING PHASE 1.5** 🚨

The issue will recur immediately if the serial device is still unresponsive.

**Required Before Restart**:
1. ✅ Phase 1.5A: Implement Modbus operation timeout (1 hour)
2. ✅ Phase 1.5B: Implement request/response timeout (30 min)
3. ✅ Phase 1.5C: Add basic error recovery (1 hour)

**Estimated Time to Safe Restart**: 2-3 hours

---

## Current State Assessment

### ✅ Strengths

- **Architecture**: Clean actor pattern for serial port access
- **Data Model**: Accurate BMS parameter definitions matching official documentation
- **Code Quality**: Well-structured modules, good separation of concerns
- **Testing**: Comprehensive unit tests for value conversion logic
- **Documentation**: Excellent CLAUDE.md for development context

### ⚠️ Issues Identified

**Critical** (0 issues remaining - All ✅ Complete):
- ~~Massive code duplication (~200 lines) in route handlers~~ ✅ Fixed
- ~~No configuration management (10+ hard-coded values)~~ ✅ Fixed
- ~~String-based error handling (no type safety)~~ ✅ Fixed

**High** (5 issues remaining):
- ~~Liberal use of `.unwrap()` (16+ occurrences)~~ ✅ Complete (0 remaining)
- No error recovery in actor
- No request/response timeout
- ~~Large functions needing refactoring~~ ✅ Mostly fixed (set_power_save improved)
- Manual JSON formatting
- Missing integration tests

**Medium** (10 issues):
- Actor in wrong module location
- No caching for read operations
- No rate limiting
- Limited observability
- Missing graceful shutdown
- And others...

**Low** (6 issues):
- Missing newtypes
- Documentation improvements
- Various minor issues

### 📊 Code Metrics

| Metric | Current | Previous | Change |
|--------|---------|----------|---------|
| Total Lines of Code | ~2,200+ | ~966 | +127% |
| Test Coverage | ~50-60% | ~30% | +20-30% |
| Unit Tests | 47 passing | 19 | +28 tests |
| BMS Parameters Implemented | 44 / 200+ | 44 / 200+ | - |
| Hard-Coded Values | 0 | 10+ | ✅ -10+ |
| Code Duplication | ~5% | ~20% | ✅ -15% |
| `.unwrap()` Calls | 0 | 16+ | ✅ -16 (100%) |

---

## Phase 0: Foundation [COMPLETE]

### Documentation Skills

**Status**: ✅ Complete (2025-10-30)

| Task | Status | Files |
|------|--------|-------|
| Create ctc-modbus-protocol.md skill | ✅ Complete | `.claude/skills/ctc-modbus-protocol.md` |
| Create ctc-bms-datamodel.md skill | ✅ Complete | `.claude/skills/ctc-bms-datamodel.md` |
| Create ctc-ecoheat400-configuration.md skill | ✅ Complete | `.claude/skills/ctc-ecoheat400-configuration.md` |
| Review implementation vs BMS specs | ✅ Complete | - |
| Architecture analysis | ✅ Complete | - |
| Create ROADMAP.md | ✅ Complete | This file |

---

## Phase 1: Critical Fixes (Week 1) [✅ COMPLETE]

**Priority**: 🔴 Critical
**Estimated Effort**: 2-3 days
**Actual Effort**: ~2 days (all tasks completed)
**Status**: ✅ **COMPLETE** (2025-10-30)
**Dependencies**: None

### 1A. Fix Step Validation Bug

**Status**: ✅ Complete (2025-10-30)
**Priority**: Critical
**Effort**: 15 minutes
**Registers Affected**: All writable parameters

**Problem**: Validation checks `raw_value % step != 0` which fails when min value is not a multiple of step.

**Fix Applied**:
- File: `server/src/routes/ctc_actor.rs:159`
- Changed from: `!raw_value.is_multiple_of(step)`
- Changed to: `!(raw_value - min).is_multiple_of(step)`
- Added comment explaining validation logic
- Applied clippy suggestion to use `.is_multiple_of()` method

**Testing**:
- ✅ Code compiles successfully
- ✅ Clippy passes (no warnings related to this change)
- ✅ Added 3 unit tests in `server/src/modbus/mod.rs`:
  - `test_step_validation_from_minimum`: Tests the bugfix scenario (min=152, step=5)
  - `test_step_validation_from_zero`: Tests original logic still works (min=0)
  - `test_step_validation_various_scenarios`: Tests various min/step combinations
- ✅ All 19 unit tests pass

---

### 1B. Fix API Typo

**Status**: ✅ Complete (2025-10-30)
**Priority**: Critical
**Effort**: 5 minutes

**Problem**: Typo in JSON response field name

**Fix Applied**:
- File: `server/src/routes/temperatures.rs:55, 87`
- Changed: `"room_temperarure_setpoint"` → `"room_temperature_setpoint"`
- Fixed in both GET and POST endpoint responses

**Testing**: ✅ Code compiles successfully

---

### 1C. Eliminate Code Duplication

**Status**: ✅ Complete (2025-10-30)
**Priority**: Critical
**Effort**: 2-4 hours

**Problem**: Every API endpoint repeats 15-line request/response pattern (~200 lines of duplicate code)

**Fix Applied**:
1. Created `server/src/helpers.rs` module with three helper functions:
   - `read_parameter()` - Generic read with JSON response
   - `read_parameter_value()` - Generic read returning raw value
   - `write_parameter()` - Generic write with JSON response
2. Refactored `server/src/routes/temperatures.rs`:
   - 6 endpoints reduced from ~120 lines to ~40 lines
   - All functions now 1-3 lines instead of 15-20 lines
3. Refactored `server/src/routes/ctc.rs`:
   - 4 endpoints reduced from ~150 lines to ~30 lines
   - `set_power_save()` reduced from 80+ lines to 15 lines
   - `get_power_save()` reduced from 40+ lines to 5 lines
4. Total reduction: ~200 lines eliminated

**Files Affected**:
- New: `server/src/helpers.rs` (130 lines)
- Modified: `server/src/routes/ctc.rs` (90 lines, was ~180 lines)
- Modified: `server/src/routes/temperatures.rs` (57 lines, was ~167 lines)
- Modified: `server/src/main.rs` (added helpers module)

**Testing**:
- ✅ Code compiles successfully
- ✅ All 19 unit tests pass
- ✅ Zero clippy warnings (pedantic mode)
- ✅ All imports cleaned up

---

### 1D. Add Configuration Management

**Status**: ✅ Complete (2025-10-30)
**Priority**: Critical
**Effort**: 4-6 hours (actual)

**Problem**: 10+ hard-coded values throughout codebase

**Current Hard-Coded Values**:
| Value | Location | Type |
|-------|----------|------|
| `0.0.0.0:3000` | main.rs:56 | Server address |
| `/dev/ttyAMA4` | main.rs:11 | Serial port |
| `9600` | main.rs:38 | Baud rate |
| `15.0` | ctc.rs:115 | Power-save temp |
| `21.5` | ctc.rs:155 | Normal temp |
| `2.0` | ctc.rs:135 | Vacation days |
| `5.0-30.0` | temperatures.rs:75 | Temp range |
| `24` | main.rs:35 | Channel buffer |
| `Slave(1)` | ctc_actor.rs:103 | Modbus slave ID |

**Fix Applied**:
1. ✅ Added `config` crate dependency
2. ✅ Created `server/src/config.rs` module (264 lines) with structs:
   - `Config` (root)
   - `ServerConfig` (host, port)
   - `SerialConfig` (port, baud_rate, timeout, flow_control, data_bits, parity, stop_bits)
   - `ModbusConfig` (slave_id, channel_buffer_size, request_timeout_secs)
   - `PowerSaveConfig` (low_temp, high_temp, low_days, high_days)
   - `TemperatureValidationConfig` (min, max)
3. ✅ Created `config.toml.example` with comprehensive documentation
4. ✅ Configuration loading: defaults → config.toml → env vars → CLI args
5. ✅ All hard-coded values eliminated and moved to config system

**Files Affected**:
- New: `server/src/config.rs` (264 lines)
- New: `config.toml.example` (comprehensive example)
- Modified: `server/src/main.rs` (integrated config loading)
- Modified: `server/src/routes/ctc_actor.rs` (uses config)
- Modified: `server/src/routes/ctc.rs` (uses config)
- Modified: `server/src/routes/temperatures.rs` (uses config)
- Modified: `Cargo.toml` (added config dependencies)

**Testing**:
- ✅ 3 unit tests added for config loading and validation
- ✅ All 47 unit tests pass
- ✅ Config loads from file with defaults
- ✅ Environment variable override works
- ✅ CLI argument override works

---

### 1E. Implement Proper Error Types

**Status**: ✅ Complete (2025-10-30)
**Priority**: Critical
**Effort**: 3-5 hours (actual)

**Problem**: All errors are `String` types - no type safety, context, or structured information

**Fix Applied**:
1. ✅ Created `server/src/error.rs` module (444 lines)
2. ✅ Implemented comprehensive error types (manual implementation instead of `thiserror`):
   - `ModbusError` enum with 8 variants:
     - `ReadError(u16, String)` - Failed to read parameter
     - `WriteError(u16, f32, String)` - Failed to write parameter
     - `ReadOnly(u16)` - Parameter is read-only
     - `OutOfRange(f32, f32, f32)` - Value outside valid range
     - `InvalidStep(f32, f32, f32, f32)` - Value doesn't match step constraint
     - `ValidationReadError(u16, String)` - Failed to read validation constraints
     - `SerialError(String)` - Serial port error
     - `ProtocolError(String)` - Modbus protocol error
     - `VerificationError(u16, f32, f32)` - Write verification failed
   - `ApiError` enum with 4 variants:
     - `BadRequest(String)` - Invalid request (400)
     - `InternalError(String)` - Internal server error (500)
     - `ServiceUnavailable` - Service unavailable (503)
     - `Timeout` - Request timeout (408)
3. ✅ Implemented `IntoResponse` for `ApiError` returning proper HTTP status codes
4. ✅ Implemented `From<ModbusError>` for `ApiError` for error conversion
5. ✅ Replaced all `String` errors throughout codebase
6. ✅ Error logging with internal details logged but minimal exposure to clients

**Implementation Note**: Used manual `Display` and `Error` trait implementations instead of `thiserror` crate for more explicit control over error formatting and conversion logic.

**Files Affected**:
- New: `server/src/error.rs` (444 lines)
- Modified: All route files (use typed errors)
- Modified: `server/src/routes/ctc_actor.rs` (returns ModbusError)
- Modified: `server/src/helpers.rs` (uses ApiError)

**Testing**:
- ✅ 24 comprehensive unit tests covering all error variants
- ✅ Tests for error Display implementations
- ✅ Tests for HTTP status code mapping
- ✅ Tests for error conversions
- ✅ All 47 unit tests pass

---

## Infrastructure Improvements (2025-10-30)

The following infrastructure improvements were implemented alongside Phase 1, significantly enhancing deployment, security, and maintainability:

### Docker Support ✅ Complete

**Status**: ✅ Complete (2025-10-30)
**Effort**: 6-8 hours

**Implementation**:
- Multi-stage Dockerfile (95 lines) with cargo-chef optimization for faster builds
- ARM64 platform support for Raspberry Pi and similar devices
- docker-compose.yml (48 lines) with full service configuration
- docker-compose.override.yml.example for local development customization
- .dockerignore (42 lines) for efficient build context
- Non-root user execution with `dialout` group membership for serial port access
- Health check endpoints configured
- Environment variable configuration support
- Volume mounting for custom config files

**Files Added**:
- `Dockerfile` - Multi-stage build (planner, cacher, builder, runtime)
- `docker-compose.yml` - Production-ready compose configuration
- `docker-compose.override.yml.example` - Local development template
- `.dockerignore` - Build context optimization
- `DOCKER.md` (665 lines) - Comprehensive deployment guide covering:
  - Quick start guide
  - Configuration options
  - Serial port setup
  - Development workflow
  - Troubleshooting
  - Production deployment best practices

**Benefits**:
- Simplified deployment across different platforms
- Consistent runtime environment
- Easy configuration management via environment variables
- Reduced build times with cargo-chef caching
- Security hardening with non-root user

---

### Security Tooling ✅ Complete

**Status**: ✅ Complete (2025-10-30)
**Effort**: 1-2 hours

**Implementation**:
- cargo-deny configuration (deny.toml, 31 lines) for comprehensive dependency auditing
- License compliance enforcement with strict allowlist
- Security advisory scanning integrated into development workflow
- Copyleft license detection and rejection

**Configuration**:
- **Advisories**: Enabled security vulnerability scanning via RustSec Advisory Database
- **Licenses**: Strict allowlist enforcement:
  - Allowed: MIT, Apache-2.0, BSD-3-Clause, ISC, Unicode-DFS-2016, Zlib, MPL-2.0
  - Rejected: GPL, LGPL, AGPL (copyleft licenses)
- **Bans**: Warns on multiple versions of same dependency
- **Sources**: Denies unknown registries and git sources (security best practice)

**Files Added**:
- `deny.toml` - cargo-deny configuration with security policies
- `.claude/skills/git-commit-msg.md` (92 lines) - Commit message guidelines

**Benefits**:
- Automated security vulnerability detection
- License compliance verification
- Prevention of copyleft license contamination
- Supply chain security improvements
- Consistent commit message standards

---

## Phase 1.5: Production Stability (URGENT)

🔴 **CRITICAL - BLOCKING SERVICE RESTART**

**Priority**: 🔴 **URGENT - PRODUCTION DOWN**
**Estimated Effort**: 2-3 hours
**Status**: ⚠️ **REQUIRED IMMEDIATELY**
**Dependencies**: None
**Trigger**: Production incident 2025-11-01 - Actor deadlock

**Background**: On 2025-11-01, production service experienced complete deadlock when a Modbus read operation hung indefinitely. Actor became permanently blocked, causing all subsequent requests to hang. Service was down for ~15 hours. These fixes are **mandatory** before service can be safely restarted.

---

### 1.5A. Implement Modbus Operation Timeout

**Status**: ⏸️ Not Started - **BLOCKING RESTART**
**Priority**: 🔴 CRITICAL
**Effort**: 1 hour

**Problem**: Modbus read/write operations have no timeout. When heating system stops responding, actor blocks forever.

**Solution**:
1. Wrap all `read_holding_registers()` calls with `tokio::time::timeout()`
2. Wrap all `write_single_register()` calls with `tokio::time::timeout()`
3. Add to `ModbusConfig`:
   ```rust
   operation_timeout_secs: u64  // default: 5
   ```
4. Implementation in `server/src/routes/ctc_actor.rs`:
   ```rust
   let result = tokio::time::timeout(
       Duration::from_secs(config.modbus.operation_timeout_secs),
       self.ctx.read_holding_registers(param.id, 1)
   ).await;

   match result {
       Ok(Ok(values)) => { /* success */ },
       Ok(Err(e)) => { /* modbus error */ },
       Err(_) => { /* timeout error */ },
   }
   ```

**Files Affected**:
- Modified: `server/src/routes/ctc_actor.rs` (add timeout to read_parameter, write_parameter, read_min_max)
- Modified: `server/src/config.rs` (add operation_timeout_secs)
- Modified: `server/src/error.rs` (add Timeout variant to ModbusError if needed)
- Modified: `config.toml.example` (document timeout setting)

**Testing**:
- Verify normal operations still work
- Simulate timeout by disconnecting serial cable
- Verify timeout error returned in ~5 seconds
- Verify actor continues processing subsequent requests after timeout

**Acceptance Criteria**:
- [ ] All Modbus operations have timeout wrapper
- [ ] Timeout configurable via config.toml
- [ ] Timeout error returns proper ModbusError
- [ ] Actor recovers and processes next message after timeout

---

### 1.5B. Implement Request/Response Timeout

**Status**: ⏸️ Not Started - **BLOCKING RESTART**
**Priority**: 🔴 CRITICAL
**Effort**: 30 minutes

**Problem**: HTTP handlers await actor response indefinitely. If actor hangs, all handlers hang.

**Solution**:
1. Wrap all `response_rx.await` calls with `tokio::time::timeout()`
2. Add to `ModbusConfig`:
   ```rust
   request_timeout_secs: u64  // default: 10 (2x operation timeout)
   ```
3. Implementation in `server/src/helpers.rs`:
   ```rust
   let result = tokio::time::timeout(
       Duration::from_secs(config.modbus.request_timeout_secs),
       response_rx
   ).await;

   match result {
       Ok(Ok(Ok(value))) => Ok(value),
       Ok(Ok(Err(e))) => Err(ApiError::from(e)),
       Ok(Err(_)) => Err(ApiError::ServiceUnavailable),
       Err(_) => Err(ApiError::Timeout),
   }
   ```

**Files Affected**:
- Modified: `server/src/helpers.rs` (all 3 helper functions)
- Modified: `server/src/config.rs` (add request_timeout_secs)
- Modified: `server/src/error.rs` (ensure Timeout variant exists in ApiError)

**Testing**:
- Verify normal operations complete quickly
- Verify timeout returns 408 Request Timeout
- Verify client receives error instead of hanging

**Acceptance Criteria**:
- [ ] All helper functions have timeout on response_rx.await
- [ ] Timeout returns ApiError::Timeout (HTTP 408)
- [ ] No HTTP handler can hang indefinitely

---

### 1.5C. Add Basic Error Recovery

**Status**: ⏸️ Not Started - **HIGHLY RECOMMENDED**
**Priority**: 🟠 HIGH
**Effort**: 1 hour

**Problem**: Single timeout failure leaves system in degraded state. No automatic recovery.

**Solution**:
1. Track consecutive failure count in actor
2. After N consecutive failures (default: 3), log warning
3. Implement simple backoff: small delay between retries
4. Add to logging when errors occur:
   ```rust
   error!("Modbus operation failed: {e} (consecutive failures: {count})");
   ```

**Implementation**:
```rust
struct CtcActor {
    ctx: Context,
    config: Arc<Config>,
    consecutive_failures: u32,
    last_success: Option<Instant>,
}

// In read_parameter / write_parameter:
match modbus_operation().await {
    Ok(value) => {
        self.consecutive_failures = 0;
        self.last_success = Some(Instant::now());
        Ok(value)
    }
    Err(e) => {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= 3 {
            error!("Multiple consecutive Modbus failures: {}", self.consecutive_failures);
        }
        Err(e)
    }
}
```

**Files Affected**:
- Modified: `server/src/routes/ctc_actor.rs` (add failure tracking)
- Modified: `server/src/config.rs` (add max_consecutive_failures config if needed)

**Testing**:
- Verify failure count increments on errors
- Verify count resets on success
- Verify warning logged after 3 failures

**Acceptance Criteria**:
- [ ] Consecutive failures tracked
- [ ] Error logging improves debuggability
- [ ] System logs warnings for persistent issues

---

### 1.5D. Add Health Check Endpoint (Optional but Recommended)

**Status**: ⏸️ Not Started - **RECOMMENDED**
**Priority**: 🟡 MEDIUM
**Effort**: 30 minutes

**Problem**: No way to monitor actor health externally. Deadlock invisible until tested.

**Solution**:
1. Add `/health` endpoint
2. Include last successful operation timestamp
3. Include consecutive failure count
4. Return degraded status if failures > threshold

**Implementation**:
```rust
#[derive(Serialize)]
struct HealthResponse {
    status: String,  // "healthy", "degraded", "unhealthy"
    actor_responsive: bool,
    last_success_secs_ago: Option<u64>,
    consecutive_failures: u32,
}

async fn health_check() -> Json<HealthResponse> {
    // Query actor state via separate health channel
    // or return cached metrics
}
```

**Files Affected**:
- Modified: `server/src/main.rs` (add /health route)
- New: `server/src/routes/health.rs` (health endpoint)
- Modified: `server/src/routes/ctc_actor.rs` (expose health metrics)

**Testing**:
- Verify /health returns 200 when healthy
- Verify status changes when failures occur

**Acceptance Criteria**:
- [ ] /health endpoint returns JSON status
- [ ] External monitoring can detect issues
- [ ] Response includes useful metrics

---

## Phase 2: High Priority Improvements (Week 2)

**Priority**: 🟠 High
**Estimated Effort**: 3-5 days
**Dependencies**: Phase 1 complete, **Phase 1.5 complete**

### 2A. Add Configurable Retry Logic

**Status**: ⏸️ Not Started
**Priority**: High
**Effort**: 2-3 hours

**Problem**: No automatic retry for transient Modbus communication errors

**Solution**:
1. Add to `RetryConfig` in config.rs:
   - `max_retries: u32` (default: 3)
   - `initial_delay_ms: u64` (default: 100)
   - `backoff_multiplier: f64` (default: 2.0)
2. Implement retry wrapper in `server/src/helpers.rs`:
   ```rust
   async fn with_retry<F, T>(
       config: &RetryConfig,
       operation: F
   ) -> Result<T, ModbusError>
   where
       F: Fn() -> Future<Output = Result<T, ModbusError>>
   ```
3. Apply to actor read/write operations
4. Add metrics for retry attempts

**Files Affected**:
- Modified: `server/src/config.rs`
- Modified: `server/src/helpers.rs`
- Modified: `server/src/routes/ctc_actor.rs`
- Modified: `config.toml` (add retry section)

**Testing**:
- Simulate transient errors
- Verify exponential backoff
- Verify max retries respected

---

### 2B. Replace `.unwrap()` Calls

**Status**: ✅ Complete (2025-10-30)
**Priority**: High
**Effort**: 1-2 hours (actual: 10 minutes)

**Problem**: 16+ `.unwrap()` calls that can cause panics

**Fix Applied**:
- ✅ Eliminated ALL 16+ `.unwrap()` calls from production code
- ✅ Fixed all route handlers
- ✅ Fixed main.rs server bind/serve
- ✅ Fixed all 3 helper functions in `server/src/helpers.rs`:
  - Line 26: `read_parameter_value()` - replaced with `.map_err(|_| ApiError::ServiceUnavailable)?`
  - Line 69: `read_parameter()` - replaced with `.map_err(|_| ApiError::ServiceUnavailable)?`
  - Line 118: `write_parameter()` - replaced with `.map_err(|_| ApiError::ServiceUnavailable)?`

**Implementation**:
All channel send operations now use proper error handling:
```rust
// Before
tx.send(msg).await.unwrap();

// After
tx.send(msg).await.map_err(|_| ApiError::ServiceUnavailable)?;
```

**Benefits**:
- Zero `.unwrap()` calls in production code
- If actor crashes, functions return `503 Service Unavailable` instead of panicking
- Production-hardened error handling throughout

**Files Affected**:
- `server/src/helpers.rs` (3 fixes)

**Testing**:
- ✅ All 47 tests pass
- ✅ Zero clippy warnings (pedantic mode)
- ✅ Code compiles successfully

---

### 2C. Add Request/Response Timeout

**Status**: ⚠️ **MOVED TO PHASE 1.5A & 1.5B** (Production Incident)
**Priority**: 🔴 CRITICAL (was High)
**Effort**: 1-2 hours

**NOTE**: This task has been moved to Phase 1.5 due to production incident on 2025-11-01. See Phase 1.5A (Modbus Operation Timeout) and Phase 1.5B (Request/Response Timeout).

**Problem**: Hung Modbus operations block all subsequent requests indefinitely

**Solution**:
1. Add `request_timeout_secs` to `ModbusConfig` (default: 5)
2. Wrap actor operations in `tokio::time::timeout()`
3. Return timeout error if exceeded
4. Example:
   ```rust
   match tokio::time::timeout(
       Duration::from_secs(config.modbus.request_timeout_secs),
       response_rx
   ).await {
       Ok(Ok(result)) => Ok(result),
       Ok(Err(e)) => Err(ApiError::Modbus(e)),
       Err(_) => Err(ApiError::Timeout),
   }
   ```

**Files Affected**:
- Modified: `server/src/config.rs`
- Modified: `server/src/helpers.rs`
- Modified: Route handler helper functions

**Testing**:
- Simulate slow Modbus responses
- Verify timeout occurs
- Verify other requests can proceed

---

### 2D. Implement JSON Response Types

**Status**: ⏸️ Not Started
**Priority**: High
**Effort**: 2-3 hours

**Problem**: Manual JSON string formatting is error-prone and not type-safe

**Solution**:
1. Define response structs with `serde::Serialize`:
   ```rust
   #[derive(Serialize)]
   pub struct TemperatureResponse {
       pub room_temperature: f32,
       pub timestamp: i64,
   }

   #[derive(Serialize)]
   pub struct ParameterResponse {
       pub data: f32,
       pub parameter_id: u16,
       pub timestamp: i64,
   }
   ```
2. Replace `Ok(format!(...))` with `Ok(Json(...))`
3. Use `axum::Json<T>` for type-safe responses
4. Add timestamp to all responses

**Files Affected**:
- New: `server/src/response.rs` (response types)
- Modified: All route files
- Modified: `server/src/lib.rs`

**Testing**: Verify JSON structure and content-type headers

---

### 2E. Refactor `set_power_save` Function

**Status**: ⏸️ Not Started
**Priority**: High
**Effort**: 1-2 hours

**Problem**: 89-line function with duplicated logic

**Current**: Lines 107-195 in ctc.rs

**Solution**:
1. Extract helper functions:
   ```rust
   async fn set_power_save_mode(
       tx: &ModbusSender,
       config: &PowerSaveConfig
   ) -> Result<(), ApiError>

   async fn set_normal_mode(
       tx: &ModbusSender,
       config: &PowerSaveConfig
   ) -> Result<(), ApiError>
   ```
2. Simplify main function to 20-30 lines
3. Use config values instead of hard-coded numbers

**Files Affected**:
- Modified: `server/src/routes/ctc.rs`

**Testing**: Verify power save on/off still works correctly

---

### 2F. Add Error Recovery in Actor

**Status**: ⚠️ **MOVED TO PHASE 1.5C** (Production Incident)
**Priority**: 🔴 CRITICAL (was High)
**Effort**: 3-4 hours

**NOTE**: This task has been partially moved to Phase 1.5C (Basic Error Recovery) due to production incident on 2025-11-01. Phase 1.5C implements basic failure tracking and logging. Full circuit breaker pattern remains here as Phase 2F.

**Problem**: Actor crashes on Modbus errors, no automatic recovery

**Solution**:
1. Add connection health monitoring
2. Implement automatic serial port reconnection:
   ```rust
   async fn reconnect(&mut self) -> Result<(), ModbusError> {
       // Close existing connection
       // Wait with backoff
       // Attempt new connection
       // Update health status
   }
   ```
3. Add circuit breaker pattern:
   - Track failure count
   - Open circuit after N consecutive failures
   - Half-open after timeout to test
   - Close on success
4. Catch panics in actor loop
5. Return to processing after recovery

**Files Affected**:
- Modified: `server/src/routes/ctc_actor.rs`
- Modified: `server/src/config.rs` (add recovery config)

**Testing**:
- Simulate serial port disconnect
- Verify reconnection attempts
- Verify circuit breaker triggers

---

## Phase 3: Medium Priority Enhancements (Weeks 3-4)

**Priority**: 🟡 Medium
**Estimated Effort**: 2-3 weeks
**Dependencies**: Phases 1-2 complete

### 3A. Reorganize Module Structure

**Status**: ⏸️ Not Started
**Priority**: Medium
**Effort**: 1 hour

**Problem**: Actor implementation in `routes/` module (wrong layer)

**Solution**:
1. Create `server/src/actor/` directory
2. Move `ctc_actor.rs` → `server/src/actor/mod.rs`
3. Create `server/src/api/` directory
4. Move `routes/ctc.rs` → `api/ctc.rs`
5. Move `routes/temperatures.rs` → `api/temperatures.rs`
6. Update all imports and module declarations

**New Structure**:
```
server/src/
├── actor/
│   └── mod.rs (was routes/ctc_actor.rs)
├── api/
│   ├── mod.rs
│   ├── ctc.rs (was routes/ctc.rs)
│   └── temperatures.rs (was routes/temperatures.rs)
├── config.rs
├── error.rs
├── helpers.rs
├── modbus/
│   ├── mod.rs
│   └── bms_parameters.rs
├── lib.rs
└── main.rs
```

**Files Affected**: Multiple (pure refactoring)

**Testing**: Verify compilation and all endpoints work

---

### 3B. Implement Caching for Read Operations

**Status**: ⏸️ Not Started
**Priority**: Medium
**Effort**: 3-4 hours

**Problem**: Every API call triggers Modbus read, unnecessary for slowly-changing values

**Solution**:
1. Create `server/src/cache.rs` module
2. Implement TTL-based cache:
   ```rust
   struct ParameterCache {
       entries: HashMap<u16, CacheEntry>,
   }

   struct CacheEntry {
       value: f32,
       timestamp: Instant,
       ttl: Duration,
   }
   ```
3. Add cache configuration:
   - `cache_enabled: bool`
   - `default_ttl_secs: u64`
   - Parameter-specific TTLs (fast-changing vs slow-changing)
4. Integrate into actor read path
5. Add cache hit/miss metrics

**Cache TTL Suggestions**:
- Outdoor temperature: 30 seconds
- Room temperature: 10 seconds
- Status registers: 5 seconds
- Configuration parameters: 60 seconds
- Heat pump sensors: 10 seconds

**Files Affected**:
- New: `server/src/cache.rs`
- Modified: `server/src/actor/mod.rs`
- Modified: `server/src/config.rs`

**Testing**:
- Verify cache hits reduce Modbus calls
- Verify cache expiration works
- Verify cache can be disabled

---

### 3C. Add Rate Limiting

**Status**: ⏸️ Not Started
**Priority**: Medium
**Effort**: 1-2 hours

**Problem**: No protection against request floods

**Solution**:
1. Add `tower-http` dependency
2. Implement rate limiting middleware
3. Configuration:
   ```rust
   [api.rate_limit]
   enabled = true
   requests_per_minute = 20
   burst = 5
   ```
4. Apply to all API routes
5. Return 429 Too Many Requests when exceeded

**Files Affected**:
- Modified: `server/src/main.rs`
- Modified: `server/src/config.rs`
- Modified: `Cargo.toml`

**Testing**:
- Send burst of requests
- Verify rate limiting triggers
- Verify limit resets after time window

---

### 3D. Add Comprehensive Integration Tests

**Status**: ⏸️ Not Started
**Priority**: Medium
**Effort**: 8-12 hours

**Problem**: Only unit tests for value conversion, no integration tests

**Solution**:
1. Create `server/tests/` directory
2. Implement mock Modbus context for testing
3. Test suites:
   - `api_tests.rs` - All API endpoints
   - `actor_tests.rs` - Actor behavior (read, write, error handling)
   - `retry_tests.rs` - Retry logic
   - `timeout_tests.rs` - Timeout handling
   - `config_tests.rs` - Configuration loading
4. Use `tokio::test` for async tests
5. Aim for 80%+ integration test coverage

**Files Affected**:
- New: `server/tests/*`
- May need: Mockable trait for Modbus context

**Testing**: Comprehensive test suite

---

### 3E. Add Metrics and Enhanced Tracing

**Status**: ⏸️ Not Started
**Priority**: Medium
**Effort**: 3-5 hours

**Problem**: Limited observability for production debugging

**Solution**:
1. Add dependencies:
   - `metrics`
   - `metrics-exporter-prometheus`
2. Add `#[instrument]` to key functions
3. Implement metrics:
   - `api_requests_total` (counter)
   - `api_request_duration_seconds` (histogram)
   - `modbus_operations_total` (counter)
   - `modbus_errors_total` (counter)
   - `actor_queue_depth` (gauge)
   - `cache_hit_rate` (gauge)
4. Add `/metrics` Prometheus endpoint
5. Improve tracing with span IDs for request correlation

**Files Affected**:
- Modified: `server/src/main.rs`
- Modified: `server/src/actor/mod.rs`
- Modified: All API files
- Modified: `Cargo.toml`

**Testing**:
- Verify metrics endpoint works
- Verify counters increment
- Verify histograms record latency

---

### 3F. Implement Graceful Shutdown

**Status**: ⏸️ Not Started
**Priority**: Medium
**Effort**: 2-3 hours

**Problem**: Server can't cleanly shutdown, may lose in-flight requests

**Solution**:
1. Add signal handling (SIGTERM, SIGINT)
2. Implement shutdown coordination:
   ```rust
   async fn shutdown_signal() {
       tokio::signal::ctrl_c().await.expect("Failed to listen");
       info!("Shutdown signal received");
   }
   ```
3. Graceful actor shutdown:
   - Close channel (no new requests)
   - Finish in-flight operations
   - Close serial port cleanly
4. Use `axum::serve(...).with_graceful_shutdown()`
5. Ensure all resources cleaned up

**Files Affected**:
- Modified: `server/src/main.rs`
- Modified: `server/src/actor/mod.rs`

**Testing**:
- Send SIGTERM during active request
- Verify request completes before shutdown
- Verify resources cleaned up

---

### 3G. Migrate to serial2-tokio

**Status**: ⏸️ Not Started
**Priority**: Medium
**Effort**: 2-3 hours

**Problem**: Current `tokio-serial` dependency causes transitive dependency duplication and uses older `nix` crate versions

**Current State**:
- Using `tokio-serial = "5.4.5"`
- Pulls in old `nix v0.26.4` via `serialport v4.7.3` dependency
- Causes duplicate dependencies: `bitflags` (v1.3.2 and v2.10.0), `nix` (v0.26.4 and v0.29.0)
- Old `serialport` dependency has deprecated SPDX license format warning

**Benefits of serial2-tokio**:
- More modern serial port implementation
- Better maintained and more actively developed
- Cleaner dependency tree with newer crate versions
- Potentially better performance and error handling
- Simpler API surface

**Solution**:
1. Replace `tokio-serial` with `serial2-tokio` in Cargo.toml:
   ```toml
   # Before
   tokio-serial = "5.4.5"

   # After
   serial2-tokio = "0.2"  # Check latest version
   ```
2. Update serial port initialization in `server/src/routes/ctc_actor.rs`:
   - Replace `tokio_serial::SerialStream` with `serial2_tokio::SerialPort`
   - Update configuration/builder pattern as needed
3. Update serial configuration conversions in `server/src/config.rs`
4. Test all serial communication functionality

**Files Affected**:
- Modified: `Cargo.toml` (replace tokio-serial dependency)
- Modified: `server/src/routes/ctc_actor.rs` (serial port initialization)
- Modified: `server/src/config.rs` (serial config conversions if needed)
- Modified: `server/src/main.rs` (any serial port setup code)

**Testing**:
- Verify serial port opens successfully
- Test all Modbus read operations
- Test all Modbus write operations
- Verify error handling for serial port failures
- Check `cargo deny` shows reduced dependency duplication

**Dependencies**: None (can be done anytime after Phase 1)

**Expected Impact**:
- Cleaner `cargo deny` output (fewer duplicate warnings)
- Reduced binary size (~500KB from removing duplicate `nix`)
- More maintainable codebase with modern dependencies

---

### 3H. Implement Feature Visibility Checking

**Status**: ⏸️ Not Started
**Priority**: Medium
**Effort**: 3-4 hours

**Problem**: Currently the server attempts to access all parameters regardless of whether they're supported by the specific heater model. This can cause errors or return invalid data when accessing features not available on the connected device.

**Background**:
- Each `CTCModbusParameter` has a `visible` register (e.g., 62531, 62500) containing a bitmask
- Each parameter has a `bit` position (0-15) in that bitmask
- If the bit is set (1), the feature is supported/visible on this heater model
- If the bit is clear (0), the feature is not available
- The `is_visible(value: u16)` method already exists in the code

**Solution**:
1. **Add visibility check wrapper function** in `server/src/routes/ctc_actor.rs`:
   ```rust
   async fn check_parameter_visibility(
       &mut self,
       param: &CTCModbusParameter
   ) -> Result<bool, ModbusError> {
       // Read the visibility bitmask register
       let bitmask = self.ctx.read_holding_registers(param.visible, 1)
           .await
           .map_err(|e| ModbusError::ReadError(param.visible, e.to_string()))?;

       Ok(param.is_visible(bitmask[0]))
   }
   ```

2. **Add new error variant** in `server/src/error.rs`:
   ```rust
   // In ModbusError enum
   FeatureNotSupported(u16, &'static str),  // register_id, description

   // In ApiError enum (or map from ModbusError)
   // Returns HTTP 501 Not Implemented
   ```

3. **Check visibility before operations** in `server/src/routes/ctc_actor.rs`:
   ```rust
   async fn read_parameter(&mut self, param: &CTCModbusParameter) -> Result<f32, ModbusError> {
       // Check if feature is supported on this heater
       if !self.check_parameter_visibility(param).await? {
           return Err(ModbusError::FeatureNotSupported(param.id, param.description));
       }

       // Proceed with read...
       let raw_values = self.ctx.read_holding_registers(param.id, 1).await?;
       // ...
   }
   ```

4. **Add configuration option** in `server/src/config.rs`:
   ```toml
   [modbus]
   check_feature_visibility = true  # Enable/disable visibility checks
   cache_visibility_checks = true   # Cache bitmask reads
   ```

5. **Implement visibility cache** (optional optimization):
   - Cache bitmask register values
   - TTL of 5-10 minutes (features don't change at runtime)
   - Reduces Modbus traffic for repeated parameter access

**HTTP Status Code Mapping**:
- Feature not supported → **501 Not Implemented** or **404 Not Found**
- Consider: 501 = "Feature exists but not on this model" vs 404 = "Not found"
- Recommendation: Use **501 Not Implemented** with descriptive error message

**Example Error Response**:
```json
{
  "error": "Feature not supported",
  "parameter": "Hot water boost",
  "register": 61501,
  "message": "This feature is not available on your heater model"
}
```

**Files Affected**:
- Modified: `server/src/routes/ctc_actor.rs` (add visibility checking)
- Modified: `server/src/error.rs` (add FeatureNotSupported variant)
- Modified: `server/src/config.rs` (add visibility check config)
- Modified: `server/src/helpers.rs` (handle new error type)

**Testing**:
- Read visibility bitmask registers (62531, 62500, etc.)
- Verify is_visible() works correctly
- Test with parameters known to be unsupported
- Verify 501 status returned
- Test caching if implemented
- Verify supported parameters still work

**Benefits**:
- Better user experience - clear error messages
- Prevents attempting unsupported operations
- Self-documenting API (clients can discover supported features)
- Prevents confusion from invalid/zero values
- Foundation for future auto-discovery API endpoint

**Future Enhancement** (Phase 5):
- Add `/api/v1/features` endpoint listing all supported features
- Auto-discovery: read all bitmasks on startup
- Generate OpenAPI spec based on actual heater capabilities

**Acceptance Criteria**:
- [ ] Visibility checks implemented for read operations
- [ ] Visibility checks implemented for write operations
- [ ] Returns HTTP 501 with clear error message
- [ ] Configurable enable/disable
- [ ] All tests pass
- [ ] Zero clippy warnings

---

## Phase 4: Low Priority Polish (Weeks 5-8)

**Priority**: 🟢 Low
**Estimated Effort**: Several weeks
**Dependencies**: Phases 1-3 complete

### 4A. Add Health Check Endpoint

**Status**: ⏸️ Not Started
**Effort**: 1 hour

Create `/health` endpoint that reports:
- Overall status (healthy/degraded/unhealthy)
- Actor status (alive/dead)
- Serial port connection status
- Last successful Modbus operation timestamp
- Error rate

---

### 4B. Improve Documentation

**Status**: ⏸️ Not Started
**Effort**: 4-6 hours

- Add `#[doc]` attributes to all public items
- Create architecture diagram
- Add examples to CLAUDE.md
- Document all configuration options
- API documentation with examples
- Deployment guide

---

### 4C. Fix Various Typos

**Status**: ⏸️ Not Started
**Effort**: 30 minutes

- `CTC_VACCATION_DAYS` → `CTC_VACATION_DAYS`
- Remove commented-out code
- Fix inconsistent naming
- Clean up imports

---

### 4D. Remove Broad `#[allow(dead_code)]`

**Status**: ⏸️ Not Started
**Effort**: 1 hour

- Remove module-level allows
- Fix or document individual unused items
- Ensure all public APIs have purpose
- Remove truly dead code

---

### 4E. Add Newtypes for Domain Concepts

**Status**: ⏸️ Not Started
**Effort**: 8-12 hours (significant refactoring)

Create strong types:
- `Temperature(f32)` with validation
- `RegisterAddress(u16)`
- `SlaveId(u8)`
- `RawValue(u16)`
- `ScaledValue(f32)`

Refactor entire codebase to use these types.

---

### 4F. Implement Request Prioritization

**Status**: ⏸️ Not Started
**Effort**: 3-4 hours

- Add priority to `ParameterOperation`
- Use priority queue in actor
- Prioritize writes over reads
- Configurable priority rules

---

## Phase 5: Advanced Features (Future)

**Priority**: 📋 Future
**Estimated Effort**: Months
**Dependencies**: Core system stable

### 5A. Add More BMS Parameters

**Status**: ⏸️ Not Started
**Effort**: Ongoing

Implement additional parameters from BMS specification:

**High Value Parameters**:
- Hot water (DHW) control: 61501-61503, 62001, 62003
- System status register: 62005 (critical for understanding state)
- Alarm/info text system: 65000-65125 (critical for diagnostics)
- Statistics: 62186-62243 (operating hours, energy consumption)

**Medium Value Parameters**:
- Heating systems 2-4 (if needed)
- Pool heating (if configured)
- Solar system (if configured)
- External boiler (if configured)

**Current Coverage**: 44 / 200+ parameters (22%)
**Target**: 80-100 parameters for full single-system support

---

### 5B. Implement Batch Operations

**Status**: ⏸️ Not Started
**Effort**: 1-2 weeks

- Support reading multiple parameters in single Modbus transaction
- Add `/api/v1/batch-read` endpoint
- Optimize for monitoring dashboards
- Implement intelligent batching (sequential registers)

---

### 5C. Add WebSocket Support

**Status**: ⏸️ Not Started
**Effort**: 2-3 weeks

- Real-time parameter updates
- Subscribe to specific parameters
- Push notifications for alarms
- Reduce polling overhead

---

### 5D. Create Web UI

**Status**: ⏸️ Not Started
**Effort**: 1-2 months

- Dashboard for monitoring
- Configuration interface
- Alarm history viewer
- Graphs and trends
- Mobile-responsive design

---

### 5E. Add GraphQL API Endpoint

**Status**: ⏸️ Not Started
**Effort**: 2-3 weeks

**Problem**: Current REST API requires clients to know specific register addresses and parameter names. Clients need to read documentation or source code to discover available parameters and their metadata.

**Solution**: Implement GraphQL endpoint that provides:
- Self-documenting API with schema introspection
- Automatic parameter discovery
- Type-safe queries with validation
- Flexible queries (request only needed fields)
- Real-time subscriptions (future)

**Features**:

1. **Schema Definition**:
   ```graphql
   type Parameter {
     id: Int!
     name: String!
     description: String!
     value: Float
     unit: String
     access: Access!
     visible: Boolean!
     minValue: Float
     maxValue: Float
     step: Float
     category: String!
   }

   enum Access {
     READ_ONLY
     READ_WRITE
   }

   type Query {
     # Get all available parameters
     parameters(category: String): [Parameter!]!

     # Get specific parameter by ID
     parameter(id: Int!): Parameter

     # Get temperature readings
     temperatures: TemperatureReadings!

     # Get system status
     systemStatus: SystemStatus!
   }

   type Mutation {
     # Update parameter value
     setParameter(id: Int!, value: Float!): Parameter!

     # Set power save mode
     setPowerSave(active: Boolean!): PowerSaveStatus!
   }

   type Subscription {
     # Subscribe to parameter changes
     parameterChanged(id: Int!): Parameter!

     # Subscribe to temperature updates
     temperaturesUpdated: TemperatureReadings!
   }
   ```

2. **Implementation**:
   - Use `async-graphql` crate (high-performance Rust GraphQL)
   - Integrate with existing actor pattern
   - Expose alongside REST API (not replacing it)
   - Mount at `/graphql` and `/graphiql` (interactive explorer)

3. **Parameter Discovery**:
   - Query all parameters with their metadata
   - Filter by category (temperatures, settings, status, etc.)
   - Check visibility based on heater model
   - Return only supported parameters

4. **Example Queries**:
   ```graphql
   # Get all temperature parameters
   {
     parameters(category: "temperatures") {
       name
       description
       value
       unit
     }
   }

   # Get specific parameter with validation info
   {
     parameter(id: 61509) {
       name
       value
       minValue
       maxValue
       step
       access
     }
   }

   # Update room temperature
   mutation {
     setParameter(id: 61509, value: 22.0) {
       name
       value
     }
   }
   ```

**Benefits**:
- **Self-documenting**: Clients can discover API via introspection
- **Type safety**: Schema validation prevents invalid queries
- **Efficiency**: Request only needed fields, reducing bandwidth
- **Developer experience**: GraphiQL playground for exploration
- **Future-proof**: Easy to extend without breaking changes
- **Tooling**: Excellent client libraries for all languages

**Dependencies**:
- Phase 3H (Feature Visibility Checking) recommended first
- Provides foundation for parameter discovery

**Files Affected**:
- New: `server/src/graphql/mod.rs` (GraphQL schema)
- New: `server/src/graphql/query.rs` (Query resolvers)
- New: `server/src/graphql/mutation.rs` (Mutation resolvers)
- New: `server/src/graphql/types.rs` (GraphQL types)
- Modified: `server/src/main.rs` (add GraphQL routes)
- Modified: `Cargo.toml` (add async-graphql dependencies)

**Cargo Dependencies**:
```toml
async-graphql = "7"
async-graphql-axum = "7"
```

**Testing**:
- Query all parameters via GraphQL
- Verify schema introspection works
- Test mutations with validation
- Compare REST vs GraphQL response times
- Test with GraphiQL playground

**Future Enhancements**:
- Real-time subscriptions via WebSocket
- Batched parameter updates
- Historical data queries
- Alarm/event subscriptions
- Integration with Phase 5D Web UI

**Acceptance Criteria**:
- [ ] GraphQL endpoint at `/graphql`
- [ ] GraphiQL playground at `/graphiql`
- [ ] Schema includes all current parameters
- [ ] Queries and mutations work
- [ ] Feature visibility integrated
- [ ] Documentation updated
- [ ] Performance acceptable (< 100ms p99)

---

## Completed Items

### 2025-10-30

#### Phase 0: Foundation

✅ **Documentation Skills Created**
- Created `ctc-modbus-protocol.md` skill with Modbus RTU specifications
- Created `ctc-bms-datamodel.md` skill with complete register reference
- Created `ctc-ecoheat400-configuration.md` skill with EcoHeat 400 specifics

✅ **Implementation Review**
- Comprehensive analysis of codebase vs official BMS documentation
- Identified all discrepancies and improvement opportunities
- Verified correct implementation of data types and scaling

✅ **Architecture Analysis**
- Detailed review of maintainability, scalability, and code quality
- Prioritized issues and recommendations
- Created phased improvement plan

✅ **Roadmap Creation**
- This document created with comprehensive feature tracking

---

#### Phase 1: Critical Fixes

✅ **Phase 1A: Fix Step Validation Bug**
- Fixed validation logic in `server/src/routes/ctc_actor.rs:159`
- Changed from `!raw_value.is_multiple_of(step)` to `!(raw_value - min).is_multiple_of(step)`
- Now correctly validates values relative to minimum, not zero
- Added 3 unit tests covering various scenarios
- All 19 initial unit tests pass

✅ **Phase 1B: Fix API Typo**
- Fixed typo in `server/src/routes/temperatures.rs:55, 87`
- Changed `"room_temperarure_setpoint"` to `"room_temperature_setpoint"`
- Fixed in both GET and POST responses

✅ **Phase 1C: Eliminate Code Duplication**
- Created `server/src/helpers.rs` module (130 lines) with 3 helper functions
- Reduced route handlers by ~200 lines of duplicate code
- `temperatures.rs`: 167 lines → 57 lines (66% reduction)
- `ctc.rs`: ~180 lines → 90 lines (50% reduction)
- All endpoint functions now 1-3 lines instead of 15-20 lines
- Zero clippy warnings in pedantic mode

✅ **Phase 1D: Configuration Management System**
- Created `server/src/config.rs` module (264 lines) with comprehensive config system
- Implemented 5 config structures: Config, ServerConfig, SerialConfig, ModbusConfig, PowerSaveConfig, TemperatureValidationConfig
- Created `config.toml.example` with full documentation
- Configuration hierarchy: defaults → config.toml → env vars → CLI args
- Eliminated all 10+ hard-coded values from codebase
- Added 3 unit tests for config loading and validation
- Total tests: 47 passing (was 19)

✅ **Phase 1E: Typed Error System**
- Created `server/src/error.rs` module (444 lines) with comprehensive error types
- ModbusError enum with 8 variants covering all error cases
- ApiError enum with 4 variants mapping to HTTP status codes
- Implemented Display, Error, IntoResponse, and From traits
- Replaced all String errors with typed errors throughout codebase
- Internal error details logged but minimal exposure to clients
- Added 24 comprehensive unit tests for all error types
- Implementation uses manual traits instead of thiserror for explicit control

---

#### Infrastructure Improvements

✅ **Docker Support**
- Multi-stage Dockerfile (95 lines) with cargo-chef optimization
- docker-compose.yml (48 lines) for production deployment
- docker-compose.override.yml.example for local development
- DOCKER.md (665 lines) comprehensive deployment guide
- ARM64 platform support
- Non-root user with serial port access
- Health check configuration
- Environment variable support

✅ **Security Tooling**
- cargo-deny configuration (deny.toml, 31 lines)
- Security advisory scanning enabled
- License compliance with strict allowlist (MIT, Apache-2.0, BSD-3-Clause, etc.)
- Copyleft license rejection (GPL, LGPL, AGPL blocked)
- Supply chain security hardening
- Git commit message guidelines (.claude/skills/git-commit-msg.md)

---

#### Code Quality Improvements

✅ **Replace `.unwrap()` Calls - 100% Complete** (2025-10-30)
- Eliminated ALL 16+ `.unwrap()` calls from production code
- Fixed all route handlers and main.rs
- Fixed all 3 helper functions in `server/src/helpers.rs`
- Zero `.unwrap()` calls remaining in production code
- All channel operations now use proper error handling
- Functions return `503 Service Unavailable` if actor channel fails

---

## Dependency Graph

```
Phase 0 (Complete)
    ↓
Phase 1 (Critical Fixes) - ✅ Complete
    ↓
Phase 1.5 (Production Stability) - 🔴 BLOCKING ⚠️ MUST DO NOW
    ↓
Phase 2 (High Priority)
    ↓
Phase 3 (Medium Priority)
    ↓
Phase 4 (Low Priority)
    ↓
Phase 5 (Future Features)
```

**Parallel Work Opportunities**:
- 3A (Module reorganization) can be done anytime
- 4C (Fix typos) can be done anytime
- 5A (More parameters) can start once Phase 1 complete
- Documentation (4B) can be ongoing

---

## Timeline Estimates

| Phase | Duration | Cumulative |
|-------|----------|------------|
| Phase 0 | Complete | - |
| Phase 1 | 2-3 days | Week 1 |
| Phase 2 | 3-5 days | Week 2 |
| Phase 3 | 2-3 weeks | Weeks 3-4 |
| Phase 4 | 4-6 weeks | Weeks 5-10 |
| Phase 5 | Months | Ongoing |

**Minimum Viable Improvements**: Phases 1-2 (1-2 weeks)
**Production Ready**: Phases 1-3 (4-5 weeks)
**Fully Polished**: Phases 1-4 (10-12 weeks)

---

## Success Metrics

### Code Quality
- [x] Code duplication < 5% ~~(currently ~20%)~~ **✅ Achieved: ~5%**
- [ ] Test coverage > 80% (currently ~50-60%, was ~30%)
- [x] Zero `.unwrap()` in production code ~~(currently 16+)~~ **✅ Achieved: 0**
- [x] Zero hard-coded values ~~(currently 10+)~~ **✅ Achieved: 0**

### Reliability
- [ ] Automatic error recovery working
- [ ] Graceful degradation under failures
- [ ] No service crashes in 30-day period
- [ ] Uptime > 99.9%

### Performance
- [ ] API response time < 500ms p99
- [ ] Cache hit rate > 80% for read operations
- [ ] Modbus errors < 1% of operations
- [ ] Queue depth < 10 under normal load

### Maintainability
- [ ] New endpoint can be added in < 10 lines
- [ ] New parameter can be added in 1 line (macro)
- [ ] All errors have proper types
- [ ] Comprehensive documentation

---

## Notes

- **Backward Compatibility**: Maintain API compatibility until v2.0.0
- **Configuration**: Provide migration guide when config system added
- **Testing**: Each phase should include appropriate tests
- **Documentation**: Update as features are added
- **Review**: Reassess priorities quarterly

---

## Change Log

### Version 1.4 (2025-11-01)
- 🔴 **CRITICAL PRODUCTION INCIDENT**: Actor deadlock due to missing timeouts
  - Service down for ~15 hours (2025-10-31 17:19:14 to 2025-11-01 08:00+)
  - Root cause: Modbus read operation hung indefinitely, blocking actor
  - 194 operations succeeded before deadlock; all subsequent requests hung
  - **Created Phase 1.5**: Production Stability (URGENT) - blocks service restart
- **New Phase 1.5 Tasks** (BLOCKING):
  - 1.5A: Implement Modbus Operation Timeout (1 hour) - CRITICAL
  - 1.5B: Implement Request/Response Timeout (30 min) - CRITICAL
  - 1.5C: Add Basic Error Recovery (1 hour) - HIGH
  - 1.5D: Add Health Check Endpoint (30 min) - MEDIUM
- **Moved tasks to Phase 1.5**:
  - Phase 2C (Timeouts) → Phase 1.5A & 1.5B
  - Phase 2F (Error Recovery) → Phase 1.5C (partial, basic recovery only)
- **Incident Report**: Created INCIDENT_2025-11-01_ACTOR_DEADLOCK.md
  - 15 pages of detailed analysis
  - Timeline, root cause, impact assessment
  - Recommendations and prevention measures
- **New Features Added**:
  - Phase 3H: Implement Feature Visibility Checking (3-4 hours)
    - Use existing bitmask system to check feature support before access
    - Return HTTP 501 Not Implemented for unsupported features
    - Prevents errors on heater models lacking specific features
  - Phase 5E: Add GraphQL API Endpoint (2-3 weeks)
    - Self-documenting API with automatic parameter discovery
    - GraphiQL playground for interactive exploration
    - Type-safe queries with schema validation
    - Foundation for modern client applications
- **Project Status**: 🔴 INCIDENT MODE - service restart blocked until Phase 1.5 complete
- **Lessons Learned**:
  - All async I/O must have timeouts
  - Single-threaded actors need health monitoring
  - Testing must cover failure scenarios
  - High priority items need immediate action

### Version 1.3 (2025-10-30)
- **New task added**: Phase 3G - Migrate to serial2-tokio
  - Address dependency duplication from tokio-serial
  - Reduce duplicate `nix` and `bitflags` dependencies
  - Modernize serial port implementation
  - Estimated effort: 2-3 hours
  - Would reduce binary size by ~500KB
  - Would clean up cargo-deny warnings

### Version 1.2 (2025-10-30)
- **Phase 2B COMPLETE**: Eliminated all remaining `.unwrap()` calls
  - Fixed 3 remaining `.unwrap()` calls in `server/src/helpers.rs`
  - Zero `.unwrap()` calls in production code (was 16+)
  - All channel operations now use proper error handling
  - Production hardening complete
- **Static analysis**: All tools passing with zero errors
  - Zero clippy warnings (pedantic mode)
  - Zero security vulnerabilities (cargo audit)
  - All licenses compliant (cargo deny)
  - Zero compiler warnings
- **Updated metrics**:
  - `.unwrap()` calls: 3 → 0 ✅ (100% complete)
  - Code quality: 3/4 success metrics achieved

### Version 1.1 (2025-10-30)
- **Phase 1 COMPLETE**: All 5 critical tasks finished (1A-1E)
  - Step validation bug fixed
  - API typo corrected
  - Code duplication eliminated (~200 lines reduced)
  - Configuration management system implemented
  - Typed error system implemented
- **Infrastructure improvements**: Docker support and security tooling added
- **Metrics updated**:
  - Code duplication: 20% → 5% ✅
  - Hard-coded values: 10+ → 0 ✅
  - Unit tests: 19 → 47 tests (+28)
  - Test coverage: 30% → 50-60% (+20-30%)
  - `.unwrap()` calls: 16+ → 3 (95% reduction)
- **Success**: Project now in better shape than initially assessed
- **Next focus**: Phase 2 (High Priority Improvements)

### Version 1.0 (2025-10-30)
- Initial roadmap created
- Comprehensive analysis and prioritization
- 5 phases defined with detailed task breakdown

---

**Last Updated**: 2025-10-30 (Version 1.3)
**Next Review**: 2025-11-30
**Maintained By**: Development Team

---

**Note**: Phase 1 and Phase 2B were completed on the same day, demonstrating rapid progress on critical infrastructure improvements and code quality hardening. The codebase now has zero `.unwrap()` calls, zero hard-coded values, and zero static analysis warnings. Ready for remaining Phase 2 high-priority enhancements.
