//! DHW boost state + atomic persistence (mirrors `heatpump_stats` pattern).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BoostPreset {
    Shower,
    Bath { hours: f32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DhwBoostState {
    pub preset: BoostPreset,
    pub started_at: DateTime<Utc>,
    pub duration_secs: u64,
    /// Snapshot of `61636` at Bath start, for restore at Bath stop. None for Shower.
    pub prior_immersion_engage_temp_c: Option<f32>,
    /// Whether the immersion gate has written a non-zero `61591` value.
    pub immersion_engaged: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DhwPersistedState {
    pub schema_version: u32,
    pub boost: Option<DhwBoostState>,
}

impl Default for DhwPersistedState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            boost: None,
        }
    }
}

const CURRENT_SCHEMA: u32 = 1;

impl DhwPersistedState {
    /// Atomic save: write `<path>.tmp` then rename. Survives mid-write crashes.
    ///
    /// # Errors
    /// Returns the underlying `std::io::Error` if creating the parent
    /// directory, writing the temp file, or renaming fails. Serialisation
    /// failures are wrapped as `std::io::Error::other`.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension(
            path.extension()
                .map_or_else(|| "tmp".into(), |e| format!("{}.tmp", e.to_string_lossy())),
        );
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Tolerant load: missing file or any parse/version problem returns a fresh
    /// `default()` so startup never blocks. Warnings are logged for non-fresh cases.
    ///
    /// # Errors
    /// Returns the underlying `std::io::Error` only for unexpected I/O
    /// failures (anything other than `NotFound`). Parse errors and
    /// unknown-schema files are swallowed and surface as `Self::default()`.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(s) if s.schema_version == CURRENT_SCHEMA => Ok(s),
            Ok(s) => {
                tracing::warn!(
                    "DHW persist: unknown schema_version {}; ignoring file at {}",
                    s.schema_version,
                    path.display()
                );
                Ok(Self::default())
            }
            Err(e) => {
                tracing::warn!(
                    "DHW persist: failed to parse {}: {e}; starting fresh",
                    path.display()
                );
                Ok(Self::default())
            }
        }
    }
}

/// Snapshot returned by `GET /dhw/state` and read by the dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct DhwSnapshot {
    pub comfort_level: crate::dhw::error::ComfortLevel,
    pub boost: Option<DhwBoostSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DhwBoostSnapshot {
    pub preset: BoostPreset,
    pub started_at: DateTime<Utc>,
    pub scheduled_end: DateTime<Utc>,
    pub elapsed_s: i64,
    pub remaining_s: i64,
    pub immersion_engaged: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::tempdir;

    #[test]
    fn persisted_state_roundtrip() {
        let snap = DhwPersistedState {
            schema_version: 1,
            boost: Some(DhwBoostState {
                preset: BoostPreset::Bath { hours: 1.5 },
                started_at: Utc::now(),
                duration_secs: 5400,
                prior_immersion_engage_temp_c: Some(60.0),
                immersion_engaged: true,
            }),
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: DhwPersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema_version, 1);
        let boost = back.boost.unwrap();
        assert!(
            matches!(boost.preset, BoostPreset::Bath { hours } if (hours - 1.5).abs() < f32::EPSILON)
        );
        assert!(boost.immersion_engaged);
    }

    #[test]
    fn atomic_save_then_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("dhw_state.json");
        let snap = DhwPersistedState {
            schema_version: 1,
            boost: Some(DhwBoostState {
                preset: BoostPreset::Shower,
                started_at: Utc::now(),
                duration_secs: 1800,
                prior_immersion_engage_temp_c: None,
                immersion_engaged: false,
            }),
        };
        snap.save(&path).unwrap();
        let loaded = DhwPersistedState::load(&path).unwrap();
        assert!(matches!(
            loaded.boost.as_ref().unwrap().preset,
            BoostPreset::Shower
        ));
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.json");
        let loaded = DhwPersistedState::load(&path).unwrap();
        assert_eq!(loaded.schema_version, 1);
        assert!(loaded.boost.is_none());
    }

    #[test]
    fn unknown_schema_version_loads_empty_with_warning() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, r#"{"schema_version": 999, "boost": null}"#).unwrap();
        let loaded = DhwPersistedState::load(&path).unwrap();
        assert_eq!(loaded.schema_version, 1);
        assert!(loaded.boost.is_none());
    }

    #[test]
    fn corrupt_json_loads_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("corrupt.json");
        std::fs::write(&path, "this is not json").unwrap();
        let loaded = DhwPersistedState::load(&path).unwrap();
        assert!(loaded.boost.is_none());
    }

    #[test]
    fn save_uses_tmp_then_rename() {
        // Property: after save() the .tmp file should NOT exist alongside the main file
        let dir = tempdir().unwrap();
        let path = dir.path().join("dhw_state.json");
        DhwPersistedState::default().save(&path).unwrap();
        assert!(path.exists());
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists());
    }
}
