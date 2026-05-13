use super::*;

fn tmp_db() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("ctc.redb")).expect("open");
    (dir, store)
}

#[test]
fn empty_store_has_default_accumulators() {
    let (_dir, store) = tmp_db();
    assert_eq!(store.accumulators(), Accumulators::default());
    assert!(store.series_range(Sensor::Room, 0, i64::MAX).is_empty());
}

#[test]
fn record_sample_serves_from_ram() {
    let (_dir, store) = tmp_db();
    let t = SystemTime::now();
    let s = unix_secs(t).unwrap();
    store.record_sample(Sensor::Room, t, 21.5).unwrap();
    let pts = store.series_range(Sensor::Room, s - 1, s + 1);
    assert_eq!(pts.len(), 1);
    assert!((pts[0].1 - 21.5).abs() < f32::EPSILON);
}

#[test]
fn latest_sample_returns_most_recent() {
    let (_dir, store) = tmp_db();
    assert!(store.latest_sample(Sensor::Room).is_none());

    let t0 = SystemTime::now();
    let t1 = t0 + std::time::Duration::from_secs(5);
    store.record_sample(Sensor::Room, t0, 19.0).unwrap();
    store.record_sample(Sensor::Room, t1, 20.5).unwrap();

    let (ts, v) = store.latest_sample(Sensor::Room).unwrap();
    assert_eq!(ts, unix_secs(t1).unwrap());
    assert!((v - 20.5).abs() < f32::EPSILON);

    // Different sensor still empty.
    assert!(store.latest_sample(Sensor::Outdoor).is_none());
}

#[test]
fn flush_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    let now = SystemTime::now();
    let s = unix_secs(now).unwrap();
    {
        let store = Store::open(&path).unwrap();
        store.record_sample(Sensor::Outdoor, now, -3.0).unwrap();
        store.set_accumulators(Accumulators {
            tracking_started_unix_secs: 42,
            total_starts: 7,
            total_operating_secs: 1234,
        });
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    let acc = store.accumulators();
    assert_eq!(acc.total_starts, 7);
    assert_eq!(acc.total_operating_secs, 1234);
    // Raw samples are RAM-only; a fresh open starts with an empty ring.
    assert!(
        store.series_range(Sensor::Outdoor, s - 1, s + 1).is_empty(),
        "series ring should be empty after restart"
    );
}

#[test]
fn flush_is_idempotent_when_clean() {
    let (_dir, store) = tmp_db();
    // No record_*; nothing dirty.
    store.flush().unwrap();
    store.flush().unwrap();
}

#[test]
fn record_sample_drops_non_finite() {
    let (_dir, store) = tmp_db();
    let t = SystemTime::now();
    store.record_sample(Sensor::Room, t, f32::NAN).unwrap();
    store.record_sample(Sensor::Room, t, f32::INFINITY).unwrap();
    store
        .record_sample(Sensor::Room, t, f32::NEG_INFINITY)
        .unwrap();
    assert!(store.series_range(Sensor::Room, 0, i64::MAX).is_empty());
    assert!(store.latest_sample(Sensor::Room).is_none());
}

#[test]
fn ring_drops_older_than_retention() {
    let (_dir, store) = tmp_db();
    let now = SystemTime::now();
    let old = now - std::time::Duration::from_secs(25 * 3600);
    store.record_sample(Sensor::Room, old, 10.0).unwrap();
    store.record_sample(Sensor::Room, now, 20.0).unwrap();
    let pts = store.series_range(Sensor::Room, 0, i64::MAX);
    assert_eq!(pts.len(), 1, "old sample should have been evicted");
    assert!((pts[0].1 - 20.0).abs() < f32::EPSILON);
}

/// Eviction predicate is `t < cutoff` (strictly less than). Samples whose
/// timestamp equals exactly `now - SERIES_RETENTION_SECS` must be retained.
/// Regression: a flip to `<=` would silently drop the boundary sample.
#[test]
fn ring_retains_sample_exactly_at_retention_boundary() {
    let (_dir, store) = tmp_db();
    let now = SystemTime::now();
    // Place a sample at exactly the boundary: now - SERIES_RETENTION_SECS.
    // record_sample drops samples STRICTLY older than that, so this one stays.
    let boundary =
        now - std::time::Duration::from_secs(crate::storage::SERIES_RETENTION_SECS as u64);
    store.record_sample(Sensor::Room, boundary, 7.5).unwrap();
    // A second sample at "now" triggers the retention sweep.
    store.record_sample(Sensor::Room, now, 8.0).unwrap();
    let pts = store.series_range(Sensor::Room, 0, i64::MAX);
    assert_eq!(pts.len(), 2, "boundary sample must be retained");
    assert!((pts[0].1 - 7.5).abs() < f32::EPSILON);
}

#[test]
fn cycle_and_daily_persist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    let started = SystemTime::now();
    {
        let store = Store::open(&path).unwrap();
        store
            .record_cycle(
                started,
                CycleBlob {
                    timestamp: "2026-05-10T15:00:00Z".to_string(),
                    duration_secs: 120,
                    outdoor_temp_c: Some(-1.0),
                },
            )
            .unwrap();
        store.upsert_daily(
            20_260_510,
            DailyBlob {
                date: "2026-05-10".to_string(),
                starts: 5,
                operating_hours: 1.5,
                avg_outdoor_temp_c: Some(-2.0),
            },
        );
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    let since = unix_secs(started).unwrap() - 10;
    let recent = store.recent_cycles(since, 10).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].duration_secs, 120);

    let daily = store.all_daily().unwrap();
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].starts, 5);
}

#[test]
fn schema_v2_migration_is_idempotent_across_reopens() {
    // After migration the v1 table is dropped, so opening v2 → reopening
    // must not re-run the migration nor lose any data.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    let started = SystemTime::now();
    {
        let store = Store::open(&path).unwrap();
        store
            .record_cycle(
                started,
                CycleBlob {
                    timestamp: "2026-05-10T15:00:00Z".to_string(),
                    duration_secs: 91,
                    outdoor_temp_c: None,
                },
            )
            .unwrap();
        store.flush().unwrap();
    }
    // Reopen → reopen → reopen. Each open must leave the cycle list intact.
    for _ in 0..3 {
        let store = Store::open(&path).unwrap();
        let since = unix_secs(started).unwrap() - 10;
        let cycles = store.recent_cycles(since, 10).unwrap();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].duration_secs, 91);
    }
}

#[test]
fn two_cycles_same_second_both_survive_flush_reopen() {
    // Two cycles whose started_at falls in the same UTC second must both
    // persist across a flush + reopen. The bug being guarded against: keying
    // CYCLES by unix_secs(started_at) lets redb::insert overwrite, silently
    // dropping one of the cycles.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    let started = SystemTime::now();
    {
        let store = Store::open(&path).unwrap();
        store
            .record_cycle(
                started,
                CycleBlob {
                    timestamp: "2026-05-10T15:00:00Z".to_string(),
                    duration_secs: 111,
                    outdoor_temp_c: Some(-1.0),
                },
            )
            .unwrap();
        store
            .record_cycle(
                started,
                CycleBlob {
                    timestamp: "2026-05-10T15:00:00Z".to_string(),
                    duration_secs: 222,
                    outdoor_temp_c: Some(-2.0),
                },
            )
            .unwrap();
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    let since = unix_secs(started).unwrap() - 10;
    let recent = store.recent_cycles(since, 10).unwrap();
    assert_eq!(
        recent.len(),
        2,
        "both same-second cycles must survive flush + reopen"
    );
    let mut durations: Vec<u64> = recent.iter().map(|c| c.duration_secs).collect();
    durations.sort_unstable();
    assert_eq!(durations, vec![111, 222]);
}

#[test]
fn migrate_from_legacy_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json_path = dir.path().join("heatpump_stats.json");
    let legacy = serde_json::json!({
        "schema_version": 1,
        "tracking_started_unix_secs": 1_700_000_000_u64,
        "total_starts": 99,
        "total_operating_secs": 86_400,
        "cycle_history": [{
            "timestamp": "2026-05-10T15:00:00+00:00",
            "duration_secs": 300,
            "outdoor_temp_c": -1.5
        }],
        "daily_history": [{
            "date": "2026-05-10",
            "starts": 4,
            "operating_hours": 1.2,
            "avg_outdoor_temp_c": -2.1
        }]
    });
    std::fs::write(&json_path, serde_json::to_vec(&legacy).unwrap()).unwrap();

    let store = Store::open(dir.path().join("ctc.redb")).unwrap();
    let migrated = store.migrate_from_legacy_json(&json_path).unwrap();
    assert!(migrated);
    assert_eq!(store.accumulators().total_starts, 99);
    assert_eq!(store.accumulators().total_operating_secs, 86_400);

    let cycles = store.recent_cycles(0, 10).unwrap();
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].duration_secs, 300);

    let daily = store.all_daily().unwrap();
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0].starts, 4);

    // Original file should have been renamed.
    assert!(!json_path.exists());
    assert!(json_path.with_extension("json.migrated").exists());
}

#[test]
fn migrate_skips_when_tracking_started_set_alone() {
    // Counters are 0 but tracking_started_unix_secs > 0 — the store has
    // been initialized (e.g. by a prior migration) and we must not re-run
    // migration on top of it.
    let dir = tempfile::tempdir().expect("tempdir");
    let json_path = dir.path().join("heatpump_stats.json");
    let legacy = serde_json::json!({
        "schema_version": 1,
        "tracking_started_unix_secs": 1_700_000_000_u64,
        "total_starts": 7,
        "total_operating_secs": 42,
        "cycle_history": [],
        "daily_history": []
    });
    std::fs::write(&json_path, serde_json::to_vec(&legacy).unwrap()).unwrap();

    let store = Store::open(dir.path().join("ctc.redb")).unwrap();
    store.set_accumulators(Accumulators {
        tracking_started_unix_secs: 999,
        total_starts: 0,
        total_operating_secs: 0,
    });
    store.flush().unwrap();

    let migrated = store.migrate_from_legacy_json(&json_path).unwrap();
    assert!(
        !migrated,
        "should skip when tracking_started_unix_secs is set"
    );
    assert_eq!(
        store.accumulators().tracking_started_unix_secs,
        999,
        "tracking_started_unix_secs should not be overwritten"
    );
    assert!(json_path.exists(), "legacy file should be left alone");
}

#[test]
fn migrate_skips_when_store_already_populated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json_path = dir.path().join("heatpump_stats.json");
    std::fs::write(&json_path, b"{}").unwrap(); // shape doesn't matter — we shouldn't read it

    let store = Store::open(dir.path().join("ctc.redb")).unwrap();
    store.set_accumulators(Accumulators {
        tracking_started_unix_secs: 0,
        total_starts: 1,
        total_operating_secs: 0,
    });
    store.flush().unwrap();

    let migrated = store.migrate_from_legacy_json(&json_path).unwrap();
    assert!(!migrated, "should not run when store already has data");
    assert!(json_path.exists(), "legacy file should be left alone");
}

#[test]
fn parse_date_yyyymmdd_rejects_calendar_invalid() {
    // Valid dates round-trip into the YYYYMMDD packed form.
    assert_eq!(parse_date_yyyymmdd("2025-02-28"), Some(20_250_228));
    assert_eq!(parse_date_yyyymmdd("2024-02-29"), Some(20_240_229)); // leap year
    // Calendar-invalid dates that the previous range-only check accepted
    // must now be rejected.
    assert_eq!(parse_date_yyyymmdd("2025-02-31"), None);
    assert_eq!(parse_date_yyyymmdd("2023-02-29"), None); // not a leap year
    assert_eq!(parse_date_yyyymmdd("2025-04-31"), None); // April has 30 days
    assert_eq!(parse_date_yyyymmdd("2025-13-01"), None); // bad month
    assert_eq!(parse_date_yyyymmdd("not-a-date"), None);
}
