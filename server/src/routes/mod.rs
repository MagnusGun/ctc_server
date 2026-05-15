pub mod activity;
pub mod alarms;
pub mod ctc;
pub mod dhw;
pub mod grid;
pub mod heatpump_stats;
pub mod modbus;
pub mod pump;
pub mod series;
pub mod smartgrid;
pub mod step_response;
pub mod temperatures;
pub mod visibility;

use std::time::SystemTime;

use crate::error::ApiError;

/// Returns `(from, now, to)` Unix-second bounds for the last `hours` hours.
/// `to` is `now+1` so `series_range`'s half-open interval includes a sample
/// written in the same second the request arrives.
pub(crate) fn series_window(hours: u32) -> Result<(i64, i64, i64), ApiError> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| ApiError::InternalError)?
        .as_secs();
    let now_i64 = i64::try_from(now).map_err(|_| ApiError::InternalError)?;
    let from = now_i64 - i64::from(hours) * 3600;
    Ok((from, now_i64, now_i64.saturating_add(1)))
}
