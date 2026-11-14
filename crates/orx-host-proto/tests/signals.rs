use orx_host_proto::*;
use serde_json::json;

#[test]
fn old_summaries_are_accepted_and_new_signals_stay_advisory() {
    let raw = json!({"state":{"phase":"running"},"log_len":4,"notify_len":0,"cleanup":null});
    let mut summary: Summary = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(summary.signals, None);
    summary.signals = Some(SignalsSummary {
        generation: "g1".into(),
        revision: 1,
        cursor: 4,
        coverage: json!({"complete":false}),
        latest: json!({"loss":"NaN"}),
        progress: json!({"done":100,"of":100}),
        diagnostics: Default::default(),
        error: None,
    });
    let value = serde_json::to_value(&summary).unwrap();
    #[derive(serde::Deserialize)]
    struct OldSummary {
        state: RunState,
        log_len: Option<u64>,
    }
    let old: OldSummary = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(old.state, RunState::Running);
    assert_eq!(old.log_len, Some(4));
    let rewritten: Summary = serde_json::from_value(value).unwrap();
    assert_eq!(rewritten, summary);
    assert_eq!(rewritten.state, RunState::Running);
}

#[test]
fn signal_rpc_pages_obey_binary_frame_contract() {
    for response in [
        Response::ReadSignals {
            page: SignalsPage::Data {
                generation: "g1".into(),
                revision: 1,
                offset: 0,
                total: 3,
                bytes: vec![1, 2, 3],
            },
            answered_at: 1_794_000_000_000,
        },
        Response::ReadSignals {
            page: SignalsPage::Reset {
                summary: Box::new(SignalsSummary {
                    generation: "g2".into(),
                    revision: 0,
                    cursor: 0,
                    coverage: json!({}),
                    latest: json!({}),
                    progress: serde_json::Value::Null,
                    diagnostics: Default::default(),
                    error: None,
                }),
            },
            answered_at: 1_794_000_000_000,
        },
    ] {
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert_eq!(
            read_frame::<Response>(&mut wire.as_slice()).unwrap(),
            Some(response)
        );
    }
}

#[test]
fn approved_signal_fixtures_roundtrip() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/signals-v1.json")).unwrap();
    let summary: SignalsSummary =
        serde_json::from_value(fixture["signalsSummary"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(summary).unwrap(),
        fixture["signalsSummary"]
    );
    let request: Request = serde_json::from_value(fixture["readSignals"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        fixture["readSignals"]
    );
}

/// Every answer that carries signals carries the host's clock; an older host's answer has none,
/// and an older reader ignores it.
#[test]
fn signal_answers_carry_the_hosts_clock_both_ways() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/signals-v1.json")).unwrap();
    for (name, answered_at) in [
        ("changes", Some(1_794_000_000_000)),
        ("olderChanges", None),
        ("inspect", Some(1_794_000_000_000)),
        ("olderInspect", None),
    ] {
        let response: Response = serde_json::from_value(fixture[name].clone()).unwrap();
        let at = match &response {
            Response::Changes { answered_at, .. } | Response::Inspect { answered_at, .. } => {
                *answered_at
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(at, answered_at, "{name}");
        assert_eq!(serde_json::to_value(&response).unwrap(), fixture[name]);
    }
    let response: Response = serde_json::from_value(fixture["readSignalsAnswer"].clone()).unwrap();
    assert!(matches!(
        response,
        Response::ReadSignals {
            answered_at: 1_794_000_000_000,
            ..
        }
    ));

    #[derive(serde::Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    enum OlderResponse {
        Changes { runs: Vec<(RunId, Summary)> },
    }
    let OlderResponse::Changes { runs } =
        serde_json::from_value(fixture["changes"].clone()).unwrap();
    assert_eq!(runs.len(), 1);
}
