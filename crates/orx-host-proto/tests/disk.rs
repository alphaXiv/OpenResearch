use orx_host_proto::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

fn fixtures() -> Vec<Value> {
    std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect()
}

fn roundtrip<T: DeserializeOwned + Serialize>(value: &Value) {
    let decoded: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
}

#[test]
fn every_historical_disk_fixture_roundtrips() {
    for fixture in fixtures() {
        for value in fixture["launch"].as_array().unwrap() {
            roundtrip::<LaunchIntent>(value);
        }
        for value in fixture["identity"].as_array().unwrap() {
            roundtrip::<ProcessIdentity>(value);
        }
        for value in fixture["exit"].as_array().unwrap() {
            roundtrip::<ExitReceipt>(value);
        }
        for value in fixture["cleanup"].as_array().unwrap() {
            roundtrip::<ProcessOutcome>(value);
        }
    }
}

#[test]
fn todays_disk_writer_matches_an_append_only_fixture() {
    let fixtures = fixtures();
    let check = |kind: &str, value: Value| {
        assert!(
            fixtures
                .iter()
                .any(|fixture| fixture[kind].as_array().unwrap().contains(&value)),
            "unfrozen {kind}: {value}"
        );
    };
    let mut launch = LaunchIntent {
        run: "r1".into(),
        digest: "a".repeat(64),
        run_sh: "true".into(),
        env: Default::default(),
        source: None,
        cancel_sh: None,
    };
    check("launch", serde_json::to_value(&launch).unwrap());
    launch.run_sh = "bash repo/run.sh".into();
    launch.env.insert("TOKEN".into(), "value".into());
    launch.source = Some("b".repeat(64));
    launch.cancel_sh = Some("true".into());
    check("launch", serde_json::to_value(launch).unwrap());
    check(
        "identity",
        serde_json::to_value(ProcessIdentity {
            node: "node-id".into(),
            boot_id: "boot-id".into(),
            pid: 42,
            starttime: 1234,
        })
        .unwrap(),
    );
    for reason in [
        ExitReason::Exited,
        ExitReason::Cancelled,
        ExitReason::SpawnFailed,
    ] {
        let code = (reason == ExitReason::Exited).then_some(0);
        check(
            "exit",
            serde_json::to_value(ExitReceipt {
                finished_at: Some(1234567),
                code,
                signal: None,
                reason,
                processes: vec![],
                outcome: CancelOutcome::Verified,
            })
            .unwrap(),
        );
    }
    for (processes, outcome) in [
        (vec![], CancelOutcome::Verified),
        (
            vec![Survivor {
                pid: 43,
                starttime: 5678,
                state: "S".into(),
            }],
            CancelOutcome::Survivors,
        ),
        (
            vec![],
            CancelOutcome::Unverifiable {
                unreadable: vec![44],
            },
        ),
    ] {
        check(
            "cleanup",
            serde_json::to_value(ProcessOutcome {
                processes: processes.clone(),
                outcome: outcome.clone(),
            })
            .unwrap(),
        );
        if outcome != CancelOutcome::Verified {
            let cancelled = matches!(outcome, CancelOutcome::Unverifiable { .. });
            check(
                "exit",
                serde_json::to_value(ExitReceipt {
                    finished_at: Some(1234567),
                    code: (!cancelled).then_some(0),
                    signal: cancelled.then_some(15),
                    reason: if cancelled {
                        ExitReason::Cancelled
                    } else {
                        ExitReason::Exited
                    },
                    processes,
                    outcome,
                })
                .unwrap(),
            );
        }
    }
}

#[test]
fn required_proof_never_defaults_and_unknown_variants_are_rejected() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/disk-v1.json")).unwrap();
    for field in ["node", "boot_id", "pid", "starttime"] {
        let mut value = fixture["identity"][0].clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ProcessIdentity>(value).is_err());
    }
    for field in ["reason", "processes", "outcome"] {
        let mut value = fixture["exit"][0].clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ExitReceipt>(value).is_err());
    }
    for field in ["processes", "outcome"] {
        let mut value = fixture["cleanup"][0].clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ProcessOutcome>(value).is_err());
    }
    let mut value = fixture["exit"][0].clone();
    value["reason"] = "future".into();
    assert!(serde_json::from_value::<ExitReceipt>(value).is_err());
    let mut value = fixture["exit"][0].clone();
    value["outcome"]["kind"] = "future".into();
    assert!(serde_json::from_value::<ExitReceipt>(value).is_err());
}
