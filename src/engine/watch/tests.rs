use super::*;
use crate::local::model::LocalExperiment;

/// A `claude` on the test home's PATH that answers each one-shot with the next reply in `dir`,
/// as Claude Code's JSON result, and keeps what it was asked.
fn fake_claude(dir: &std::path::Path, replies: &[&str]) {
    std::fs::create_dir_all(dir).unwrap();
    for (index, reply) in replies.iter().enumerate() {
        std::fs::write(
            dir.join(format!("reply-{}", index + 1)),
            serde_json::to_string(reply).unwrap(),
        )
        .unwrap();
    }
    let bin = crate::config::home_dir().unwrap().join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = format!(
        r#"#!/bin/sh
dir='{}'
n=$(cat "$dir/calls" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$dir/calls"
printf '%s\n' "$@" > "$dir/args-$n"
printf '%s' "$2" > "$dir/prompt-$n"
reply=$(cat "$dir/reply-$n")
printf '{{"type":"result","is_error":false,"result":%s,"total_cost_usd":0.002,"usage":{{"input_tokens":3000,"output_tokens":60}}}}' "$reply"
"#,
        dir.display()
    );
    orx_test_support::write_executable(bin.join("claude"), script, 0o755).unwrap();
}

fn calls(dir: &std::path::Path) -> usize {
    std::fs::read_to_string(dir.join("calls")).map_or(0, |n| n.trim().parse().unwrap())
}

fn chat(store: &Store, id: &str, harness: &str) {
    store
        .create_chat_session(&StoredChatSession {
            profile_id: None,
            id: id.into(),
            project_id: "project".into(),
            harness: harness.into(),
            native_session_id: None,
            native_fork_at: None,
            title: None,
            title_source: None,
            model: None,
            service_tier: None,
            permission_mode: None,
            plan_mode: false,
            plan_reset_pending: false,
            reasoning_level: None,
            archived: false,
            context_usage_json: None,
            bootstrap_context: None,
            goal: None,
            autonomy: None,
            active_leaf_id: None,
            side_parent_session_id: None,
            side_parent_leaf_id: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
}

/// A running run of its own experiment, launched by `chat`, whose log the laptop keeps.
fn run(store: &Store, id: &str, title: &str, chat: &str, log: &str) -> StoredRun {
    store
        .create_local_experiment(&LocalExperiment {
            id: format!("exp-{id}"),
            project_id: "project".into(),
            parent_experiment_id: None,
            slug: format!("exp-{id}"),
            branch_name: format!("orx/exp-{id}"),
            title: Some(title.into()),
            description: None,
            run_command: "train".into(),
            agent_status: "idle".into(),
            created_at: 1,
            updated_at: 1,
            chat_session_id: Some(chat.into()),
            archived: false,
        })
        .unwrap();
    let now = crate::store::now_ms();
    let row = StoredRun {
        id: id.into(),
        experiment_id: format!("exp-{id}"),
        project_id: "project".into(),
        status: "running".into(),
        backend_json: r#"{"kind":"local"}"#.into(),
        command: "train".into(),
        created_at: now - 60_000,
        updated_at: now,
        ended_at: None,
        exit_code: None,
        commit_sha: None,
        result_markdown: None,
        cancel_requested: false,
        cancel_reason: None,
        chat_session_id: Some(chat.into()),
    };
    store.upsert_run(&row).unwrap();
    keep_log(store, id, log);
    row
}

fn keep_log(store: &Store, id: &str, log: &str) {
    let file = store.data_dir().join("run-logs").join(format!("{id}.log"));
    std::fs::write(&file, log).unwrap();
    crate::commands::up::api_v2::keep_ranges(
        store.data_dir(),
        id,
        vec![crate::commands::up::api_v2::KeptRange {
            offset: 0,
            len: log.len() as u64,
            file,
            ident: crate::engine::FileIdent { dev: 1, ino: 7 },
            eof: true,
        }],
    )
    .unwrap();
}

fn look(store: &Store, chat: &str, kind: &str, runs: &[(&str, &str)]) -> i64 {
    let rows: Vec<(&str, &str, &str)> = runs
        .iter()
        .map(|(id, key)| (*id, *key, "subject"))
        .collect();
    store
        .prepare_watch_look(
            chat,
            kind,
            "Claude Haiku",
            "a test asked",
            &rows,
            crate::store::now_ms(),
        )
        .unwrap()
}

const LOG: &str = "config: depth=4 width=512\nWARNING --depth 8 overrides DEPTH; using 4\nstep 100 loss 2.1\nretrying shard 3 (timed out)\nretrying shard 4 (timed out)\n";

#[tokio::test]
async fn one_look_writes_each_runs_sentence_and_the_summary_through_the_agent_cli() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let calls_dir = crate::config::home_dir().unwrap().join("claude-calls");
    fake_claude(
        &calls_dir,
        &[
            r#"{"runs": {"r1": {"status": "ok", "line": "Retried **{count:\"timed out\"}** shards since the start."},
                       "r2": {"status": "flag", "line": "Prints depth **4** though its title says 8.", "evidence": [{"from": 26, "to": 69}]}},
              "summary": {"status": "ok", "line": "{r2} may run the wrong depth: it prints depth **4**, not 8."},
              "notes": "watch the depth"}"#,
        ],
    );
    let store = Store::open().unwrap();
    chat(&store, "chat", "claude-code");
    run(&store, "a", "Depth 6", "chat", LOG);
    run(&store, "b", "Depth 8", "chat", LOG);
    let id = look(&store, "chat", "ceiling", &[("a", "r1"), ("b", "r2")]);

    take(id).await.unwrap();

    assert_eq!(calls(&calls_dir), 1, "one call for both runs");
    let args = std::fs::read_to_string(calls_dir.join("args-1")).unwrap();
    for flag in [
        "--tools\n\n",
        "--strict-mcp-config",
        "--output-format\njson",
        "--model\nhaiku",
    ] {
        assert!(args.contains(flag), "{flag} in {args}");
    }
    let prompt = std::fs::read_to_string(calls_dir.join("prompt-1")).unwrap();
    assert!(
        prompt.contains("\"key\":\"r1\"") && prompt.contains("\"key\":\"r2\""),
        "{prompt}"
    );
    assert!(
        prompt.contains("overrides DEPTH"),
        "the log's head reaches the look: {prompt}"
    );
    let look = store.watch_look(id).unwrap().unwrap();
    assert_eq!(look.state, "done");
    assert_eq!(look.usage.unwrap().cost_usd, Some(0.002));
    let results = store.watch_look_runs(id).unwrap();
    assert_eq!(results[0].result.as_deref(), Some("line"));
    assert_eq!(results[1].result.as_deref(), Some("flag"));
    assert_eq!(
        results[1].text.as_deref(),
        Some("Prints depth **4** though its title says 8.")
    );
    assert_eq!(results[1].evidence, json!([{"from": 26, "to": 69}]));
    assert_eq!(
        store.watch_counts("a").unwrap()[0].0,
        "timed out",
        "a counted pattern starts counting from the log's start"
    );
    let summary = chat_summary(&store, "chat").unwrap();
    assert_eq!(
        summary["summary"]["template"],
        "{r2} may run the wrong depth: it prints depth **4**, not 8."
    );
    assert_eq!(summary["keys"]["r2"], "b");
    let facts = run_facts(&store, &store.get_run("b").unwrap().unwrap()).unwrap();
    assert_eq!(facts["state"], "on");
    assert_eq!(facts["model"], "Claude Haiku");
    assert_eq!(facts["looks"][0]["said"], "flag");
    assert_eq!(facts["looks"][0]["with"], json!(["Depth 6"]));
    assert_eq!(
        facts["spentUsd"],
        json!(0.001),
        "a look's cost is split over its runs"
    );
    assert_eq!(
        facts["sentence"]["template"],
        "Prints depth **4** though its title says 8."
    );
}

#[tokio::test]
async fn a_reply_that_breaks_the_contract_is_asked_for_once_more_with_why() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let calls_dir = crate::config::home_dir().unwrap().join("claude-calls");
    fake_claude(
        &calls_dir,
        &[
            r#"{"runs": {"r1": {"status": "ok", "line": "Two shards retried and it is fine."}}}"#,
            r#"{"runs": {"r1": {"status": "ok", "line": "**{count:\"timed out\"}** shards retried; nothing else."}}}"#,
        ],
    );
    let store = Store::open().unwrap();
    chat(&store, "chat", "claude-code");
    run(&store, "a", "Depth 6", "chat", LOG);
    let id = look(&store, "chat", "ceiling", &[("a", "r1")]);

    take(id).await.unwrap();

    assert_eq!(calls(&calls_dir), 2);
    let again = std::fs::read_to_string(calls_dir.join("prompt-2")).unwrap();
    assert!(
        again.contains("It was not accepted: r1: mark exactly one figure in **bold**"),
        "{again}"
    );
    let results = store.watch_look_runs(id).unwrap();
    assert_eq!(results[0].result.as_deref(), Some("line"));
    let usage = store.watch_look(id).unwrap().unwrap().usage.unwrap();
    assert_eq!(
        (usage.input_tokens, usage.cost_usd),
        (6000, Some(0.004)),
        "both calls are counted"
    );
}

#[tokio::test]
async fn a_reply_still_wrong_after_its_retry_keeps_the_last_sentence_and_says_why() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let calls_dir = crate::config::home_dir().unwrap().join("claude-calls");
    let wrong = r#"{"runs": {"r1": {"status": "ok", "line": "No bold here."}}}"#;
    fake_claude(&calls_dir, &[wrong, wrong]);
    let store = Store::open().unwrap();
    chat(&store, "chat", "claude-code");
    run(&store, "a", "Depth 6", "chat", LOG);
    let id = look(&store, "chat", "ceiling", &[("a", "r1")]);

    take(id).await.unwrap();

    assert_eq!(calls(&calls_dir), 2, "one retry, never a third call");
    let look = store.watch_look(id).unwrap().unwrap();
    assert_eq!(look.state, "error");
    assert!(look.error.unwrap().contains("bold"));
    let facts = run_facts(&store, &store.get_run("a").unwrap().unwrap()).unwrap();
    assert_eq!(facts["sentence"], Value::Null);
    assert_eq!(facts["error"]["code"], "error");
}

#[tokio::test]
async fn a_credential_in_the_digest_stops_the_look_before_anything_is_sent() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let calls_dir = crate::config::home_dir().unwrap().join("claude-calls");
    fake_claude(&calls_dir, &[r#"{"runs": {}}"#]);
    let store = Store::open().unwrap();
    chat(&store, "chat", "claude-code");
    let token = format!("ghp_{}", "a1B2c3D4".repeat(5));
    run(
        &store,
        "a",
        "Depth 6",
        "chat",
        &format!("{LOG}export GITHUB_TOKEN={token}\n"),
    );
    let id = look(&store, "chat", "ceiling", &[("a", "r1")]);

    let error = take(id).await.unwrap_err();

    assert_eq!(calls(&calls_dir), 0, "nothing reached the model");
    let message = format!("{error:#}");
    assert!(message.contains("the log of Depth 6"), "{message}");
    assert!(message.contains("GitHub token"), "{message}");
    assert!(
        !message.contains(&token),
        "the credential itself is never repeated"
    );
    assert_eq!(
        store.watch_look(id).unwrap().unwrap().state,
        "prepared",
        "never marked sent"
    );
}

#[test]
fn a_look_orx_stopped_after_sending_is_interrupted_and_never_sent_again() {
    let dir = std::env::temp_dir().join(format!("orx-watch-{}", uuid::Uuid::new_v4()));
    let store = Store::open_at(dir).unwrap();
    let sent = look(&store, "chat", "ceiling", &[("a", "r1"), ("b", "r2")]);
    let waiting = look(&store, "chat", "manual", &[("a", "r1")]);
    assert!(store.watch_look_sent(sent, 5).unwrap());

    let prepared = store.recover_watch_looks(10).unwrap();

    assert_eq!(
        prepared.iter().map(|look| look.id).collect::<Vec<_>>(),
        [waiting]
    );
    assert_eq!(
        store.watch_look(sent).unwrap().unwrap().state,
        "interrupted"
    );
    assert!(store
        .watch_look_runs(sent)
        .unwrap()
        .iter()
        .all(|run| run.result.as_deref() == Some("interrupted")));
    assert!(
        !store.watch_look_sent(sent, 11).unwrap(),
        "an interrupted look is not sent again"
    );
}

#[test]
fn each_agent_answers_which_model_watches_its_chat() {
    if !crate::config::in_test_child(&["ORX_WATCH_WITH"]) {
        return;
    }
    let store = Store::open().unwrap();
    chat(&store, "claude", "claude-code");
    chat(&store, "open", "opencode");
    let of =
        |id: &str| watching_model(&store, &store.get_chat_session(id).unwrap().unwrap()).unwrap();
    assert!(
        matches!(of("claude"), Watching::Selected { label, model: None, .. } if label == "Claude Haiku")
    );
    assert_eq!(of("open"), Watching::NeedsPick);

    std::env::set_var("ORX_WATCH_WITH", "claude-code");
    assert!(matches!(of("open"), Watching::Selected { harness, .. } if harness == "claude-code"));

    store.set_run_watching(false).unwrap();
    assert_eq!(of("claude"), Watching::Off);
}

#[test]
fn the_ladder_restarts_with_each_stage_and_never_waits_past_the_longest_gap() {
    let minute = paced_ms(60);
    assert_eq!(ladder_slots(0, paced_ms(1800)), 0);
    assert_eq!(ladder_slots(minute, paced_ms(1800)), 1);
    assert_eq!(ladder_slots(5 * minute, paced_ms(1800)), 3);
    assert_eq!(ladder_slots(50 * minute, paced_ms(1800)), 6);
    assert_eq!(
        ladder_slots(10 * minute, paced_ms(120)),
        6,
        "a gap shorter than the ladder caps it"
    );
}

#[test]
fn a_number_leaves_its_trend_only_past_three_spreads() {
    let falling: Vec<(f64, f64)> = (0..10)
        .map(|i| {
            (
                i as f64 * 10.0,
                3.0 - i as f64 * 0.1 + if i % 2 == 0 { 0.01 } else { -0.01 },
            )
        })
        .collect();
    let line = fit(&falling).unwrap();
    assert!((line.slope + 0.01).abs() < 1e-3, "{line:?}");
    let expected = line.intercept + line.slope * 110.0;
    assert!(
        (1.91 - expected).abs() < 3.0 * line.spread,
        "a point on the trend stays"
    );
    assert!(
        (2.6 - expected).abs() > 3.0 * line.spread,
        "a turn upward leaves it"
    );
    assert_eq!(fit(&falling[..4]), None, "no trend before the fifth point");
}
