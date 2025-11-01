//! Error types for CTC server
//!
//! This module defines custom error types that provide type safety while
//! minimizing information exposure to API clients. Errors map to standard
//! HTTP status codes without revealing implementation details.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::fmt;

/// Internal Modbus operation errors
///
/// These errors capture detailed information about what went wrong internally,
/// but are converted to generic `ApiError` before being sent to clients.
#[derive(Debug)]
pub enum ModbusError {
    /// Failed to read a parameter
    ReadError { register: u16, reason: String },

    /// Failed to write a parameter
    WriteError {
        register: u16,
        value: f32,
        reason: String,
    },

    /// Attempted to write to a read-only parameter
    ReadOnly { register: u16 },

    /// Value outside allowed range
    OutOfRange {
        value: f32,
        min: f32,
        max: f32,
        register: u16,
    },

    /// Value doesn't match step requirement
    InvalidStep {
        value: f32,
        min: f32,
        step: f32,
        register: u16,
    },

    /// Failed to read min/max/step validation parameters
    ValidationReadError { register: u16, reason: String },

    /// Serial port communication error
    #[allow(dead_code)]
    SerialError { reason: String },

    /// Modbus protocol error
    ProtocolError { reason: String },

    /// Read-back verification failed after write
    VerificationError {
        expected: f32,
        actual: f32,
        register: u16,
    },

    /// Operation timed out
    Timeout { register: u16, operation: String },

    /// Maximum retry attempts exceeded
    MaxRetriesExceeded {
        register: u16,
        retries: u32,
        last_error: String,
    },
}

impl fmt::Display for ModbusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadError { register, reason } => {
                write!(f, "Failed to read register {register}: {reason}")
            }
            Self::WriteError {
                register,
                value,
                reason,
            } => {
                write!(
                    f,
                    "Failed to write value {value} to register {register}: {reason}"
                )
            }
            Self::ReadOnly { register } => {
                write!(f, "Register {register} is read-only")
            }
            Self::OutOfRange {
                value,
                min,
                max,
                register,
            } => {
                write!(
                    f,
                    "Value {value} for register {register} is outside allowed range [{min}, {max}]"
                )
            }
            Self::InvalidStep {
                value,
                min,
                step,
                register,
            } => {
                write!(
                    f,
                    "Value {value} for register {register} is not a valid step from minimum {min} (step: {step})"
                )
            }
            Self::ValidationReadError { register, reason } => {
                write!(
                    f,
                    "Failed to read validation parameters for register {register}: {reason}"
                )
            }
            Self::SerialError { reason } => {
                write!(f, "Serial port error: {reason}")
            }
            Self::ProtocolError { reason } => {
                write!(f, "Modbus protocol error: {reason}")
            }
            Self::VerificationError {
                expected,
                actual,
                register,
            } => {
                write!(
                    f,
                    "Read-back verification failed for register {register}: expected {expected}, got {actual}"
                )
            }
            Self::Timeout {
                register,
                operation,
            } => {
                write!(
                    f,
                    "Operation timed out for register {register}: {operation}"
                )
            }
            Self::MaxRetriesExceeded {
                register,
                retries,
                last_error,
            } => {
                write!(
                    f,
                    "Maximum retries ({retries}) exceeded for register {register}: {last_error}"
                )
            }
        }
    }
}

impl std::error::Error for ModbusError {}

/// API-level errors returned to clients
///
/// These errors map to HTTP status codes and expose minimal information
/// to clients. Full error details are logged internally.
#[derive(Debug)]
pub enum ApiError {
    /// Invalid request parameters (400 Bad Request)
    BadRequest,

    /// Internal server error (500 Internal Server Error)
    InternalError,

    /// Service temporarily unavailable (503 Service Unavailable)
    ServiceUnavailable,

    /// Request timeout (408 Request Timeout)
    Timeout,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest => write!(f, "Bad Request"),
            Self::InternalError => write!(f, "Internal Server Error"),
            Self::ServiceUnavailable => write!(f, "Service Unavailable"),
            Self::Timeout => write!(f, "Request Timeout"),
        }
    }
}

impl std::error::Error for ApiError {}

/// Convert `ModbusError` to `ApiError`
///
/// Maps internal errors to appropriate API-level errors.
/// Full details of the `ModbusError` should be logged before conversion.
impl From<ModbusError> for ApiError {
    fn from(err: ModbusError) -> Self {
        match err {
            // Client errors (invalid input)
            ModbusError::ReadOnly { .. }
            | ModbusError::OutOfRange { .. }
            | ModbusError::InvalidStep { .. } => Self::BadRequest,

            // Timeout errors
            ModbusError::Timeout { .. } => Self::Timeout,

            // Server errors (internal failures)
            ModbusError::ReadError { .. }
            | ModbusError::WriteError { .. }
            | ModbusError::ValidationReadError { .. }
            | ModbusError::SerialError { .. }
            | ModbusError::ProtocolError { .. }
            | ModbusError::VerificationError { .. }
            | ModbusError::MaxRetriesExceeded { .. } => Self::InternalError,
        }
    }
}

/// Implement Axum's `IntoResponse` for `ApiError`
///
/// Returns only the HTTP status code to clients, no body.
/// This minimizes information exposure.
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout => StatusCode::REQUEST_TIMEOUT,
        };

        // Return only status code, no body
        status.into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to extract status code from IntoResponse
    fn get_status_code(error: ApiError) -> StatusCode {
        let response = error.into_response();
        response.status()
    }

    #[test]
    fn test_modbus_error_display_read_error() {
        let err = ModbusError::ReadError {
            register: 1234,
            reason: "timeout".to_string(),
        };
        assert_eq!(err.to_string(), "Failed to read register 1234: timeout");
    }

    #[test]
    fn test_modbus_error_display_write_error() {
        let err = ModbusError::WriteError {
            register: 5678,
            value: 23.5,
            reason: "connection lost".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Failed to write value 23.5 to register 5678: connection lost"
        );
    }

    #[test]
    fn test_modbus_error_display_read_only() {
        let err = ModbusError::ReadOnly { register: 999 };
        assert_eq!(err.to_string(), "Register 999 is read-only");
    }

    #[test]
    fn test_modbus_error_display_out_of_range() {
        let err = ModbusError::OutOfRange {
            value: 30.0,
            min: 5.0,
            max: 25.0,
            register: 1111,
        };
        assert_eq!(
            err.to_string(),
            "Value 30 for register 1111 is outside allowed range [5, 25]"
        );
    }

    #[test]
    fn test_modbus_error_display_invalid_step() {
        let err = ModbusError::InvalidStep {
            value: 17.5,
            min: 15.0,
            step: 1.0,
            register: 2222,
        };
        assert_eq!(
            err.to_string(),
            "Value 17.5 for register 2222 is not a valid step from minimum 15 (step: 1)"
        );
    }

    #[test]
    fn test_modbus_error_display_validation_read_error() {
        let err = ModbusError::ValidationReadError {
            register: 3333,
            reason: "register not found".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Failed to read validation parameters for register 3333: register not found"
        );
    }

    #[test]
    fn test_modbus_error_display_protocol_error() {
        let err = ModbusError::ProtocolError {
            reason: "invalid CRC".to_string(),
        };
        assert_eq!(err.to_string(), "Modbus protocol error: invalid CRC");
    }

    #[test]
    fn test_modbus_error_display_verification_error() {
        let err = ModbusError::VerificationError {
            expected: 22.0,
            actual: 21.5,
            register: 4444,
        };
        assert_eq!(
            err.to_string(),
            "Read-back verification failed for register 4444: expected 22, got 21.5"
        );
    }

    #[test]
    fn test_api_error_display_bad_request() {
        let err = ApiError::BadRequest;
        assert_eq!(err.to_string(), "Bad Request");
    }

    #[test]
    fn test_api_error_display_internal_error() {
        let err = ApiError::InternalError;
        assert_eq!(err.to_string(), "Internal Server Error");
    }

    #[test]
    fn test_api_error_display_service_unavailable() {
        let err = ApiError::ServiceUnavailable;
        assert_eq!(err.to_string(), "Service Unavailable");
    }

    #[test]
    fn test_api_error_display_timeout() {
        let err = ApiError::Timeout;
        assert_eq!(err.to_string(), "Request Timeout");
    }

    #[test]
    fn test_modbus_to_api_error_read_only_is_bad_request() {
        let modbus_err = ModbusError::ReadOnly { register: 100 };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_modbus_to_api_error_out_of_range_is_bad_request() {
        let modbus_err = ModbusError::OutOfRange {
            value: 50.0,
            min: 0.0,
            max: 40.0,
            register: 200,
        };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_modbus_to_api_error_invalid_step_is_bad_request() {
        let modbus_err = ModbusError::InvalidStep {
            value: 17.5,
            min: 15.0,
            step: 1.0,
            register: 300,
        };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_modbus_to_api_error_read_error_is_internal_error() {
        let modbus_err = ModbusError::ReadError {
            register: 400,
            reason: "timeout".to_string(),
        };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_modbus_to_api_error_write_error_is_internal_error() {
        let modbus_err = ModbusError::WriteError {
            register: 500,
            value: 10.0,
            reason: "hardware failure".to_string(),
        };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_modbus_to_api_error_protocol_error_is_internal_error() {
        let modbus_err = ModbusError::ProtocolError {
            reason: "invalid frame".to_string(),
        };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_modbus_to_api_error_verification_error_is_internal_error() {
        let modbus_err = ModbusError::VerificationError {
            expected: 20.0,
            actual: 19.0,
            register: 600,
        };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_api_error_bad_request_status_code() {
        let err = ApiError::BadRequest;
        assert_eq!(get_status_code(err), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_api_error_internal_error_status_code() {
        let err = ApiError::InternalError;
        assert_eq!(get_status_code(err), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_api_error_service_unavailable_status_code() {
        let err = ApiError::ServiceUnavailable;
        assert_eq!(get_status_code(err), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_api_error_timeout_status_code() {
        let err = ApiError::Timeout;
        assert_eq!(get_status_code(err), StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn test_error_trait_implemented_for_modbus_error() {
        let err = ModbusError::ReadError {
            register: 1,
            reason: "test".to_string(),
        };
        // This compiles only if Error trait is implemented
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn test_error_trait_implemented_for_api_error() {
        let err = ApiError::InternalError;
        // This compiles only if Error trait is implemented
        let _: &dyn std::error::Error = &err;
    }

    // Tests for new Timeout and MaxRetriesExceeded variants

    #[test]
    fn test_modbus_error_display_timeout() {
        let err = ModbusError::Timeout {
            register: 61509,
            operation: "read_holding_registers".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Operation timed out for register 61509: read_holding_registers"
        );
    }

    #[test]
    fn test_modbus_error_display_max_retries_exceeded() {
        let err = ModbusError::MaxRetriesExceeded {
            register: 61509,
            retries: 3,
            last_error: "Connection refused".to_string(),
        };
        assert!(err.to_string().contains("Maximum retries (3) exceeded"));
        assert!(err.to_string().contains("61509"));
        assert!(err.to_string().contains("Connection refused"));
    }

    #[test]
    fn test_modbus_timeout_to_api_error() {
        let modbus_err = ModbusError::Timeout {
            register: 61509,
            operation: "read".to_string(),
        };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn test_modbus_max_retries_to_api_error_is_internal_error() {
        let modbus_err = ModbusError::MaxRetriesExceeded {
            register: 61509,
            retries: 3,
            last_error: "timeout".to_string(),
        };
        let api_err: ApiError = modbus_err.into();
        assert_eq!(get_status_code(api_err), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_timeout_error_contains_register_id() {
        let err = ModbusError::Timeout {
            register: 12345,
            operation: "test_op".to_string(),
        };
        let err_string = err.to_string();
        assert!(err_string.contains("12345"));
        assert!(err_string.contains("test_op"));
    }

    #[test]
    fn test_max_retries_error_contains_retry_count() {
        let err = ModbusError::MaxRetriesExceeded {
            register: 100,
            retries: 5,
            last_error: "fail".to_string(),
        };
        let err_string = err.to_string();
        assert!(err_string.contains('5'));
    }

    #[test]
    fn test_timeout_api_error_returns_408_status() {
        let api_err = ApiError::Timeout;
        assert_eq!(get_status_code(api_err), StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn test_timeout_display_format() {
        let err = ModbusError::Timeout {
            register: 999,
            operation: "write_single_register".to_string(),
        };
        assert!(err.to_string().contains("timed out"));
        assert!(err.to_string().contains("999"));
    }

    #[test]
    fn test_max_retries_display_includes_last_error() {
        let err = ModbusError::MaxRetriesExceeded {
            register: 777,
            retries: 2,
            last_error: "Hardware failure detected".to_string(),
        };
        let err_string = err.to_string();
        assert!(err_string.contains("Hardware failure detected"));
        assert!(err_string.contains("777"));
        assert!(err_string.contains('2'));
    }

    #[test]
    fn test_error_trait_for_timeout_variant() {
        let err = ModbusError::Timeout {
            register: 1,
            operation: "test".to_string(),
        };
        // This compiles only if Error trait is implemented
        let _: &dyn std::error::Error = &err;
    }
}
