# CTC Server - Development Roadmap

**Document Version**: 1.0
**Last Updated**: 2025-10-30
**Project Status**: Active Development

## Table of Contents

- [Current State Assessment](#current-state-assessment)
- [Phase 0: Foundation](#phase-0-foundation-complete)
- [Phase 1: Critical Fixes (Week 1)](#phase-1-critical-fixes-week-1)
- [Phase 2: High Priority Improvements (Week 2)](#phase-2-high-priority-improvements-week-2)
- [Phase 3: Medium Priority Enhancements (Weeks 3-4)](#phase-3-medium-priority-enhancements-weeks-3-4)
- [Phase 4: Low Priority Polish (Weeks 5-8)](#phase-4-low-priority-polish-weeks-5-8)
- [Phase 5: Advanced Features (Future)](#phase-5-advanced-features-future)
- [Completed Items](#completed-items)

---

## Current State Assessment

### ✅ Strengths

- **Architecture**: Clean actor pattern for serial port access
- **Data Model**: Accurate BMS parameter definitions matching official documentation
- **Code Quality**: Well-structured modules, good separation of concerns
- **Testing**: Comprehensive unit tests for value conversion logic
- **Documentation**: Excellent CLAUDE.md for development context

### ⚠️ Issues Identified

**Critical** (3 issues):
- Massive code duplication (~200 lines) in route handlers
- No configuration management (10+ hard-coded values)
- String-based error handling (no type safety)

**High** (6 issues):
- Liberal use of `.unwrap()` (16+ occurrences)
- No error recovery in actor
- No request/response timeout
- Large functions needing refactoring
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

| Metric | Value |
|--------|-------|
| Total Lines of Code | ~966 |
| Test Coverage | ~30% (unit tests only) |
| BMS Parameters Implemented | 44 / 200+ |
| Hard-Coded Values | 10+ |
| Code Duplication | ~200 lines |

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

## Phase 1: Critical Fixes (Week 1)

**Priority**: 🔴 Critical
**Estimated Effort**: 2-3 days
**Dependencies**: None

### 1A. Fix Step Validation Bug

**Status**: ⏸️ Not Started
**Priority**: Critical
**Effort**: 15 minutes
**Registers Affected**: All writable parameters

**Problem**: Validation checks `raw_value % step != 0` which fails when min value is not a multiple of step.

**Fix**:
- File: `server/src/routes/ctc_actor.rs:158`
- Change from: `raw_value % step != 0`
- Change to: `((raw_value - min) % step) != 0`

**Testing**: Add unit test with min=152, step=5, verify 152 and 157 pass, 153 fails

---

### 1B. Fix API Typo

**Status**: ⏸️ Not Started
**Priority**: Critical
**Effort**: 5 minutes

**Problem**: Typo in JSON response field name

**Fix**:
- File: `server/src/routes/temperatures.rs:55, 87`
- Change: `"room_temperarure_setpoint"` → `"room_temperature_setpoint"`

**Testing**: Verify API response has correct field name

---

### 1C. Eliminate Code Duplication

**Status**: ⏸️ Not Started
**Priority**: Critical
**Effort**: 2-4 hours

**Problem**: Every API endpoint repeats 15-line request/response pattern (~200 lines of duplicate code)

**Solution**:
1. Create `server/src/helpers.rs` module
2. Implement helper functions:
   ```rust
   pub async fn read_parameter(
       tx: &ModbusSender,
       param: &'static CTCModbusParameter
   ) -> Result<f32, ApiError>

   pub async fn write_parameter(
       tx: &ModbusSender,
       param: &'static CTCModbusParameter,
       value: f32
   ) -> Result<f32, ApiError>
   ```
3. Refactor all 16 route handlers to use helpers
4. Estimated reduction: 200+ lines removed

**Files Affected**:
- New: `server/src/helpers.rs`
- Modified: `server/src/routes/ctc.rs`
- Modified: `server/src/routes/temperatures.rs`
- Modified: `server/src/lib.rs` (add helpers module)

**Testing**: Verify all endpoints still work correctly

---

### 1D. Add Configuration Management

**Status**: ⏸️ Not Started
**Priority**: Critical
**Effort**: 4-6 hours

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

**Solution**:
1. Add `config` crate dependency
2. Create `server/src/config.rs` module with structs:
   - `Config` (root)
   - `ServerConfig` (host, port)
   - `SerialConfig` (port, baud_rate, timeout, flow_control)
   - `ModbusConfig` (slave_id, channel_buffer_size, timeout)
   - `PowerSaveConfig` (temperature, vacation_days, normal_temp)
   - `RetryConfig` (max_retries, initial_delay_ms, backoff_multiplier)
3. Create example `config.toml` in project root
4. Support loading: default → config.toml → env vars → CLI args
5. Update all hard-coded references to use config

**Files Affected**:
- New: `server/src/config.rs`
- New: `config.toml` (example)
- Modified: `server/src/main.rs`
- Modified: `server/src/routes/ctc_actor.rs`
- Modified: `server/src/routes/ctc.rs`
- Modified: `server/src/routes/temperatures.rs`
- Modified: `Cargo.toml` (add config dependency)

**Testing**:
- Verify loading from config file
- Verify env var override
- Verify CLI arg override

---

### 1E. Implement Proper Error Types

**Status**: ⏸️ Not Started
**Priority**: Critical
**Effort**: 3-5 hours

**Problem**: All errors are `String` types - no type safety, context, or structured information

**Solution**:
1. Add `thiserror` crate dependency
2. Create `server/src/error.rs` module
3. Define error types:
   ```rust
   #[derive(Debug, thiserror::Error)]
   pub enum ModbusError {
       #[error("Failed to read parameter {0}: {1}")]
       ReadError(u16, String),

       #[error("Failed to write parameter {0} with value {1}: {2}")]
       WriteError(u16, f32, String),

       #[error("Parameter {0} is read-only")]
       ReadOnly(u16),

       #[error("Value {0} outside range [{1}, {2}]")]
       OutOfRange(f32, f32, f32),

       #[error("Modbus communication error: {0}")]
       Communication(#[from] tokio_modbus::Error),

       #[error("Serial port error: {0}")]
       SerialPort(#[from] tokio_serial::Error),
   }

   #[derive(Debug, thiserror::Error)]
   pub enum ApiError {
       #[error("Modbus error: {0}")]
       Modbus(#[from] ModbusError),

       #[error("Invalid parameter value: {0}")]
       InvalidValue(String),

       #[error("Service unavailable")]
       ServiceUnavailable,

       #[error("Request timeout")]
       Timeout,
   }
   ```
4. Implement `IntoResponse` for `ApiError` to return proper HTTP status codes
5. Replace all `String` errors throughout codebase

**Files Affected**:
- New: `server/src/error.rs`
- Modified: All route files
- Modified: `server/src/routes/ctc_actor.rs`
- Modified: `server/src/helpers.rs` (if created)
- Modified: `Cargo.toml` (add thiserror dependency)

**Testing**: Verify proper error responses with status codes

---

## Phase 2: High Priority Improvements (Week 2)

**Priority**: 🟠 High
**Estimated Effort**: 3-5 days
**Dependencies**: Phase 1 complete

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

**Status**: ⏸️ Not Started
**Priority**: High
**Effort**: 1-2 hours

**Problem**: 16+ `.unwrap()` calls that can cause panics

**Locations**:
- `main.rs:56-57` - Server bind/serve
- All route handlers - Channel send operations
- Various other locations

**Solution**:
1. Audit all `.unwrap()` calls
2. Replace with `?` operator and proper error handling
3. Use `.map_err()` to provide context
4. Example:
   ```rust
   // Before
   tx.send(msg).await.unwrap();

   // After
   tx.send(msg).await.map_err(|_| ApiError::ServiceUnavailable)?;
   ```

**Files Affected**: All route files, main.rs

**Testing**: Verify proper error handling when operations fail

---

### 2C. Add Request/Response Timeout

**Status**: ⏸️ Not Started
**Priority**: High
**Effort**: 1-2 hours

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

**Status**: ⏸️ Not Started
**Priority**: High
**Effort**: 3-4 hours

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

## Completed Items

### 2025-10-30

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

## Dependency Graph

```
Phase 0 (Complete)
    ↓
Phase 1 (Critical Fixes)
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
- [ ] Code duplication < 5% (currently ~20%)
- [ ] Test coverage > 80% (currently ~30%)
- [ ] Zero `.unwrap()` in production code (currently 16+)
- [ ] Zero hard-coded values (currently 10+)

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

**Last Updated**: 2025-10-30
**Next Review**: 2025-11-30
**Maintained By**: Development Team
