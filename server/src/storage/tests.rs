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
    // Sample-persistence behavior is covered by the SERIES_MINUTES round-trip
    // tests below.
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
    let old = now - std::time::Duration::from_hours(25);
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
fn sensor_from_slug_round_trips_known_and_rejects_unknown() {
    // A representative spread including the dash/underscore aliases.
    assert_eq!(Sensor::from_slug("room"), Some(Sensor::Room));
    assert_eq!(Sensor::from_slug("flow_sp"), Some(Sensor::FlowSp));
    assert_eq!(Sensor::from_slug("flow-sp"), Some(Sensor::FlowSp));
    assert_eq!(Sensor::from_slug("hp-status"), Some(Sensor::HpStatus));
    assert_eq!(Sensor::from_slug("brine_out"), Some(Sensor::BrineOut));
    assert_eq!(Sensor::from_slug("nope"), None);
    // Stable numeric ids are exposed via as_u16.
    assert_eq!(Sensor::Room.as_u16(), 1);
    assert_eq!(Sensor::HpStatus.as_u16(), 19);
}

#[test]
fn bucket_minutes_empty_sensor_returns_empty() {
    let (_dir, store) = tmp_db();
    // Sensor with no ring entries → early-return Vec::new().
    assert!(
        store
            .bucket_minutes(Sensor::Outdoor, 0, i64::MAX)
            .is_empty()
    );
}

#[test]
fn series_range_filters_outside_window() {
    let (_dir, store) = tmp_db();
    let t = SystemTime::now();
    let s = unix_secs(t).unwrap();
    store.record_sample(Sensor::Room, t, 21.0).unwrap();
    // Window entirely before the sample → filtered out (data present, range miss).
    assert!(store.series_range(Sensor::Room, s - 100, s - 50).is_empty());
    // Window entirely after → also empty.
    assert!(store.series_range(Sensor::Room, s + 50, s + 100).is_empty());
}

#[test]
fn step_events_record_serve_and_survive_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    {
        let store = Store::open(&path).unwrap();
        store.record_step_event(StepEventBlob {
            started_at: 1_700_000_000,
            flow_before: 30.0,
            flow_after: 40.0,
            return_before: 28.0,
            samples: vec![(0, 30.0, 28.0), (5, 35.0, 29.0)],
        });
        store.record_step_event(StepEventBlob {
            started_at: 1_700_000_100,
            flow_before: 31.0,
            flow_after: 41.0,
            return_before: 29.0,
            samples: vec![],
        });
        // In-memory cache serves newest-first immediately.
        let recent = store.recent_step_events(10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].started_at, 1_700_000_100);
        // limit is honored.
        assert_eq!(store.recent_step_events(1).len(), 1);
        store.flush().unwrap();
    }
    // Reopen hydrates step_events_cache from disk (newest first).
    let store = Store::open(&path).unwrap();
    let recent = store.recent_step_events(10);
    assert_eq!(recent.len(), 2, "step events must survive flush + reopen");
    assert_eq!(recent[0].started_at, 1_700_000_100);
}

#[test]
fn step_events_pruned_to_max_on_flush() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    {
        let store = Store::open(&path).unwrap();
        // Record more than MAX_STEP_EVENTS distinct events.
        for i in 0..(MAX_STEP_EVENTS as u64 + 10) {
            store.record_step_event(StepEventBlob {
                started_at: 1_700_000_000 + i,
                flow_before: 30.0,
                flow_after: 40.0,
                return_before: 28.0,
                samples: vec![],
            });
        }
        store.flush().unwrap();
    }
    // After reopen, the on-disk table was pruned to MAX_STEP_EVENTS, so the
    // hydrated cache holds exactly that many (newest retained).
    let store = Store::open(&path).unwrap();
    let recent = store.recent_step_events(usize::MAX);
    assert_eq!(recent.len(), MAX_STEP_EVENTS);
    // Newest event must be present; oldest beyond the cap dropped.
    assert_eq!(
        recent[0].started_at,
        1_700_000_000 + MAX_STEP_EVENTS as u64 + 9
    );
}

#[test]
fn step_event_timestamp_overflowing_i64_is_clamped_not_dropped() {
    // started_at beyond i64::MAX (only representable as u64) must be kept,
    // clamped to i64::MAX, and survive a flush+reopen rather than being
    // silently discarded. Exercises the i64::try_from-fails clamp branch.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    {
        let store = Store::open(&path).unwrap();
        store.record_step_event(StepEventBlob {
            started_at: u64::MAX, // > i64::MAX → clamp path
            flow_before: 30.0,
            flow_after: 40.0,
            return_before: 28.0,
            samples: vec![],
        });
        // Immediately served from the in-memory cache (value preserved).
        let recent = store.recent_step_events(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].started_at, u64::MAX);
        store.flush().unwrap();
    }
    // The disk key was clamped to i64::MAX; reopen rehydrates the event.
    let store = Store::open(&path).unwrap();
    let recent = store.recent_step_events(10);
    assert_eq!(recent.len(), 1, "clamped event must survive reopen");
    assert_eq!(recent[0].started_at, u64::MAX);
}

#[test]
fn recent_cycles_caps_at_limit_from_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    let base = SystemTime::now();
    {
        let store = Store::open(&path).unwrap();
        for i in 0..5u64 {
            store
                .record_cycle(
                    base + std::time::Duration::from_secs(i),
                    CycleBlob {
                        timestamp: format!("2026-05-10T15:00:0{i}Z"),
                        duration_secs: 100 + i,
                        outdoor_temp_c: None,
                    },
                )
                .unwrap();
        }
        store.flush().unwrap();
    }
    // Reopen so pending is empty and all reads come from disk.
    let store = Store::open(&path).unwrap();
    let since = unix_secs(base).unwrap() - 10;
    let recent = store.recent_cycles(since, 2).unwrap();
    assert_eq!(recent.len(), 2, "disk read must respect the limit cap");
}

#[test]
fn recent_cycles_skips_entries_before_since() {
    let (_dir, store) = tmp_db();
    let base = SystemTime::now();
    // One old cycle and one recent cycle, both pending in RAM.
    store
        .record_cycle(
            base - std::time::Duration::from_secs(10_000),
            CycleBlob {
                timestamp: "old".to_string(),
                duration_secs: 1,
                outdoor_temp_c: None,
            },
        )
        .unwrap();
    store
        .record_cycle(
            base,
            CycleBlob {
                timestamp: "new".to_string(),
                duration_secs: 2,
                outdoor_temp_c: None,
            },
        )
        .unwrap();
    let since = unix_secs(base).unwrap() - 5;
    let recent = store.recent_cycles(since, 10).unwrap();
    assert_eq!(recent.len(), 1, "the pre-since cycle must be skipped");
    assert_eq!(recent[0].timestamp, "new");
}

#[test]
fn recent_cycles_pending_limit_returns_early() {
    let (_dir, store) = tmp_db();
    let base = SystemTime::now();
    for i in 0..4u64 {
        store
            .record_cycle(
                base + std::time::Duration::from_secs(i),
                CycleBlob {
                    timestamp: format!("c{i}"),
                    duration_secs: i,
                    outdoor_temp_c: None,
                },
            )
            .unwrap();
    }
    let since = unix_secs(base).unwrap() - 5;
    // Limit smaller than pending count → early return out of the RAM loop.
    let recent = store.recent_cycles(since, 2).unwrap();
    assert_eq!(recent.len(), 2);
}

#[test]
fn migrate_from_legacy_json_corrupt_file_is_left_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json_path = dir.path().join("heatpump_stats.json");
    // Valid JSON but not the expected shape → parse_legacy_json returns None.
    std::fs::write(&json_path, br#"{"unexpected":"shape"}"#).unwrap();

    let store = Store::open(dir.path().join("ctc.redb")).unwrap();
    let migrated = store.migrate_from_legacy_json(&json_path).unwrap();
    assert!(!migrated, "unparseable legacy file must not migrate");
    assert!(json_path.exists(), "corrupt file is left untouched");
    assert_eq!(store.accumulators(), Accumulators::default());
}

#[test]
fn migrate_from_legacy_json_missing_file_is_noop() {
    let (dir, store) = tmp_db();
    let absent = dir.path().join("does_not_exist.json");
    let migrated = store.migrate_from_legacy_json(&absent).unwrap();
    assert!(!migrated, "a missing legacy file is a no-op, not an error");
}

#[test]
fn v1_to_v2_migration_copies_cycle_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    // Hand-build a v1-schema database: META schema_version = 1 and a CYCLES_V1
    // row keyed by i64 seconds. open() must migrate it into the v2 table.
    {
        let db = Database::create(&path).unwrap();
        let w = db.begin_write().unwrap();
        {
            let mut m = w.open_table(META).unwrap();
            m.insert(SCHEMA_KEY, 1u32).unwrap();
            let blob = CycleBlob {
                timestamp: "2026-05-10T15:00:00Z".to_string(),
                duration_secs: 321,
                outdoor_temp_c: Some(-4.0),
            };
            let buf = serde_json::to_vec(&blob).unwrap();
            let mut c = w.open_table(CYCLES_V1).unwrap();
            c.insert(1_700_000_000i64, buf.as_slice()).unwrap();
        }
        w.commit().unwrap();
        drop(db); // release the file lock before Store::open reopens it
    }
    // open() runs migrate_cycles_v1_to_v2 because stored_version == Some(1).
    {
        let store = Store::open(&path).unwrap();
        let recent = store.recent_cycles(0, 10).unwrap();
        assert_eq!(recent.len(), 1, "v1 cycle row must migrate into v2");
        assert_eq!(recent[0].duration_secs, 321);
    }

    // Reopen: schema is now v3 and the v1 table is gone, so no re-migration
    // and the row is still served.
    let store2 = Store::open(&path).unwrap();
    assert_eq!(store2.recent_cycles(0, 10).unwrap().len(), 1);
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

// ---- SERIES_MINUTES bucketing, hydration, and pruning ----

fn assert_float_eq(a: f32, b: f32, msg: &str) {
    assert!(
        (a - b).abs() < 1e-4,
        "{msg}: expected {b}, got {a} (diff {})",
        (a - b).abs()
    );
}

/// Build a `SystemTime` that's `secs_back` seconds before now. Helper for
/// tests that want timestamps definitely "older than the current minute"
/// so the bucket finalizes on flush.
fn secs_ago(secs_back: u64) -> SystemTime {
    SystemTime::now() - std::time::Duration::from_secs(secs_back)
}

#[test]
fn bucketing_averages_samples_within_minute() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    // Two samples 300 s in the past — same wall-clock minute, so they
    // collapse into one bucket whose mean is (20.0 + 22.0) / 2 = 21.0.
    let t = secs_ago(300);
    {
        let store = Store::open(&path).unwrap();
        store.record_sample(Sensor::Room, t, 20.0).unwrap();
        store.record_sample(Sensor::Room, t, 22.0).unwrap();
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    let pts = store.series_range(Sensor::Room, 0, i64::MAX);
    assert_eq!(pts.len(), 1, "two same-minute samples must collapse to one");
    assert_float_eq(pts[0].1, 21.0, "bucket mean");
}

#[test]
fn bucketing_separates_boundary_samples() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    // Choose two timestamps in the past that straddle a minute boundary:
    // 1 s before and 1 s after the start of some minute that's already
    // closed by the time flush runs.
    let now_secs = unix_secs(SystemTime::now()).unwrap();
    let boundary = now_secs - (now_secs % 60) - 120;
    let t_before = std::time::UNIX_EPOCH
        + std::time::Duration::from_secs(u64::try_from(boundary - 1).unwrap());
    let t_after = std::time::UNIX_EPOCH
        + std::time::Duration::from_secs(u64::try_from(boundary + 1).unwrap());
    {
        let store = Store::open(&path).unwrap();
        store.record_sample(Sensor::Room, t_before, 10.0).unwrap();
        store.record_sample(Sensor::Room, t_after, 30.0).unwrap();
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    let pts = store.series_range(Sensor::Room, 0, i64::MAX);
    assert_eq!(
        pts.len(),
        2,
        "samples either side of a minute boundary must yield distinct rows"
    );
    let mut values: Vec<f32> = pts.iter().map(|(_, v)| *v).collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_float_eq(values[0], 10.0, "older bucket");
    assert_float_eq(values[1], 30.0, "newer bucket");
}

#[test]
fn open_bucket_not_finalized_until_minute_closes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    // Use a timestamp ~1 minute in the future so the bucket key is reliably
    // >= whatever current_minute flush() computes — bulletproof against the
    // wall-clock minute boundary rolling over between record_sample and
    // flush. flush() must leave this open bucket in pending and write
    // nothing to SERIES_MINUTES.
    let future = SystemTime::now() + std::time::Duration::from_mins(1);
    {
        let store = Store::open(&path).unwrap();
        store.record_sample(Sensor::Room, future, 19.0).unwrap();
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    // Hydration only pulls rows out of SERIES_MINUTES, so an open bucket
    // that never landed on disk leaves the ring empty.
    assert!(
        store.series_range(Sensor::Room, 0, i64::MAX).is_empty(),
        "open-minute bucket must not be persisted"
    );
}

#[test]
fn hydration_populates_ring_for_dashboard() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    // Three closed minutes in the past, ascending: 5m, 4m, 3m ago.
    let t5 = secs_ago(300);
    let t4 = secs_ago(240);
    let t3 = secs_ago(180);
    {
        let store = Store::open(&path).unwrap();
        store.record_sample(Sensor::Outdoor, t5, -1.0).unwrap();
        store.record_sample(Sensor::Outdoor, t4, -2.0).unwrap();
        store.record_sample(Sensor::Outdoor, t3, -3.0).unwrap();
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    let pts = store.series_range(Sensor::Outdoor, 0, i64::MAX);
    assert_eq!(pts.len(), 3, "all three closed minutes must hydrate");
    // Iteration order on the SERIES_MINUTES table is ascending by key, so
    // the ring is rebuilt in chronological order.
    for w in pts.windows(2) {
        assert!(
            w[0].0 < w[1].0,
            "hydrated ring must be in ascending timestamp order"
        );
    }
    // Values match the recorded samples (each in its own minute).
    let values: Vec<f32> = pts.iter().map(|(_, v)| *v).collect();
    assert_float_eq(values[0], -1.0, "oldest bucket");
    assert_float_eq(values[1], -2.0, "middle bucket");
    assert_float_eq(values[2], -3.0, "newest closed bucket");
}

#[test]
fn prune_evicts_rows_older_than_24h() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    // Two samples: one past the 24h retention boundary (must be pruned), one
    // 5m old (must survive).
    let too_old = secs_ago(u64::try_from(crate::storage::SERIES_RETENTION_SECS).unwrap() + 3600);
    let fresh = secs_ago(300);
    {
        let store = Store::open(&path).unwrap();
        store.record_sample(Sensor::Room, too_old, 99.0).unwrap();
        store.record_sample(Sensor::Room, fresh, 21.0).unwrap();
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    let pts = store.series_range(Sensor::Room, 0, i64::MAX);
    assert_eq!(pts.len(), 1, "25h-old bucket must be pruned");
    assert_float_eq(pts[0].1, 21.0, "fresh bucket survives");
}

#[test]
fn non_finite_samples_stay_out_of_pending_minutes() {
    // record_sample drops non-finite values before they reach the ring or
    // pending_minutes, so flush + reopen must persist nothing.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ctc.redb");
    let t = secs_ago(300);
    {
        let store = Store::open(&path).unwrap();
        store.record_sample(Sensor::Room, t, f32::NAN).unwrap();
        store.record_sample(Sensor::Room, t, f32::INFINITY).unwrap();
        store
            .record_sample(Sensor::Room, t, f32::NEG_INFINITY)
            .unwrap();
        // No finite samples → nothing dirty → flush is a no-op.
        store.flush().unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert!(store.series_range(Sensor::Room, 0, i64::MAX).is_empty());
}

#[test]
fn bucket_minutes_helper_collapses_subminute_samples() {
    let (_dir, store) = tmp_db();
    // Step back to the minute floor so both samples land in the same minute.
    // Anchoring on the floor (not raw `now`) keeps the test from flaking when
    // it runs in a minute's final second, where `now + 1s` would spill into the
    // next minute and produce two buckets instead of one.
    let now = SystemTime::now();
    let now_secs = unix_secs(now).unwrap();
    let into_minute = u64::try_from(now_secs.rem_euclid(60)).unwrap();
    let base = now - std::time::Duration::from_secs(into_minute);
    let expected_minute = now_secs - now_secs.rem_euclid(60);
    // Two sub-minute samples on the same minute → one mean point.
    store.record_sample(Sensor::Room, base, 18.0).unwrap();
    store
        .record_sample(Sensor::Room, base + std::time::Duration::from_secs(1), 22.0)
        .unwrap();
    let pts = store.bucket_minutes(Sensor::Room, 0, i64::MAX);
    assert_eq!(pts.len(), 1, "sub-minute samples must collapse");
    assert_float_eq(pts[0].1, 20.0, "bucket mean");
    // Bucket timestamp is the minute floor of the sample time.
    assert_eq!(pts[0].0, expected_minute, "bucket key = minute floor");
}
