#![cfg(unix)]
#![allow(clippy::disallowed_methods)]
use orx_host::signals;
use std::{fs, io::Write, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("orx-signals-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn log(&self) -> PathBuf {
        self.0.join("log")
    }
    fn snapshot(&self) -> PathBuf {
        self.0.join("run-signals/r1.json")
    }
    fn append(&self, bytes: &[u8]) {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log())
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    fn scan(&self, terminal: bool) -> orx_signals::Snapshot {
        signals::scan(&self.log(), &self.snapshot(), "r1", terminal, 1000, None)
            .unwrap()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn publication_and_resume_preserve_cursor_and_do_not_repeat_rows() {
    let f = Fixture::new();
    f.append(b"::orx:: {\"step\":1,\"row\":\"eval\",\"m\":{\"score\":1}}\n::orx:: {\"step\":");
    let first = f.scan(false);
    assert_eq!(first.rows["eval"].len(), 1);
    assert!(!first.cursor.pending_base64.is_empty());
    f.append(b"2,\"row\":\"eval\",\"m\":{\"score\":2}}\n");
    let second = f.scan(true);
    assert_eq!(second.rows["eval"].len(), 2);
    assert_eq!(second.latest["score"], 2);
    assert_eq!(first.generation, second.generation);
    assert_eq!(second, f.scan(true));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(f.snapshot()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn replaced_and_truncated_logs_get_new_generations() {
    let f = Fixture::new();
    f.append(b"::orx:: {\"m\":{\"a\":1}}\n");
    let first = f.scan(false);
    fs::rename(f.log(), f.0.join("old-log")).unwrap();
    f.append(b"::orx:: {\"m\":{\"b\":2}}\n");
    let second = f.scan(false);
    assert_ne!(first.generation, second.generation);
    assert!(!second.latest.contains_key("a"));
    fs::write(f.log(), b"::orx:: {}\n").unwrap();
    let third = f.scan(true);
    assert_ne!(third.generation, second.generation);
    assert!(third.latest.is_empty());
    assert_eq!(third.coverage.reasons, Vec::<String>::new());
}

#[test]
fn malformed_checkpoint_is_retained_and_rebuilt_from_raw_log() {
    let f = Fixture::new();
    f.append(b"::orx:: {\"m\":{\"a\":1}}\n");
    let mut invalid_width = f.scan(false);
    invalid_width.series_widths.insert("a".into(), 0.0);
    for bytes in [
        b"{broken".to_vec(),
        serde_json::to_vec(&invalid_width).unwrap(),
    ] {
        fs::write(f.snapshot(), bytes).unwrap();
        let rebuilt = f.scan(true);
        assert_ne!(rebuilt.generation, invalid_width.generation);
        assert_eq!(rebuilt.latest["a"], 1);
        assert_eq!(rebuilt.diagnostics.counts["checkpoint_unreadable"], 1);
        assert!(fs::read_dir(f.snapshot().parent().unwrap())
            .unwrap()
            .any(|e| e
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .contains("corrupt-")));
    }
}

#[test]
fn failure_before_rename_retains_published_cursor() {
    let f = Fixture::new();
    f.append(b"::orx:: {\"m\":{\"a\":1}}\n");
    let before = f.scan(false);
    let mut changed = before.clone();
    changed.cursor.offset += 10;
    let invalid = f.snapshot().join("child");
    assert!(signals::publish(&invalid, &changed).is_err());
    assert_eq!(signals::load(&f.snapshot()).unwrap().unwrap(), before);
}

#[test]
fn two_processes_cannot_publish_one_run_simultaneously() {
    if std::env::var_os("ORX_SIGNALS_LOCK_CHILD").is_some() {
        let dir = PathBuf::from(std::env::var_os("ORX_SIGNALS_LOCK_CHILD").unwrap());
        let error = signals::scan(
            &dir.join("log"),
            &dir.join("run-signals/r1.json"),
            "r1",
            false,
            1000,
            None,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        return;
    }
    let f = Fixture::new();
    f.append(b"hello\n");
    f.scan(false);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.snapshot().with_extension("lock"))
        .unwrap();
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "two_processes_cannot_publish_one_run_simultaneously",
        ])
        .env("ORX_SIGNALS_LOCK_CHILD", &f.0)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn same_inode_rewrite_is_detected_by_cursor_anchor() {
    let f = Fixture::new();
    f.append(b"::orx:: {\"m\":{\"a\":1}}\n");
    let before = f.scan(false);
    fs::write(f.log(), b"::orx:: {\"m\":{\"b\":2}}\n").unwrap();
    let after = f.scan(false);
    assert_ne!(before.generation, after.generation);
    assert!(!after.latest.contains_key("a"));
    assert_eq!(after.latest["b"], 2);
}

#[test]
fn terminal_metadata_is_published_even_with_partial_signal_coverage() {
    let f = Fixture::new();
    for i in 0..65 {
        f.append(format!("::orx:: {{\"m\":{{\"metric{i}\":1}}}}\n").as_bytes());
    }
    let live = f.scan(false);
    assert_eq!(live.coverage.reasons, vec!["truncated"]);
    let final_snapshot = f.scan(true);
    assert!(final_snapshot.rev > live.rev);
    assert_eq!(
        signals::load(&f.snapshot()).unwrap().unwrap().extra["terminal"],
        true
    );
}

#[test]
fn terminal_checkpoint_outlives_the_raw_log_and_missing_live_log_is_explicit() {
    let f = Fixture::new();
    f.append(b"::orx:: {\"step\":1}\n");
    let saved = f.scan(true);
    fs::remove_file(f.log()).unwrap();
    assert_eq!(
        signals::scan(&f.log(), &f.snapshot(), "r1", true, 2000, None).unwrap(),
        Some(saved)
    );
    let empty = Fixture::new();
    assert!(
        signals::scan(&empty.log(), &empty.snapshot(), "r1", false, 0, None)
            .unwrap()
            .is_none()
    );
    empty.append(b"::orx:: {\"step\":1}\n");
    empty.scan(false);
    fs::remove_file(empty.log()).unwrap();
    let live = signals::scan(&empty.log(), &empty.snapshot(), "r1", false, 1, None)
        .unwrap()
        .unwrap();
    assert!(serde_json::to_value(live.coverage).unwrap()["reasons"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("log_missing")));
}

#[test]
fn populated_host_checkpoint_matches_the_disk_contract_both_ways() {
    let f = Fixture::new();
    f.append(include_bytes!(
        "../../orx-signals/tests/fixtures/checkpoint.txt"
    ));
    let mut snapshot = signals::scan(&f.log(), &f.snapshot(), "r1", true, 1000, Some(500))
        .unwrap()
        .unwrap();
    snapshot.cursor.ident = "1-100".into();
    snapshot.generation = "g1".into();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../orx-host-proto/tests/fixtures/run-signals-v1.json"
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), fixture);
    signals::publish(&f.snapshot(), &serde_json::from_value(fixture).unwrap()).unwrap();
    assert_eq!(signals::load(&f.snapshot()).unwrap().unwrap(), snapshot);
}

fn touch(log: &std::path::Path, ms: i64) {
    fs::File::options()
        .write(true)
        .open(log)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64))
        .unwrap();
}

/// A run that hangs while no one is reading, then a catch-up that needs more than one scan:
/// a scan that stops short moves nothing, and the one that reaches the end places the last
/// advance when the log last changed, so the hang shows.
#[test]
fn a_hang_during_an_outage_shows_after_a_catch_up_of_several_scans() {
    let f = Fixture::new();
    let scan = |now: i64| {
        signals::scan(&f.log(), &f.snapshot(), "r1", false, now, None)
            .unwrap()
            .unwrap()
    };
    let report = |step: u32| {
        let pad = "x".repeat(1000);
        format!(
            "::orx:: {{\"step\":{step},\"of\":9000,\"t\":{step},\"m\":{{\"pad\":\"{pad}\"}}}}\n"
        )
    };
    let started = 1_794_000_000_000_i64;
    for step in 0..10 {
        f.append(report(step).as_bytes());
    }
    touch(&f.log(), started + 9_000);
    assert_eq!(
        scan(started + 9_500).timing.anchor,
        Some((0, 9., started + 9_000))
    );

    let more: String = (10..5000).map(report).collect();
    f.append(more.as_bytes());
    let hung = started + 4_999_000;
    touch(&f.log(), hung);
    let read = hung + 3_600_000;
    let short = scan(read);
    assert!(short.coverage.reasons.contains(&"catching_up".into()));
    assert_eq!(short.timing.anchor, Some((0, 9., started + 9_000)));
    let caught_up = scan(read + 5_000);
    assert_eq!(caught_up.progress().unwrap().done, Some(4999.));
    assert_eq!(caught_up.timing.anchor, Some((0, 4999., hung)));
}

/// On a shared filesystem a log can carry a time ahead of the host's clock; the scan's own time
/// bounds it.
#[test]
fn a_log_written_ahead_of_the_hosts_clock_sits_at_the_scan() {
    let f = Fixture::new();
    f.append(b"::orx:: {\"step\":1,\"of\":4,\"t\":1}\n::orx:: {\"step\":2,\"t\":2}\n");
    let now = 1_794_000_000_000_i64;
    touch(&f.log(), now + 3_600_000);
    let scanned = signals::scan(&f.log(), &f.snapshot(), "r1", false, now, None)
        .unwrap()
        .unwrap();
    assert_eq!(scanned.timing.anchor, Some((0, 2., now)));
}

#[test]
fn new_patterns_fold_the_same_log_again_and_replace_nothing() {
    let f = Fixture::new();
    f.append(include_bytes!("../../orx-signals/tests/fixtures/codec.out"));
    let before = f.scan(true);
    assert_eq!(before.phases[0].source, "detected");
    assert!(before.latest.is_empty());

    let config: orx_signals::Config = serde_json::from_value(serde_json::json!({"patterns": [
        {"regex": "^chunk (?P<step>\\d+)/(?P<of>\\d+) done", "phase": "compress", "unit": "chunk"},
        {"regex": "^METRIC (?P<name>\\w+) (?P<value>\\S+)$"},
    ]}))
    .unwrap();
    assert!(signals::configure(&f.snapshot(), &config).unwrap());
    assert!(
        !signals::configure(&f.snapshot(), &config).unwrap(),
        "already that"
    );
    let after = f.scan(true);
    assert_ne!(after.generation, before.generation);
    assert_eq!(after.config_hash, config.hash());
    assert_eq!(after.phases[0].name, "compress");
    assert_eq!(after.latest["front_gain"], 0.0213);
    assert!(!after.coverage.reasons.iter().any(|r| r == "log_replaced"));
    assert_eq!(after, f.scan(true), "and resumes it after");

    fs::write(
        f.snapshot().with_extension("config.json"),
        b"{\"patterns\":[{\"regex\":\"(\"}]}",
    )
    .unwrap();
    let error = signals::scan(&f.log(), &f.snapshot(), "r1", true, 1000, None).unwrap_err();
    assert!(error.to_string().contains("pattern 1:"), "{error}");
}
