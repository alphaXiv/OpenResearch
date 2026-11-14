use orx_signals::Snapshot;
use serde_json::json;
fn fresh() -> Snapshot {
    Snapshot::new("r1", "1-100", "g1")
}
fn record(s: &mut Snapshot, v: serde_json::Value) {
    s.feed(format!("::orx:: {v}\n").as_bytes(), false, 1000, true)
        .unwrap();
}
#[test]
fn long_fractional_history_keeps_resolution() {
    let mut s = fresh();
    for i in 0..20_000 {
        record(
            &mut s,
            json!({"step":i as f64/2000.,"of":10,"m":{"loss":i%31}}),
        );
    }
    let p = &s.series["loss"];
    assert!(p.len() > 512, "retained {} points", p.len());
    assert_eq!(p.first().unwrap().0, 0.);
    assert_eq!(p.last().unwrap().0, 9.9995);
    assert!(p.windows(2).all(|w| w[1].0 - w[0].0 < 0.1));
}
#[test]
fn timing_contains_only_observed_reports_and_total_changes_restart_learning() {
    let mut s = fresh();
    record(&mut s, json!({"step":3000,"of":5000,"t":50,"m":{"loss":3}}));
    record(&mut s, json!({"step":3100,"t":51,"m":{"loss":2}}));
    assert!(serde_json::to_value(&s.phases[0])
        .unwrap()
        .get("crossings")
        .is_none());
    assert_eq!(
        s.timing.progress_points,
        vec![(0, 50., 3000.), (0, 51., 3100.)]
    );
    record(
        &mut s,
        json!({"step":3200,"of":10000,"t":52,"m":{"loss":1}}),
    );
    assert_eq!(s.timing.progress_points, vec![(0, 52., 3200.)]);
    assert_eq!(
        s.series["loss"],
        vec![(3000., Some(3.)), (3100., Some(2.)), (3200., Some(1.))]
    );
}
/// One detected bar is the run's progress; once it restarts, each bar keeps its own step of
/// total and the whole run is unknown for the rest of the run, across a restart of the fold.
#[test]
fn a_single_bar_is_the_run_until_a_second_one_opens() {
    let bars = [
        ("tqdm", "N/4 [00:0N<00:00, 1it/s]\r"),
        ("epoch", "Epoch N/4\n"),
        ("counter", "N/4\n"),
    ];
    for (kind, template) in bars {
        let mut s = fresh();
        let epoch = |s: &mut Snapshot| {
            for n in 1..=4 {
                let line = template.replace('N', &n.to_string());
                s.feed(line.as_bytes(), false, 1000, true).unwrap();
            }
        };
        epoch(&mut s);
        let first = s.progress().unwrap();
        assert_eq!(first.of, Some(4.), "{kind}");
        assert!(first.done.unwrap() > 0., "{kind}");
        assert!(!s.several_bars(), "{kind}");

        let mut s: Snapshot = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        epoch(&mut s);
        assert!(s.several_bars(), "{kind}");
        assert_eq!(s.progress().unwrap().of, Some(4.), "{kind}");
        epoch(&mut s);
        assert_eq!(s.phases.len(), 3, "{kind}");
        assert!(s.several_bars(), "{kind}");
    }
}
/// Only the first phase starts at the payload's start, a declared run never has several bars,
/// and a clock that steps back never moves an observed report earlier.
#[test]
fn time_enters_the_fold_only_through_observation() {
    let mut s = fresh();
    s.timing.started_ms = Some(500);
    let at = |s: &mut Snapshot, line: serde_json::Value, now: i64, observed: bool| {
        s.feed(format!("::orx:: {line}\n").as_bytes(), false, now, observed)
            .unwrap();
    };
    at(
        &mut s,
        json!({"phase":"train","step":0,"of":4,"t":7}),
        2000,
        false,
    );
    at(&mut s, json!({"step":1,"t":8}), 3000, false);
    assert_eq!(s.phases[0].started_at, Some(500));
    assert_eq!(s.timing.anchor, None, "nothing is placed before the end");
    at(
        &mut s,
        json!({"phase":"eval","step":0,"of":2}),
        12_000,
        true,
    );
    assert_eq!(s.phases[1].started_at, Some(12_000));
    assert_eq!(s.progress().unwrap().of, Some(2.));
    assert!(!s.several_bars(), "declared phases are not bars");

    let mut s = fresh();
    at(&mut s, json!({"step":0,"of":4}), 10_000, true);
    at(&mut s, json!({"step":1}), 12_000, true);
    at(&mut s, json!({"step":2}), 11_000, true);
    assert_eq!(
        s.timing.progress_points,
        vec![(0, 10., 0.), (0, 12., 1.), (0, 12., 2.)]
    );
}

/// The last report that advanced sits at the end of the log a scan read to: placed at the
/// earliest such end, moved only by a later advance, never by a repeat.
#[test]
fn the_last_advance_sits_at_the_end_of_the_log() {
    let mut s = fresh();
    let at = |s: &mut Snapshot, line: serde_json::Value, now: i64, observed: bool| {
        s.feed(format!("::orx:: {line}\n").as_bytes(), false, now, observed)
            .unwrap();
    };
    at(&mut s, json!({"step":0,"of":4,"t":7}), 2000, false);
    at(&mut s, json!({"step":1,"t":8}), 2000, false);
    s.reached_end(1500);
    assert_eq!(
        s.timing.anchor,
        Some((0, 8., 1500)),
        "a replay is placed too"
    );
    let rev = s.rev;
    s.reached_end(9000);
    assert_eq!(
        s.timing.anchor,
        Some((0, 8., 1500)),
        "no later than first seen"
    );
    assert_eq!(s.rev, rev, "an unchanged placement is no revision");
    at(&mut s, json!({"step":2,"t":9}), 4000, true);
    at(&mut s, json!({"step":2,"t":10,"m":{"loss":1}}), 9000, true);
    s.reached_end(9000);
    assert_eq!(
        s.timing.anchor,
        Some((0, 9., 9000)),
        "a repeat never anchors"
    );
    at(&mut s, json!({"step":3,"t":11}), 12_000, true);
    s.reached_end(11_000);
    assert_eq!(
        s.timing.anchor,
        Some((0, 11., 11_000)),
        "the last, not the first"
    );

    let mut s = fresh();
    at(&mut s, json!({"step":0,"of":10}), 2000, false);
    at(&mut s, json!({"step":3}), 2000, false);
    assert!(s.timing.progress_points.is_empty());
    s.reached_end(1500);
    assert_eq!(
        s.timing.progress_points,
        vec![(0, 1.5, 3.)],
        "a replay without times gets its last advance"
    );
    at(&mut s, json!({"step":4}), 60_000, true);
    at(&mut s, json!({"step":5}), 60_000, true);
    s.reached_end(20_000);
    assert_eq!(
        s.timing.progress_points,
        vec![(0, 1.5, 3.), (0, 20., 4.), (0, 20., 5.)],
        "a catch-up read moves back to when the log last changed"
    );
    assert_eq!(s.progress().unwrap().last_point, Some((20., 5.)));
}
#[test]
fn coverage_retains_truncation_after_catching_up_and_preserves_gaps() {
    let mut s = fresh();
    for i in 0..65 {
        record(&mut s, json!({"m":{format!("m{i}"):1}}));
    }
    let v = serde_json::to_value(&s.coverage).unwrap();
    assert_eq!(v["reasons"], json!(["truncated"]));
    let end = s.cursor.offset;
    s.cursor.offset += 100;
    s.cursor.frame_start = s.cursor.offset;
    s.feed(b"ordinary\n", true, 1000, true).unwrap();
    assert!(!s.coverage.complete);
    assert_eq!(s.coverage.ranges, vec![(0, end), (end + 100, end + 109)]);
}
#[test]
fn wire_boundaries_and_marker_diagnostics_are_contracts() {
    let mut s = fresh();
    s.feed(
        b"\xef\xbb\xbf::orx:: {\"m\":{\"bom\":1}}\n::orx:: {NaN:1}\n",
        false,
        0,
        false,
    )
    .unwrap();
    assert_eq!(s.latest["bom"], 1);
    assert_eq!(s.diagnostics.counts["invalid_record"], 1);
    record(
        &mut s,
        json!({"phase":"p".repeat(129),"unit":"u".repeat(65),"m":{"long":"x".repeat(1025),"ok":1}}),
    );
    assert!(!s.latest.contains_key("long"));
    assert!(s.phases.is_empty());
    s.feed(&vec![b'x'; 65537], false, 0, false).unwrap();
    assert!(s.cursor.dropping);
    let mut s = fresh();
    s.feed(
        b"::orx::{\"step\":1}\n::orx::\t{}\nprefix ::orx:: {}\nordinary \xff\n",
        false,
        0,
        false,
    )
    .unwrap();
    assert_eq!(s.diagnostics.counts.values().sum::<u64>(), 3);
}
#[test]
fn progress_coalesces_unknown_keys_and_equal_numbers_but_rows_are_events() {
    let mut s = fresh();
    record(&mut s, json!({"step":1,"t":1,"future":1,"m":{"a":1}}));
    record(&mut s, json!({"step":1,"t":2,"future":2,"m":{"a":1.0}}));
    assert_eq!(s.series["a"].len(), 1);
    assert!(!s.diagnostics.counts.contains_key("conflicting_report"));
    record(&mut s, json!({"m":{"a":2}}));
    record(&mut s, json!({"m":{"a":3}}));
    assert!(!s.diagnostics.counts.contains_key("conflicting_report"));
    for i in 0..64 {
        record(&mut s, json!({"m":{format!("m{i}"):1}}));
    }
    for _ in 0..2 {
        record(&mut s, json!({"row":"eval","m":{"task":"a","score":0.5}}));
    }
    assert_eq!(s.rows["eval"].len(), 2);
    assert_eq!(s.rows["eval"][0]["task"], "a");
}
#[test]
fn declared_gate_and_plain_counter_boundaries() {
    let mut s = fresh();
    record(&mut s, json!({"step":5,"of":10}));
    s.feed(b"1/10\n2/10\n", false, 0, false).unwrap();
    assert_eq!(s.progress().unwrap().done, Some(5.));
    let mut s = fresh();
    s.feed(b"x | 7/10\nx | 8/10\n", false, 0, false).unwrap();
    assert!(s.progress().is_none());
}
#[test]
fn terminal_is_an_immutable_byte_boundary() {
    let mut s = fresh();
    s.feed(b"::orx:: {", true, 0, false).unwrap();
    let before = s.clone();
    assert!(s.feed(b"\"step\":1}\n", false, 1, false).is_err());
    assert_eq!(s, before);
}

#[test]
fn populated_disk_contract_is_written_and_read() {
    let bytes = include_bytes!("fixtures/checkpoint.txt");
    let mut snapshot = fresh();
    snapshot.timing.started_ms = Some(500);
    snapshot.feed(bytes, false, 1000, false).unwrap();
    snapshot.feed(&[], true, 1000, false).unwrap();
    snapshot.reached_end(1000);
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/contracts/run-signals.json")).unwrap();
    fixture.as_object_mut().unwrap().remove("anchorBase64");
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), fixture);
    assert_eq!(
        serde_json::from_value::<Snapshot>(fixture.clone()).unwrap(),
        snapshot
    );
    let mut older = fixture;
    older["timing"]
        .as_object_mut()
        .unwrap()
        .remove("lastAdvance");
    older.as_object_mut().unwrap().remove("seriesStages");
    let older: Snapshot = serde_json::from_value(older).unwrap();
    assert!(
        older.series_stages.is_empty(),
        "a checkpoint before it reads, its stages unknown"
    );
    assert_eq!(
        older.timing.last_advance, None,
        "a checkpoint before it reads"
    );
    assert_eq!(older.timing.anchor, snapshot.timing.anchor);
    assert_eq!(older.last_line, "", "a checkpoint before it reads");

    let mut newer = serde_json::to_value(&snapshot).unwrap();
    newer["lastLine"] = json!("Downloading shard_00001.parquet...");
    let newer: Snapshot = serde_json::from_value(newer).unwrap();
    assert_eq!(newer.last_line, "Downloading shard_00001.parquet...");
    assert_eq!(
        serde_json::to_value(&newer).unwrap()["lastLine"],
        "Downloading shard_00001.parquet..."
    );
}

#[test]
fn current_bounds_fit_without_a_snapshot_reduction_pass() {
    let mut snapshot = fresh();
    for metric in 0..64 {
        let name = format!("m{metric:063}");
        snapshot
            .latest
            .insert(name.clone(), json!("\0".repeat(1024)));
        snapshot.series.insert(
            name,
            vec![(1.2345678901234567e-200, Some(1.2345678901234567e200)); 2048],
        );
    }
    for table in 0..16 {
        let name = format!("t{table:063}");
        let cells = (0..64)
            .map(|i| (format!("c{i:063}"), json!("\0".repeat(1024))))
            .collect();
        snapshot.rows.insert(name, vec![cells]);
    }
    snapshot.cursor.pending_base64 = "x".repeat(87384);
    snapshot.last_canonical = Some(json!("x".repeat(65536)));
    let measured = serde_json::to_vec(&snapshot).unwrap().len();
    let rows = serde_json::to_vec(&snapshot.rows).unwrap().len();
    let upper = measured - rows + 2 * 1024 * 1024 + 2 * 1024 * 1024;
    assert!(upper < orx_signals::MAX_SNAPSHOT);
}

#[test]
fn ignored_top_level_fields_do_not_defeat_coordinate_coalescing() {
    let mut s = fresh();
    record(
        &mut s,
        json!({"step":1,"of":10,"t":1,"future":1,"m":{"a":1}}),
    );
    record(
        &mut s,
        json!({"step":1,"of":10,"t":2,"future":2,"m":{"a":1}}),
    );
    assert_eq!(s.timing.progress_points.len(), 1);
}

#[test]
fn random_chunking_and_checkpoint_resume_preserve_the_whole_fold() {
    let mut seed = 1234567u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        seed
    };
    let cases = [
        b"ordinary \xff\n".as_slice(),
        b"::orx:: {\"step\":1,\"t\":1,\"m\":{\"loss\":NaN}}\r\n",
        b"::orx:: {\"step\":2,\r\"m\":{\"loss\":1,\"note\":\"NaN literal\"}}\n",
        b"::orx::{bad}\n",
        b"1/10\r2/10\r10/10\r0/10\r",
        b"::orx:: {\"row\":\"eval\",\"m\":{\"x\":2}}\n",
        b"::orx:: {\"m\":{\"a\":1,\"a\":2}}\n",
        b"::orx:: {\"step\":3,\"t\":0.5,\"m\":{\"loss\":3}}\n",
    ];
    for _ in 0..300 {
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        for _ in 0..20 {
            bytes.extend_from_slice(cases[next() as usize % cases.len()]);
        }
        for observed in [false, true] {
            let mut whole = fresh();
            whole.feed(&bytes, false, 1000, observed).unwrap();
            whole.feed(&[], true, 1000, observed).unwrap();
            let mut split = fresh();
            let mut start = 0;
            while start < bytes.len() {
                let end = (start + 1 + next() as usize % 137).min(bytes.len());
                split
                    .feed(&bytes[start..end], false, 1000, observed)
                    .unwrap();
                split = serde_json::from_slice(&serde_json::to_vec(&split).unwrap()).unwrap();
                start = end;
            }
            split.feed(&[], true, 1000, observed).unwrap();
            split.rev = whole.rev;
            assert_eq!(split, whole);
        }
    }
}

#[test]
fn timing_keeps_slice_brackets_first_and_cadence_tail() {
    let mut s = fresh();
    for step in 0..=5000 {
        record(&mut s, json!({"step":step,"of":5000,"t":step}));
    }
    let points: Vec<_> = s
        .timing
        .progress_points
        .iter()
        .map(|p| p.2 as usize)
        .collect();
    let mut expected = std::collections::BTreeSet::from([0]);
    for boundary in (125..=5000).step_by(125) {
        expected.extend([boundary - 1, boundary]);
    }
    expected.extend(4988..=5000);
    assert!(expected.iter().all(|step| points.contains(step)));
    assert!(points.len() <= orx_signals::MAX_TIMING_REPORTS);
    let mut unknown = fresh();
    for step in 0..=5000 {
        record(&mut unknown, json!({"step":step,"t":step}));
    }
    let points = &unknown.timing.progress_points;
    assert_eq!(points[0].2, 0.);
    assert!(points.len() <= orx_signals::MAX_TIMING_REPORTS);
    assert!((4988..=5000).all(|step| points.iter().any(|p| p.2 == step as f64)));
}

#[test]
fn frame_quotes_empty_bodies_and_escape_prefixes_agree() {
    for prefix in [b"".as_slice(), b"\x1b7", b"\x1b[0m\xef\xbb\xbf"] {
        let mut s = fresh();
        let mut input = prefix.to_vec();
        input.extend_from_slice(
            b"::orx:: {\"m\":{\"s\":\"}}\"},\r\"step\":1}\n::orx:: \r{\"step\":2}\n",
        );
        s.feed(&input, true, 0, false).unwrap();
        assert_eq!(s.progress().unwrap().done, Some(2.));
        assert_eq!(s.latest["s"], "}}");
    }
}

#[test]
fn detector_selection_releases_only_after_completion_for_all_bars() {
    let mut s = fresh();
    s.feed(
        b"1/2 [00:01<00:01, 1it/s]\r2/2 [00:02<00:00, 1it/s]\r",
        false,
        0,
        false,
    )
    .unwrap();
    for _ in 0..3 {
        s.feed(
            b"1/10 [00:01<00:09, 1it/s]\r2/10 [00:02<00:08, 1it/s]\r10/10 [00:10<00:00, 1it/s]\r",
            false,
            0,
            false,
        )
        .unwrap();
    }
    assert_eq!(s.phases.len(), 4);
    assert_eq!(s.progress().unwrap().done, Some(10.));
    let mut s = fresh();
    for _ in 0..3 {
        s.feed(b"1/3\n2/3\n3/3\n", false, 0, false).unwrap();
    }
    assert_eq!(s.phases.len(), 3);
    let mut s = fresh();
    s.feed(b"1/10 [00:01<00:09, 1it/s]\r5/10 [00:05<00:05, 1it/s]\r1/10 [00:01<00:09, 1it/s]\r2/10 [00:02<00:08, 1it/s]\r",false,0,false).unwrap();
    assert_eq!(s.phases.len(), 1);
    assert_eq!(s.progress().unwrap().done, Some(5.));
}

#[test]
fn every_detector_kind_finishes_at_its_own_total() {
    let mut s = fresh();
    s.feed(b"Epoch 1/3\nEpoch 2/3\nEpoch 3/3\n", false, 0, false)
        .unwrap();
    assert_eq!(s.progress().unwrap().done, Some(2.));
    s.feed(b"Epoch 1/5\nEpoch 2/5\n", false, 0, false).unwrap();
    assert_eq!(s.phases.len(), 2);
    assert_eq!(s.progress().unwrap().done, Some(1.));
    s.feed(b"Epoch 1/5\nEpoch 2/5\n", false, 0, false).unwrap();
    assert_eq!(
        s.phases.len(),
        2,
        "an unfinished epoch counter keeps its occurrence"
    );

    let mut s = fresh();
    s.feed(b"1/3\n2/3\n3/3\n1/4\n2/4\n", false, 0, false)
        .unwrap();
    assert_eq!(s.phases.len(), 2);

    let mut s = fresh();
    s.feed(
        b"1/3 [00:01<00:02, 1it/s]\r3/3 [00:03<00:00, 1it/s]\r1/4 [00:01<00:03, 1it/s]\r2/4 [00:02<00:02, 1it/s]\r",
        false,
        0,
        false,
    )
    .unwrap();
    assert_eq!(s.phases.len(), 2);

    let mut s = fresh();
    s.feed(
        b"5it [00:01, 5it/s]\r9it [00:02, 5it/s]\r1/4 [00:01<00:03, 1it/s]\r2/4 [00:02<00:02, 1it/s]\r",
        false,
        0,
        false,
    )
    .unwrap();
    assert_eq!(s.phases.len(), 1, "an open bar never finishes");
    assert_eq!(s.progress().unwrap().done, Some(9.));
}

#[test]
fn charset_escapes_and_a_second_bom_are_not_stripped_twice() {
    let mut s = fresh();
    s.feed(b"\x1b(B::orx:: {\"step\":1}\n", true, 0, false)
        .unwrap();
    assert_eq!(s.progress().unwrap().done, Some(1.));
    for input in [
        b"\xef\xbb\xbf\xef\xbb\xbf::orx:: {\"step\":1}\n".as_slice(),
        b"x\n\xef\xbb\xbf::orx:: {\"step\":1}\n",
    ] {
        let mut s = fresh();
        s.feed(input, true, 0, false).unwrap();
        assert!(s.progress().is_none());
        assert_eq!(s.diagnostics.counts["misplaced_marker"], 1);
    }
    let input = b"\xef\xbb\xbf\xef\xbb\xbf::orx:: {\"m\":{\"s\":\"a\r\"}}\n";
    let mut s = fresh();
    s.feed(input, true, 0, false).unwrap();
    let cr = input.iter().position(|b| *b == b'\r').unwrap() as u64;
    assert_eq!(
        s.diagnostics.examples[0].to,
        cr + 1,
        "a CR ends an undeclared frame"
    );
}

#[test]
fn independent_row_columns_and_sorted_coverage_are_bounded() {
    let mut s = fresh();
    for col in 0..100 {
        record(&mut s, json!({"row":"eval","m":{format!("c{col}"):col}}));
    }
    assert_eq!(s.row_columns["eval"].len(), 64);
    assert_eq!(s.rows["eval"].iter().map(|r| r.len()).sum::<usize>(), 64);
    s.coverage.add_reason("z");
    s.coverage.add_reason("a");
    assert!(s.coverage.reasons.windows(2).all(|w| w[0] < w[1]));
    let mut s = fresh();
    s.feed(b"::orx:: {\"m\":{NaN :1}}\n", true, 0, false)
        .unwrap();
    assert!(s.latest.is_empty());
    assert_eq!(s.diagnostics.counts["invalid_record"], 1);
}

#[test]
fn old_occurrences_are_retained_without_claiming_lost_coverage() {
    let mut s = fresh();
    for i in 0..100 {
        record(&mut s, json!({"phase":format!("p{i}"),"step":0}));
    }
    s.feed(&[], true, 0, false).unwrap();
    assert_eq!(s.phases.len(), 64);
    assert_eq!(s.omitted["phases"], 36);
    assert!(s.coverage.complete);
}

#[test]
fn timing_bound_is_per_occurrence_and_shared_grid_handles_fractional_totals() {
    assert_eq!(orx_signals::slice_count(0.1), 1);
    assert_eq!(orx_signals::slice_count(12.4), 13);
    assert_eq!(orx_signals::slice_count(12.5), 13);
    assert_eq!(orx_signals::slice_count(5000.), 40);
    assert_eq!(orx_signals::slice_boundary(12.4, 13), 12.4);
    let mut s = fresh();
    for phase in 0..64 {
        for step in 0..1000 {
            record(
                &mut s,
                json!({"phase":format!("p{phase}"),"step":step,"of":1000,"t":phase*10000+step}),
            );
        }
    }
    let mut counts = std::collections::BTreeMap::<usize, usize>::new();
    for point in &s.timing.progress_points {
        *counts.entry(point.0).or_default() += 1;
    }
    assert_eq!(counts.len(), 64);
    assert!(counts
        .values()
        .all(|n| *n <= orx_signals::MAX_TIMING_REPORTS));
    assert!(serde_json::to_vec(&s).unwrap().len() < orx_signals::MAX_SNAPSHOT);
}

#[test]
fn removing_a_coverage_reason_updates_terminal_completeness_immediately() {
    let mut s = fresh();
    s.coverage.add_reason("catching_up");
    s.remove_reason("catching_up");
    assert!(!s.coverage.complete);
    s.feed(&[], true, 0, false).unwrap();
    s.coverage.add_reason("catching_up");
    s.remove_reason("catching_up");
    assert!(s.coverage.complete);
}

#[test]
fn retention_does_not_prevent_source_collection_completion() {
    let mut s = fresh();
    for i in 0..65 {
        record(&mut s, json!({"m":{format!("m{i}"):i}}));
    }
    s.feed(&[], true, 0, false).unwrap();
    assert!(!s.coverage.complete);
    assert!(s.source_complete());
    s.coverage.add_reason("missing_middle");
    assert!(!s.source_complete());
}
