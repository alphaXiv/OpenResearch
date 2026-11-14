use orx_signals::{Snapshot, MAX_FRAME, MAX_SNAPSHOT};
use serde_json::json;

fn fresh() -> Snapshot {
    Snapshot::new("r1", "1-100", "g1")
}
fn feed(s: &mut Snapshot, text: &str) {
    s.feed(text.as_bytes(), false, 1000, true).unwrap();
}

#[test]
fn every_byte_boundary_survives_checkpoint_restart() {
    let input = "\u{feff}\x1b[32m::orx:: {\"phase\":\"訓練\",\"step\":1,\"of\":3,\"m\":{\"loss\":NaN,\"note\":\"Infinity literal\"}}\x1b[0m\r\n::orx:: {\"step\":2,\"m\":{\"loss\":2}}\n";
    let mut expected = fresh();
    feed(&mut expected, input);
    for split in 0..=input.len() {
        let mut s = fresh();
        s.feed(&input.as_bytes()[..split], false, 1000, true)
            .unwrap();
        s = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        s.feed(&input.as_bytes()[split..], false, 1000, true)
            .unwrap();
        assert_eq!(s.latest, expected.latest, "split {split}");
        assert_eq!(s.series, expected.series, "split {split}");
        assert_eq!(s.phases, expected.phases, "split {split}");
        assert_eq!(s.cursor, expected.cursor, "split {split}");
    }
}

#[test]
fn terminal_eof_is_distinct_from_live_eof_and_truncated_json() {
    let mut s = fresh();
    feed(&mut s, "::orx:: {\"m\":{\"a\":1}}");
    assert!(s.latest.is_empty());
    s.feed(b"", true, 1000, true).unwrap();
    assert_eq!(s.latest["a"], 1);
    let mut s = fresh();
    feed(&mut s, "::orx:: {\"m\":{");
    s.feed(b"", true, 1000, true).unwrap();
    assert_eq!(s.diagnostics.counts["incomplete_record"], 1);
}

#[test]
fn tolerance_table_rejects_whole_invalid_record_and_only_bad_siblings() {
    let mut s = fresh();
    feed(
        &mut s,
        "prose ::orx:: {\"m\":{\"embedded\":1}}\n2026-11-11 ::orx:: {\"m\":{\"prefix\":1}}\n",
    );
    feed(
        &mut s,
        "::orx:: {\"m\":{\"duplicate\":1,\"duplicate\":2}}\n::orx:: {broken}\n",
    );
    feed(&mut s, "::orx:: {\"step\":Infinity,\"spend\":NaN,\"future\":{\"x\":1},\"m\":{\"a\":Infinity,\"b\":-Infinity,\"c\":\"NaN\",\"step\":9,\"bad\":true,\"bad2\":[],\"good\":4}}\n");
    assert_eq!(
        s.latest,
        serde_json::from_value(json!({"a":"Infinity","b":"-Infinity","c":"NaN","step":9,"good":4}))
            .unwrap()
    );
    assert!(s.progress().is_none());
    assert_eq!(s.spend, None);
    assert_eq!(s.diagnostics.counts["invalid_record"], 2);
    assert_eq!(s.diagnostics.counts["non_finite"], 3);
}

#[test]
fn cr_redraws_osc_and_partial_declared_records() {
    let mut s = fresh();
    feed(&mut s, "\x1b]0;title\x07::orx:: {\"step\":1,\r\"of\":3}\n::orx:: {\"step\":2}\r::orx:: {\"step\":3}\r\n");
    assert_eq!(s.progress().unwrap().done, Some(3.0));
    assert_eq!(s.progress().unwrap().of, Some(3.0));
    assert!(s.diagnostics.counts.is_empty());
    let mut s = fresh();
    feed(&mut s, "0/10\r1/10\r2/10\r");
    assert_eq!(s.progress().unwrap().done, Some(2.0));
}

#[test]
fn phase_occurrences_regressions_null_and_string_transitions() {
    let mut s = fresh();
    feed(&mut s, "::orx:: {\"phase\":\"train\",\"step\":2,\"of\":10,\"t\":2,\"spend\":3,\"m\":{\"loss\":2}}\n::orx:: {\"step\":1,\"t\":3,\"spend\":2,\"m\":{\"loss\":null}}\n");
    assert_eq!(s.progress().unwrap().done, Some(2.0));
    assert_eq!(s.spend, Some(3.0));
    assert_eq!(s.latest["loss"], 2);
    feed(&mut s, "::orx:: {\"step\":3,\"m\":{\"loss\":\"waiting\"}}\n::orx:: {\"step\":4,\"m\":{\"loss\":1}}\n::orx:: {\"of\":null}\n");
    assert_eq!(
        s.series["loss"],
        vec![(2.0, Some(2.0)), (3.0, None), (4.0, Some(1.0))]
    );
    assert_eq!(s.progress().unwrap().of, None);
    feed(
        &mut s,
        "::orx:: {\"phase\":\"eval\",\"step\":0}\n::orx:: {\"phase\":\"train\",\"step\":0}\n",
    );
    assert_eq!(s.phases.len(), 3);
    assert_eq!(s.progress().unwrap().occurrence, 2);
}

#[test]
fn ranks_merge_without_summing_and_coalesce_rows_ignoring_time() {
    let mut s = fresh();
    feed(&mut s, "::orx:: {\"step\":1,\"row\":\"eval\",\"t\":1,\"m\":{\"a\":1}}\n::orx:: {\"step\":1,\"row\":\"eval\",\"t\":2,\"m\":{\"a\":1}}\n::orx:: {\"step\":1,\"m\":{\"b\":2}}\n::orx:: {\"step\":1,\"m\":{\"a\":3}}\n");
    assert_eq!(s.rows["eval"].len(), 2);
    assert_eq!(s.latest["a"], 3);
    assert_eq!(s.latest["b"], 2);
    assert_eq!(s.diagnostics.counts["conflicting_report"], 1);
}

#[test]
fn oversized_frames_are_bounded_and_recover_at_terminator() {
    let mut s = fresh();
    s.feed(&vec![b'x'; MAX_FRAME * 10], false, 1000, true)
        .unwrap();
    assert!(s.cursor.dropping);
    assert!(s.cursor.pending_base64.len() <= MAX_FRAME * 2);
    feed(&mut s, "\n::orx:: {\"m\":{\"ok\":1}}\n");
    assert_eq!(s.latest["ok"], 1);
    assert_eq!(s.diagnostics.counts["oversized_record"], 1);
}

#[test]
fn detectors_require_compatible_observations_and_never_override_declared() {
    for (input, done, of) in [
        ("Epoch 1/10\nEpoch 2/10\n", 1., Some(10.)),
        (
            " 10%|# | 1/10 [00:02<00:18, 0.5it/s]\r 20%|## | 2/10 [00:04<00:16, 0.5it/s]\r",
            2.,
            Some(10.),
        ),
        ("10it [00:02, 5it/s]\r20it [00:04, 5it/s]\r", 20., None),
    ] {
        let mut s = fresh();
        feed(&mut s, input);
        assert_eq!(s.progress().unwrap().done, Some(done));
        assert_eq!(s.progress().unwrap().of, of);
        feed(&mut s, "::orx:: {\"step\":7,\"of\":20}\n30/100\n40/100\n");
        assert_eq!(s.progress().unwrap().done, Some(7.));
        assert_eq!(s.progress().unwrap().source, "declared");
    }
    let mut s = fresh();
    feed(
        &mut s,
        "2026/11/11\ndownloaded file 1/10\n/path/1/10\n1/10\n2/20\n1/20\n",
    );
    assert!(s.progress().is_none());
}

#[test]
fn a_counter_opening_a_line_is_progress_alone_bracketed_or_after_a_counted_noun() {
    for (input, done, unit) in [
        ("3/10\n4/10\n", 4., Some("items")),
        ("[3/10] Compiling x\n[4/10]: Compiling y\n", 4., Some("items")),
        ("chunk 3/10 done, 372s\nchunk 4/10 done, 496s\n", 4., Some("chunks")),
        ("Batch: 3/10\nBATCHES 4/10: loss=0.5\n", 4., Some("batches")),
        ("step 3/10 | loss 0.5\nStep 4/10 | loss 0.4\n", 4., None),
        ("Iteration 3/10\niters 4/10\n", 4., None),
        (
            "chunk: 30%|###   | 3/10 [00:03<00:07, 1.0it/s]\r40%|####  | 4/10 [00:04<00:06, 1.0it/s]\r",
            4.,
            None,
        ),
    ] {
        let mut s = fresh();
        feed(&mut s, input);
        let progress = s.progress().expect(input);
        assert_eq!(progress.done, Some(done), "{input}");
        assert_eq!(progress.of, Some(10.), "{input}");
        assert_eq!(progress.unit.as_deref(), unit, "{input}");
    }
}

fn assert_no_progress(input: &str) {
    let mut s = fresh();
    feed(&mut s, input);
    assert!(s.progress().is_none(), "{input}: {:?}", s.progress());
}

#[test]
fn a_counter_after_a_word_runs_do_not_count_is_not_progress() {
    assert_no_progress("retry 2/3 after timeout\nretry 3/3 after timeout\n");
    assert_no_progress("attempt 1/5\nattempt 2/5\n");
    assert_no_progress("loss 0.5/1.0\nloss 0.6/1.0\n");
}

#[test]
fn a_counter_inside_a_sentence_is_not_progress() {
    assert_no_progress("done with 3/10 chunks\ndone with 4/10 chunks\n");
}

#[test]
fn a_date_is_not_progress() {
    assert_no_progress("11/12/2026 started\n11/13/2026 started\n");
}

#[test]
fn a_path_is_not_progress() {
    assert_no_progress("data/3/10/x.png\ndata/4/10/x.png\nfile data/3/10\nfile data/4/10\n");
}

#[test]
fn a_ratio_with_a_decimal_is_not_progress() {
    assert_no_progress("step 0.5/1\nstep 0.6/1\n[0.5/1] x\n[0.6/1] x\nchunk 1/2.5\nchunk 2/2.5\n");
}

#[test]
fn a_counter_whose_total_changes_is_not_progress() {
    assert_no_progress("chunk 1/10 done\nchunk 2/12 done\nchunk 3/14 done\n");
}

#[test]
fn an_uninstrumented_benchmark_log_folds_to_its_chunk_counter_only() {
    let input = include_bytes!("fixtures/codec.out");
    let mut s = fresh();
    for chunk in input.chunks(7) {
        s.feed(chunk, false, 1000, false).unwrap();
        s = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
    }
    s.feed(b"", true, 1000, false).unwrap();

    assert_eq!(s.phases.len(), 1);
    let progress = s.progress().unwrap();
    assert_eq!(progress.source, "detected");
    assert_eq!((progress.done, progress.of), (Some(10.), Some(10.)));
    assert_eq!(progress.unit.as_deref(), Some("chunks"));
    assert!(s.latest.is_empty() && s.series.is_empty(), "{:?}", s.latest);
    assert!(s.rows.is_empty());
    assert!(s.diagnostics.counts.is_empty(), "{:?}", s.diagnostics);
    assert!(s.coverage.complete);
}

fn one_progress(input: &str) -> (f64, f64, Option<String>) {
    let mut s = fresh();
    feed(&mut s, input);
    let progress = s.progress().expect(input);
    (
        progress.done.unwrap(),
        progress.of.unwrap(),
        progress.unit.clone(),
    )
}

#[test]
fn a_counter_after_a_logging_prefix_is_progress() {
    let unit = |unit: &str| Some(unit.to_owned());
    for (input, expected) in [
        (
            "2026-11-13 13:20:13,057 - rustbpe - INFO - Progress: 56% (18202/32503 merges)\n2026-11-13 13:20:14,057 - rustbpe - INFO - Progress: 57% (18527/32503 merges)\n",
            (18527., 32503., unit("merges")),
        ),
        (
            "I1113 13:20:42.713 train.py:42] step 300/1000 loss=0.5\nI1113 13:20:43.713 train.py:42] step 301/1000 loss=0.5\n",
            (301., 1000., None),
        ),
        (
            "[2026-11-13T13:20:13Z INFO trainer] chunk 3/10 done\n[2026-11-13T13:20:14Z INFO trainer] chunk 4/10 done\n",
            (4., 10., unit("chunks")),
        ),
        (
            "2026-11-13T13:20:13.123456Z  INFO train::loop: shards: 3/9\n2026-11-13T13:20:14.123456Z  INFO train::loop: shards: 4/9\n",
            (4., 9., unit("shards")),
        ),
        ("INFO:root:Epoch 1/10\nINFO:root:Epoch 2/10\n", (1., 10., unit("epochs"))),
        ("Tokenizing: 5% (5/100 files)\n6% (6/100 files)\n", (6., 100., unit("files"))),
        ("Progress: 50% (5/10)\n60% (6/10)\n", (6., 10., None)),
    ] {
        assert_eq!(one_progress(input), expected, "{input}");
    }
}

#[test]
fn a_percentage_without_its_count_or_against_the_wrong_count_is_not_progress() {
    assert_no_progress("Progress: 56%\nProgress: 57%\n");
    assert_no_progress("Progress: 10% (50/100 merges)\nProgress: 11% (51/100 merges)\n");
    assert_no_progress(
        "it is 56% (18202/32503 merges) done\nit is 57% (18527/32503 merges) done\n",
    );
    assert_no_progress(
        "2026-11-13 13:20:13 - x - INFO - retry 2/3\n2026-11-13 13:20:14 - x - INFO - retry 3/3\n",
    );
    assert_no_progress("2026-11-13 13:20:13 something INFO or other: 3/10\n");
}

#[test]
fn a_run_nobody_instrumented_folds_to_its_tokenizer_then_its_training_steps() {
    let input = include_bytes!("fixtures/nanochat-setup.out");
    let mut s = fresh();
    s.feed(&input[..input.len() / 2], false, 1000, true)
        .unwrap();
    assert!(!s.has_signals() || s.progress().is_some());
    for chunk in input[input.len() / 2..].chunks(4096) {
        s.feed(chunk, false, 1000, true).unwrap();
    }

    assert_eq!(s.phases.len(), 2, "{:?}", s.phases);
    let [tokenizer, training] = &s.phases[..] else {
        unreachable!()
    };
    assert_eq!(
        (tokenizer.source.as_str(), tokenizer.name.as_str()),
        ("detected", "")
    );
    assert_eq!((tokenizer.done, tokenizer.of), (Some(32503.), Some(32503.)));
    assert_eq!(tokenizer.unit.as_deref(), Some("merges"));
    assert_eq!((training.done, training.of), (Some(20.), Some(1617.)));
    assert_eq!(training.unit, None);
    assert!(s.several_bars());
    assert!(s.diagnostics.counts.is_empty(), "{:?}", s.diagnostics);
}

#[test]
fn the_tokenizer_bar_is_found_before_training_prints_anything() {
    let input = include_bytes!("fixtures/nanochat-setup.out");
    let end = input
        .windows(14)
        .position(|w| w == b"Training time:")
        .unwrap();
    let mut s = fresh();
    s.feed(&input[..end], false, 1000, true).unwrap();

    let progress = s.progress().unwrap();
    assert_eq!((progress.done, progress.of), (Some(32503.), Some(32503.)));
    assert_eq!(s.phases.len(), 1);
}

#[test]
fn setup_output_before_any_progress_is_not_progress() {
    let input = include_bytes!("fixtures/nanochat-setup.out");
    let end = input
        .windows(14)
        .position(|w| w == b"max_chars: 2,0")
        .unwrap();
    let mut s = fresh();
    s.feed(&input[..end], false, 1000, true).unwrap();

    assert!(!s.has_signals(), "{:?}", s.phases);
    assert!(s.diagnostics.counts.is_empty());
}

#[test]
fn the_latest_line_is_the_newest_that_says_something_and_never_a_payload_line() {
    let mut s = fresh();
    assert_eq!(s.last_line, "");
    feed(&mut s, "Downloading shard_00001.parquet...\n");
    feed(&mut s, "\x1b[32m\x1b[1mINFO\x1b[0m   Computing initial pair counts \t from 1952346 unique sequences  \n");
    feed(
        &mut s,
        "   \n\u{2588}\u{2588}\u{2591}\u{2591}\n::orx:: {\"m\":{\"loss\":1}}\n",
    );
    assert_eq!(
        s.last_line,
        "INFO   Computing initial pair counts   from 1952346 unique sequences"
    );

    feed(
        &mut s,
        "2026-11-13 13:20:09,328 - rustbpe - \x1b[32m\x1b[1mINFO\x1b[0m - Starting merge loop\n",
    );
    assert_eq!(
        s.last_line, "Starting merge loop",
        "past the logging prefix"
    );

    feed(&mut s, &format!("{}\n", "x".repeat(500)));
    assert_eq!(s.last_line.len(), 200);

    feed(&mut s, "::orx:: {\"phase\":\"train\"}\n");
    assert_eq!(s.last_line, "", "a new stage has said nothing yet");
}

#[test]
fn deterministic_bounded_series_retain_extrema_and_latest() {
    let mut a = fresh();
    let mut b = fresh();
    for i in 0..20_000 {
        let line = format!(
            "::orx:: {{\"step\":{i},\"m\":{{\"loss\":{}}}}}\n",
            if i == 10_001 {
                -100_000.0
            } else {
                (i as f64).sin()
            }
        );
        feed(&mut a, &line);
        b.feed(line.as_bytes(), false, 1000, false).unwrap();
        if i % 137 == 0 {
            b = serde_json::from_slice(&serde_json::to_vec(&b).unwrap()).unwrap();
        }
    }
    assert_eq!(a.series, b.series);
    assert!(a.series["loss"].len() <= 2048);
    assert!(a.series["loss"].iter().any(|p| p.1 == Some(-100_000.0)));
    assert_eq!(a.series["loss"].last().unwrap().0, 19_999.);
    assert!(serde_json::to_vec(&a).unwrap().len() < MAX_SNAPSHOT);
}

#[test]
fn metric_discovery_and_diagnostics_are_bounded() {
    let mut s = fresh();
    for i in 0..1000 {
        feed(&mut s, &format!("::orx:: {{\"m\":{{\"metric{i}\":1}}}}\n"));
    }
    assert_eq!(s.latest.len(), 64);
    assert_eq!(s.diagnostics.examples.len(), 16);
    assert_eq!(s.coverage.reasons, vec!["truncated"]);
}

#[test]
fn prototype_scenarios_replay_through_public_interface() {
    for (name, input) in [
        (
            "fine-tune",
            include_bytes!("fixtures/fine-tune.out").as_slice(),
        ),
        (
            "self-play",
            include_bytes!("fixtures/self-play.out").as_slice(),
        ),
        ("eval", include_bytes!("fixtures/eval.out").as_slice()),
        (
            "pipeline",
            include_bytes!("fixtures/pipeline.out").as_slice(),
        ),
        (
            "open-ended",
            include_bytes!("fixtures/open-ended.out").as_slice(),
        ),
    ] {
        let mut s = fresh();
        for chunk in input.chunks(317) {
            s.feed(chunk, false, 1000, false).unwrap();
            s = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        }
        s.feed(b"", true, 1000, false).unwrap();
        assert!(s.progress().is_some(), "{name}");
        assert!(s.coverage.complete, "{name}: {:?}", s.diagnostics);
        assert_eq!(
            s.progress().unwrap().source,
            if name == "open-ended" {
                "detected"
            } else {
                "declared"
            }
        );
        let (done, time) = match name {
            "fine-tune" => (5000., 7539.876457963002),
            "self-play" => (24., 22043.553251138692),
            "eval" => (1200., 2630.837227541287),
            "pipeline" => (1., 4118.183819335778),
            "open-ended" => (146000., 28623.),
            _ => unreachable!(),
        };
        assert_eq!(s.progress().unwrap().done, Some(done), "{name}");
        assert!(
            (s.progress().unwrap().last_point.unwrap().0 - time).abs() < 1e-8,
            "{name}"
        );
        if name == "fine-tune" {
            assert_eq!(s.latest["loss"], json!(1.580241758890959));
        }
        if name == "eval" {
            assert_eq!(s.spend, Some(23.336800041690417));
            assert_eq!(s.latest["c"], 3);
        }
        if name == "pipeline" {
            assert_eq!(s.latest["loss"], json!(0.40013604348559734));
        }
        if name == "pipeline" {
            assert_eq!(s.phases.len(), 5);
        }
    }
}

#[test]
fn shipped_checkpoint_and_payload_contracts_round_trip_with_future_fields() {
    let mut raw: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/contracts/run-signals.json")).unwrap();
    raw["future"] = json!({"x":1});
    raw["cursor"]["future"] = json!(true);
    raw["timing"]["future"] = json!("kept");
    let checkpoint: Snapshot = serde_json::from_value(raw).unwrap();
    let saved = serde_json::to_value(&checkpoint).unwrap();
    assert_eq!(saved["future"], json!({"x":1}));
    assert_eq!(saved["cursor"]["future"], true);
    assert_eq!(saved["timing"]["future"], "kept");
    let mut s = fresh();
    s.feed(
        include_bytes!("../../../tests/contracts/run-monitor-wire.jsonl"),
        true,
        1000,
        false,
    )
    .unwrap();
    assert_eq!(s.latest["step"], 999);
    assert_eq!(s.latest["loss"], 1.5);
    assert_eq!(s.rows["eval"].len(), 2);
    assert_eq!(s.phases.len(), 3);
    assert_eq!(s.progress().unwrap().of, None);
    assert_eq!(s.diagnostics.counts["non_finite"], 3);
    assert_eq!(s.plan.as_ref().map(Vec::len), Some(3));
}

#[test]
fn payload_and_observed_clocks_never_mix_in_rate_learning() {
    let mut s = fresh();
    s.feed(
        b"::orx:: {\"step\":0,\"of\":10,\"t\":0}\n",
        false,
        1_000_000,
        true,
    )
    .unwrap();
    s.feed(b"::orx:: {\"step\":1}\n", false, 1_001_000, true)
        .unwrap();
    assert_eq!(s.timing.progress_points.len(), 1);
    s.feed(b"::orx:: {\"step\":2,\"t\":2}\n", false, 1_002_000, true)
        .unwrap();
    assert_eq!(s.timing.progress_points, vec![(0, 0., 0.), (0, 2., 2.)]);
    let mut s = fresh();
    s.feed(b"::orx:: {\"step\":0,\"of\":10}\n", false, 1_000_000, true)
        .unwrap();
    s.feed(b"::orx:: {\"step\":1,\"t\":1}\n", false, 1_001_000, true)
        .unwrap();
    assert_eq!(s.timing.progress_points, vec![(0, 1., 1.)]);
}

#[test]
fn metric_only_records_do_not_invent_progress_or_historical_time() {
    let mut s = fresh();
    s.feed(b"::orx:: {\"m\":{\"loss\":2}}\n", false, 1000, false)
        .unwrap();
    assert!(s.progress().is_none());
    assert!(s.series.is_empty());
    assert_eq!(s.latest["loss"], 2);
    s.feed(b"::orx:: {\"step\":3,\"of\":10,\"t\":5,\"m\":{\"loss\":1}}\n::orx:: {\"t\":6,\"m\":{\"loss\":0.9}}\n",true,1000,false).unwrap();
    assert_eq!(s.series["loss"].last().unwrap().0, 3.0);
}

#[test]
fn declared_progress_does_not_inherit_a_detectors_clock() {
    let mut s = fresh();
    feed(&mut s, "10it [00:20, 0.5it/s]\r20it [00:40, 0.5it/s]\r");
    feed(&mut s, "::orx:: {\"step\":1,\"of\":10,\"t\":1}\n");
    assert_eq!(s.progress().unwrap().source, "declared");
    assert_eq!(s.progress().unwrap().last_point, Some((1., 1.)));
    assert_eq!(s.progress().unwrap().timing_source, "payload");
}

#[test]
fn incomplete_cr_frames_keep_raw_bytes_and_respect_the_raw_frame_limit() {
    let mut s = fresh();
    feed(&mut s, "::orx:: {\"m\":{\"note\":\"a\r");
    feed(&mut s, "b\"}}\n");
    assert!(!s.latest.contains_key("note"));
    let mut s = fresh();
    feed(&mut s, "::orx:: {");
    s.feed(&vec![b'\r'; MAX_FRAME + 1], false, 1000, true)
        .unwrap();
    assert_eq!(s.diagnostics.counts["oversized_record"], 1);
    assert!(s.cursor.pending_base64.len() <= MAX_FRAME * 2);
    feed(&mut s, "\n::orx:: {\"m\":{\"ok\":1}}\n");
    assert_eq!(s.latest["ok"], 1);
}

#[test]
fn row_history_is_bounded_by_count_and_serialized_bytes() {
    let mut s = fresh();
    let mut bytes = Vec::new();
    let cells: serde_json::Map<String, serde_json::Value> = (0..16)
        .map(|i| (format!("cell{i}"), json!("a\n".repeat(512))))
        .collect();
    for step in 0..300 {
        let line = format!("::orx:: {}\n", json!({"step":step,"row":"eval","m":cells}));
        bytes.extend_from_slice(line.as_bytes());
        if bytes.len() > 1024 * 1024 {
            s.feed(&bytes, false, 1000, false).unwrap();
            bytes.clear();
        }
    }
    s.feed(&bytes, true, 1000, false).unwrap();
    assert!(serde_json::to_vec(&s.rows).unwrap().len() <= 2 * 1024 * 1024);
    assert!(s.omitted["rows"] > 0);
    assert_eq!(s.latest.len(), 16);
    let mut s = fresh();
    let mut bytes = Vec::new();
    for step in 0..2500 {
        bytes.extend_from_slice(
            format!("::orx:: {{\"step\":{step},\"row\":\"eval\",\"m\":{{\"score\":{step}}}}}\n")
                .as_bytes(),
        );
    }
    s.feed(&bytes, true, 1000, false).unwrap();
    assert_eq!(s.rows["eval"].len(), 2048);
    assert_eq!(s.rows["eval"].last().unwrap()["score"], 2499);
}

#[test]
fn invalid_declared_progress_does_not_displace_valid_detected_progress() {
    let mut s = fresh();
    feed(&mut s, "1/10\r2/10\r");
    feed(
        &mut s,
        "::orx:: {\"step\":NaN,\"of\":-1,\"unit\":false,\"m\":{\"loss\":2}}\n",
    );
    assert_eq!(s.progress().unwrap().source, "detected");
    assert_eq!(s.progress().unwrap().done, Some(2.));
    assert_eq!(s.latest["loss"], 2);
}

#[test]
fn phase_bounds_preserve_current_progress_latest_values_and_nonfinite_diagnostics() {
    let mut s = fresh();
    for i in 0..65 {
        feed(
            &mut s,
            &format!(
                "::orx:: {{\"phase\":\"phase{i}\",\"step\":1,\"of\":1,\"m\":{{\"loss\":{}}}}}\n",
                if i == 64 {
                    "NaN".to_string()
                } else {
                    i.to_string()
                }
            ),
        );
    }
    assert_eq!(s.phases.len(), 64);
    assert_eq!(s.progress().unwrap().name, "phase64");
    assert_eq!(s.latest["loss"], "NaN");
    assert_eq!(s.diagnostics.counts["non_finite"], 1);
    assert_eq!(s.omitted["phases"], 1);
}

#[test]
fn coalescing_does_not_hide_a_later_step_back_in_t() {
    let mut s = fresh();
    feed(&mut s,"::orx:: {\"step\":1,\"t\":1,\"m\":{\"loss\":1}}\n::orx:: {\"step\":1,\"t\":3,\"m\":{\"loss\":1}}\n::orx:: {\"step\":2,\"t\":2,\"m\":{\"loss\":2}}\n");
    assert_eq!(s.timing.payload_t, Some(2.));
    assert_eq!(s.diagnostics.counts["t_origin"], 1);
    assert_eq!(s.timing.progress_points, vec![(0, 2., 2.)]);
}

/// A process relaunched inside the run restarts `t` from zero: a new stretch, as normal as the
/// next stage's script, whose reports keep progress and metrics flowing; no duration spans the
/// step. A step back anywhere else means one process's reports disagree, and is counted.
#[test]
fn a_backward_t_is_a_new_origin_never_an_error() {
    let mut s = fresh();
    for step in 0..=5 {
        feed(
            &mut s,
            &format!(
                "::orx:: {{\"phase\":\"train\",\"step\":{step},\"of\":20,\"t\":{}}}\n",
                100 + step * 10
            ),
        );
    }
    s.reached_end(5000);
    for (step, t) in [(6, 0), (7, 10), (8, 20)] {
        feed(
            &mut s,
            &format!("::orx:: {{\"step\":{step},\"t\":{t},\"m\":{{\"loss\":{step}}}}}\n"),
        );
    }
    assert_eq!(s.progress().unwrap().done, Some(8.));
    assert_eq!(s.latest["loss"], 8);
    assert!(
        s.diagnostics.counts.is_empty(),
        "{:?}",
        s.diagnostics.counts
    );
    assert_eq!(
        s.timing.progress_points,
        vec![(0, 0., 6.), (0, 10., 7.), (0, 20., 8.)],
        "no point from before the step"
    );
    assert_eq!(
        s.timing.anchor, None,
        "the old stretch's anchor goes with it"
    );
    assert_eq!(s.timing.last_advance, Some((0, 20.)));

    feed(
        &mut s,
        "::orx:: {\"phase\":\"eval\",\"step\":0,\"of\":4,\"t\":5}\n",
    );
    assert_eq!(s.diagnostics.counts["t_origin"], 1);
    assert_eq!(
        s.phases[0].last_point,
        Some((20., 8.)),
        "a new phase keeps the last"
    );
    assert_eq!(
        s.timing.progress_points.iter().filter(|p| p.0 == 0).count(),
        3
    );
}

#[test]
fn a_step_back_in_t_folds_the_same_at_every_byte_boundary() {
    let input = "::orx:: {\"step\":1,\"of\":9,\"t\":50}\n::orx:: {\"step\":2,\"t\":60}\n::orx:: {\"step\":3,\"t\":30}\n::orx:: {\"step\":4,\"t\":35,\"m\":{\"loss\":1}}\n";
    let mut expected = fresh();
    feed(&mut expected, input);
    assert_eq!(expected.diagnostics.counts["t_origin"], 1);
    for split in 0..=input.len() {
        let mut s = fresh();
        s.feed(&input.as_bytes()[..split], false, 1000, true)
            .unwrap();
        s = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        s.feed(&input.as_bytes()[split..], false, 1000, true)
            .unwrap();
        s.rev = expected.rev;
        assert_eq!(s, expected, "split {split}");
    }
}

#[test]
fn series_compaction_terminates_at_the_largest_finite_coordinate() {
    let mut snapshot = Snapshot::new("run", "1-2", "generation");
    for index in 0..2050 {
        let phase = if index % 2 == 0 { "a" } else { "b" };
        let step = if index % 2 == 0 { 0.0 } else { f64::MAX };
        let record = serde_json::json!({"phase":phase,"step":step,"m":{"score":index}});
        snapshot
            .feed(format!("::orx:: {record}\n").as_bytes(), false, 0, false)
            .unwrap();
    }
    assert!(snapshot.series["score"].len() <= 2048);
    assert_eq!(snapshot.latest["score"], 2049);
    assert_eq!(snapshot.progress().unwrap().done, Some(f64::MAX));
}

#[test]
fn matching_declared_progress_still_takes_priority_over_a_detector() {
    let mut snapshot = Snapshot::new("run", "1-2", "generation");
    snapshot
        .feed(
            b"10%|#| 1/10 [00:01<00:09, 1it/s]\r20%|##| 2/10 [00:02<00:08, 1it/s]\r",
            false,
            0,
            false,
        )
        .unwrap();
    assert_eq!(snapshot.progress().unwrap().source, "detected");
    snapshot
        .feed(
            b"::orx:: {\"step\":2.0,\"of\":10.0,\"unit\":\"it\"}\n",
            false,
            0,
            false,
        )
        .unwrap();
    assert_eq!(snapshot.progress().unwrap().source, "declared");
    assert_eq!(snapshot.progress().unwrap().done, Some(2.0));
}

fn train(steps: std::ops::RangeInclusive<u32>, t0: f64, loss: impl Fn(u32) -> f64) -> String {
    steps
        .map(|step| {
            let t = t0 + f64::from(step) * 0.5;
            format!(
                "::orx:: {{\"step\":{step},\"t\":{t},\"m\":{{\"loss\":{}}}}}\n",
                loss(step)
            )
        })
        .collect()
}

/// A process resumed from an older checkpoint restarts `t` and says where it is: progress moves
/// back to it, each redone step shows once with its new value, and timing starts again there.
#[test]
fn a_relaunch_from_an_older_checkpoint_is_taken_at_its_word() {
    let mut s = fresh();
    feed(&mut s, "::orx:: {\"step\":0,\"of\":2000,\"t\":0}\n");
    feed(&mut s, &train(1..=1200, 0.0, f64::from));
    s.reached_end(5000);
    let rev = s.series_rev;

    feed(&mut s, "::orx:: {\"step\":1000,\"of\":2000,\"t\":0.25}\n");
    assert_eq!(s.progress().unwrap().done, Some(1000.));
    assert_eq!(s.timing.progress_points, vec![(0, 0.25, 1000.)]);
    assert_eq!(s.timing.last_advance, Some((0, 0.25)));
    assert_eq!(s.series["loss"].last(), Some(&(1000., Some(1000.))));
    assert!(s.series_rev > rev, "a reader holding the old points resets");
    s.reached_end(9000);
    assert_eq!(s.timing.anchor, Some((0, 0.25, 9000)));

    feed(
        &mut s,
        &train(1001..=1400, 1.0, |step| f64::from(step) + 0.5),
    );
    assert_eq!(s.progress().unwrap().done, Some(1400.));
    let loss = &s.series["loss"];
    assert_eq!(loss.len(), 1400);
    assert!(loss.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert_eq!(loss[999], (1000., Some(1000.)));
    assert_eq!(loss[1199], (1200., Some(1200.5)));
    assert!(
        s.diagnostics.counts.is_empty(),
        "{:?}",
        s.diagnostics.counts
    );
    assert!(s.timing.progress_points.iter().all(|p| p.1 <= 701.));
}

/// The step a relaunch resumes at is redone: its new value replaces the old, never beside it.
#[test]
fn a_relaunch_replaces_the_value_at_the_step_it_resumes() {
    let mut s = fresh();
    feed(&mut s, "::orx:: {\"step\":0,\"of\":2000,\"t\":0}\n");
    feed(&mut s, &train(1..=1200, 0.0, f64::from));
    let rev = s.series_rev;
    feed(
        &mut s,
        "::orx:: {\"step\":1000,\"t\":0.25,\"m\":{\"loss\":7}}\n",
    );
    let loss = &s.series["loss"];
    assert_eq!(loss.len(), 1000);
    assert_eq!(loss.last(), Some(&(1000., Some(7.))));
    assert!(s.series_rev > rev);
}

/// A second `progress()` with its own total is taken at its word too, in a new process or the
/// same one; it is another loop, so the first loop's points stay.
#[test]
fn a_second_progress_with_its_own_total_counts_from_its_own_start() {
    for t0 in [0.0, 600.0] {
        let mut s = fresh();
        feed(&mut s, "::orx:: {\"step\":0,\"of\":2000,\"t\":0}\n");
        feed(&mut s, &train(1..=1000, 0.0, f64::from));
        feed(
            &mut s,
            &format!("::orx:: {{\"step\":0,\"of\":50,\"t\":{t0}}}\n"),
        );
        feed(&mut s, &train(1..=20, t0, |step| f64::from(step) / 100.));
        let progress = s.progress().unwrap();
        assert_eq!((progress.done, progress.of), (Some(20.), Some(50.)), "{t0}");
        assert_eq!(s.series["loss"].len(), 1020);
        assert_eq!(s.series["loss"][999], (1000., Some(1000.)));
        assert!(
            s.diagnostics.counts.is_empty(),
            "{:?}",
            s.diagnostics.counts
        );
    }
}

/// Without a new origin a lower step is out-of-order noise: counted and sampled, it moves
/// nothing back and plots nothing at a step it does not have.
#[test]
fn a_lower_step_without_a_new_origin_is_out_of_order_noise() {
    let mut s = fresh();
    feed(&mut s, &train(1..=10, 0.0, f64::from));
    let series = s.series.clone();
    let lagging: String = (0..200)
        .map(|i| {
            let t = if i % 2 == 0 {
                format!(",\"t\":{}", 10 + i)
            } else {
                String::new()
            };
            format!("::orx:: {{\"step\":5{t},\"m\":{{\"loss\":{}}}}}\n", 100 + i)
        })
        .collect();
    feed(&mut s, &lagging);
    assert_eq!(s.progress().unwrap().done, Some(10.));
    assert_eq!(s.series, series);
    assert_eq!(s.diagnostics.counts["invalid_field"], 200);
    assert!(!s.diagnostics.counts.contains_key("conflicting_report"));
    assert!(!s.diagnostics.counts.contains_key("t_origin"));
    assert_eq!(s.diagnostics.examples.len(), 16);
    feed(&mut s, &train(11..=11, 200.0, f64::from));
    assert_eq!(s.series["loss"].last(), Some(&(11., Some(11.))));
}

/// Reports after a relaunch that do not advance leave nothing to place: the old stretch's last
/// advance went with it.
#[test]
fn a_relaunch_that_has_not_advanced_is_not_placed_from_the_old_stretch() {
    let mut s = fresh();
    feed(&mut s, &train(1..=20, 100.0, f64::from));
    s.reached_end(5000);
    assert_eq!(s.timing.anchor, Some((0, 110., 5000)));
    feed(
        &mut s,
        "::orx:: {\"t\":0,\"m\":{\"lr\":0.1}}\n::orx:: {\"t\":1,\"m\":{\"lr\":0.2}}\n",
    );
    s.reached_end(9000);
    assert_eq!(s.timing.last_advance, None);
    assert_eq!(s.timing.anchor, None);
}

#[test]
fn a_relaunch_folds_the_same_at_every_byte_boundary() {
    let input = "::orx:: {\"step\":1,\"of\":9,\"t\":50,\"m\":{\"loss\":9}}\n::orx:: {\"step\":2,\"t\":60,\"m\":{\"loss\":8}}\n::orx:: {\"step\":3,\"t\":70,\"m\":{\"loss\":7}}\n::orx:: {\"step\":1,\"of\":9,\"t\":0}\n::orx:: {\"step\":2,\"t\":5,\"m\":{\"loss\":6}}\n::orx:: {\"step\":1,\"t\":6,\"m\":{\"loss\":1}}\n";
    let mut expected = fresh();
    feed(&mut expected, input);
    assert_eq!(expected.progress().unwrap().done, Some(2.));
    assert_eq!(
        expected.series["loss"],
        vec![(1., Some(9.)), (2., Some(6.))]
    );
    for split in 0..=input.len() {
        let mut s = fresh();
        s.feed(&input.as_bytes()[..split], false, 1000, true)
            .unwrap();
        s = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        s.feed(&input.as_bytes()[split..], false, 1000, true)
            .unwrap();
        s.rev = expected.rev;
        assert_eq!(s, expected, "split {split}");
    }
}

/// A relaunch whose first report repeats the last one but for `t` still starts its stretch
/// there, so a relaunch that hangs at once is placed and shows its silence.
#[test]
fn a_relaunch_repeating_the_last_report_starts_its_stretch_there() {
    let mut s = fresh();
    feed(
        &mut s,
        "::orx:: {\"step\":2,\"of\":10,\"t\":20}\n::orx:: {\"step\":3,\"of\":10,\"t\":30}\n",
    );
    feed(&mut s, "::orx:: {\"step\":3,\"of\":10,\"t\":0}\n");
    assert_eq!(s.timing.progress_points, vec![(0, 0., 3.)]);
    s.reached_end(9000);
    assert_eq!(s.timing.anchor, Some((0, 0., 9000)));
}

#[test]
fn signals_and_columns_keep_the_order_they_were_printed_in() {
    let mut s = fresh();
    feed(
        &mut s,
        "::orx:: {\"step\":1,\"m\":{\"lr\":0.0003,\"loss\":2.5}}\n\
         ::orx:: {\"row\":\"rounds\",\"m\":{\"round\":1,\"win_rate\":0.5,\"elo\":1200,\"game_len\":80}}\n\
         ::orx:: {\"row\":\"rounds\",\"m\":{\"round\":2,\"accuracy\":0.6,\"win_rate\":0.5,\"elo\":1210,\"game_len\":81}}\n\
         ::orx:: {\"step\":2,\"m\":{\"loss\":2.4,\"tokens\":9042}}\n",
    );
    assert_eq!(
        s.signal_order,
        ["lr", "loss", "round", "win_rate", "elo", "game_len", "accuracy", "tokens"]
    );
    assert_eq!(
        s.row_columns["rounds"],
        ["round", "win_rate", "elo", "game_len", "accuracy"]
    );
    let restored: Snapshot = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
    assert_eq!(restored.signal_order, s.signal_order);
    assert_eq!(restored.row_columns, s.row_columns);
}

/// A frozen benchmark harness prints no `::orx::` lines and cannot be changed; four patterns read it.
fn bench_patterns() -> orx_signals::Config {
    serde_json::from_value(json!({"patterns": [
        {"regex": "^chunk (?P<step>\\d+)/(?P<of>\\d+) done", "phase": "compress", "unit": "chunk"},
        {"regex": "^== reference self-check", "phase": "score"},
        {"regex": "^\\s*(?P<level>\\d)\\s+(?P<ref_ms>[\\d.]+)\\s+(?P<cand_ms>[\\d.]+)\\s+(?P<speedup>[\\d.]+)\\s+(?P<ref_kb>[\\d.]+)\\s+(?P<cand_kb>[\\d.]+)\\s+(?P<size_ratio>[\\d.]+)$", "row": "levels"},
        {"regex": "^METRIC (?P<name>\\w+) (?P<value>\\S+)$"},
    ]}))
    .unwrap()
}

#[test]
fn patterns_read_a_log_the_agent_cannot_change() {
    let config = bench_patterns();
    let patterns = std::sync::Arc::new(orx_signals::Patterns::compile(&config).unwrap());
    let mut s = fresh();
    s.tally = Some(Default::default());
    s.configure(Some(patterns));
    assert_eq!(s.config_hash, config.hash());
    assert_ne!(s.config_hash, orx_signals::DEFAULT_CONFIG_HASH);
    s.feed(include_bytes!("fixtures/codec.out"), true, 1000, false)
        .unwrap();

    let names: Vec<(&str, &str)> = s
        .phases
        .iter()
        .map(|p| (p.name.as_str(), p.source.as_str()))
        .collect();
    assert_eq!(names, [("compress", "pattern"), ("score", "pattern")]);
    assert_eq!((s.phases[0].done, s.phases[0].of), (Some(10.0), Some(10.0)));
    assert_eq!(s.phases[0].unit.as_deref(), Some("chunk"));
    let levels = &s.rows["levels"];
    assert_eq!(levels.len(), 9);
    assert_eq!(levels[4]["cand_ms"], json!(46.29));
    assert_eq!(levels[0]["level"], json!(1.0));
    assert_eq!(s.latest["front_gain"], json!(0.0213));
    assert_eq!(s.latest["valid"], json!(1.0));
    // Nothing in the config lines, the header or the frontier lines is taken for a signal.
    assert!(!s.latest.contains_key("load"));
    assert_eq!(s.row_columns["levels"].len(), 7);

    let tally = s.tally.as_ref().unwrap();
    assert_eq!(tally.lines, 40);
    let matched: Vec<u64> = tally.hits.iter().map(|h| h.matched).collect();
    assert_eq!(matched, [10, 1, 9, 10]);
    assert_eq!(
        tally.hits[1].first,
        Some((17, "== reference self-check: ok".into()))
    );
}

#[test]
fn a_pattern_that_cannot_be_used_says_which_and_where() {
    let compile = |patterns: serde_json::Value| {
        orx_signals::Patterns::compile(
            &serde_json::from_value(json!({"patterns": patterns})).unwrap(),
        )
        .unwrap_err()
    };
    let error = compile(json!([{"regex": "^ok$"}, {"regex": "^chunk (?P<step\\d+)"}]));
    assert_eq!((error.index, error.column), (1, Some(16)));
    assert!(
        error.reason.contains("invalid capture group"),
        "{}",
        error.reason
    );
    let error = compile(json!([{"regex": "^METRIC (?P<name>\\w+)"}]));
    assert_eq!(
        error.reason,
        "groups name and value go together: the log names the signal and gives its value"
    );
    let error = compile(json!([{"regex": "(?<=x)y"}]));
    assert!(error.reason.contains("look-around"), "{}", error.reason);
    let error = compile(json!([{"regex": "^x", "row": "eval set"}]));
    assert_eq!(
        error.reason,
        "row \"eval set\" is not a name: use letters, digits and _"
    );
}

#[test]
fn a_run_declares_its_plan_and_a_later_one_replaces_it() {
    let mut s = fresh();
    feed(
        &mut s,
        "::orx:: {\"plan\":[{\"phase\":\"install\"},{\"phase\":\"train\",\"of\":5000,\"unit\":\"steps\",\"why\":\"later\"}]}\n",
    );
    let plan = s.plan.clone().unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(
        (
            plan[1].phase.as_str(),
            plan[1].of.as_ref().and_then(serde_json::Number::as_f64),
            plan[1].unit.as_deref()
        ),
        ("train", Some(5000.0), Some("steps"))
    );
    assert!(s.phases.is_empty(), "a plan begins no stage");
    assert!(
        s.diagnostics.counts.is_empty(),
        "a key a stage does not know is ignored"
    );

    feed(
        &mut s,
        "::orx:: {\"plan\":[{\"phase\":\"train\"},{\"phase\":\"train\"}]}\n",
    );
    feed(&mut s, "::orx:: {\"plan\":[{\"of\":3}]}\n");
    feed(&mut s, "::orx:: {\"plan\":{\"phase\":\"train\"}}\n");
    assert_eq!(
        s.plan.as_ref().unwrap().len(),
        2,
        "a plan that cannot be read keeps the last one"
    );
    assert_eq!(s.diagnostics.counts["invalid_field"], 3);

    feed(&mut s, "::orx:: {\"phase\":\"train\",\"step\":0,\"plan\":[{\"phase\":\"train\"},{\"phase\":\"eval\"},{\"phase\":\"train\"}]}\n");
    let names: Vec<_> = s
        .plan
        .as_ref()
        .unwrap()
        .iter()
        .map(|stage| stage.phase.as_str())
        .collect();
    assert_eq!(
        names,
        ["train", "eval", "train"],
        "a stage that runs again is listed again"
    );
    assert_eq!(s.progress().unwrap().name, "train");

    let restored: Snapshot = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(restored.plan, s.plan);
}

#[test]
fn a_plan_an_agent_writes_is_read_strictly() {
    let refused = |value: serde_json::Value| orx_signals::plan(&value, true).unwrap_err();
    assert!(refused(json!([{"phase": "train", "steps": 3}])).contains("unknown key \"steps\""));
    assert!(refused(json!([{"phase": ""}])).contains("stage 1 needs \"phase\""));
    assert!(refused(json!([{"phase": "a"}, {"phase": "b", "of": 0}])).contains("stage 2's \"of\""));
    assert!(
        refused(json!([{"phase": "a"}, {"phase": "a"}])).contains("stages 1 and 2 are both \"a\"")
    );
    assert!(refused(json!(vec![json!({"phase": "a"}); 65])).contains("at most 64"));
    assert!(refused(json!("train")).contains("a plan is a list"));
    assert_eq!(orx_signals::plan(&json!([]), true).unwrap(), vec![]);
}
