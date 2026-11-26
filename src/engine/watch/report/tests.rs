// The update tests fake the agent CLI with shell scripts, so most helpers serve only Unix.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use super::*;
use crate::local::chat::WirePart;
use crate::store::{StoredChatMessage, NOTES_KEPT};

fn host() -> Arc<ChatHost> {
    Arc::new(ChatHost::new(
        Arc::new(crate::local::opencode::AgentHost::new(None)),
        Arc::new(crate::local::codex::CodexHost::new()),
        Arc::new(crate::local::claude::ClaudeHost::new()),
    ))
}

/// Runs one pass and waits for every update it started to end.
async fn settle(host: &Arc<ChatHost>) {
    let mut events = host.subscribe();
    pass(host).await.unwrap();
    while !writing().lock().unwrap().is_empty() {
        tokio::time::timeout(Duration::from_secs(30), events.recv())
            .await
            .expect("an update ends")
            .unwrap();
    }
}

fn say(store: &Store, chat: &str, id: &str, role: &str, text: &str, at: i64) {
    store
        .upsert_chat_message(&StoredChatMessage {
            id: id.into(),
            session_id: chat.into(),
            role: role.into(),
            parts_json: serde_json::to_string(&[WirePart::text(id, text)]).unwrap(),
            created_at: at,
            completed_at: Some(at),
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
            result_native_message_id: None,
            native_harness: None,
        })
        .unwrap();
}

fn chat(store: &Store, id: &str, harness: &str) -> StoredChatSession {
    store.put_chat_session(id, harness);
    store.get_chat_session(id).unwrap().unwrap()
}

/// Notes a milestone that `chat` keeps.
fn noted(store: &Store, chat: &StoredChatSession, note: &str) {
    let noted = milestone(store, chat, note).unwrap();
    assert!(matches!(noted, Milestone::Noted), "{noted:?}");
}

/// Why a milestone of `chat` was not noted.
fn skipped(store: &Store, chat: &StoredChatSession) -> String {
    match milestone(store, chat, "x").unwrap() {
        Milestone::Skipped(why) => why,
        Milestone::Noted => panic!("noted"),
    }
}

fn notes(report: &ChatReport) -> Vec<&str> {
    report.notes.iter().map(|(_, note)| note.as_str()).collect()
}

/// Chat `chat` on Claude Code, its report already open.
fn opened(store: &Store) -> StoredChatSession {
    let session = chat(store, "chat", "claude-code");
    store.set_chat_report_enabled("chat", true, 1).unwrap();
    session
}

fn prompt(dir: &std::path::Path, call: usize) -> String {
    std::fs::read_to_string(dir.join(format!("prompt-{call}"))).unwrap()
}

/// Moves the report's last update back past the gap, as time passing would.
fn age(store: &Store, chat: &str) {
    store.conn_execute_for_tests(&format!(
        "UPDATE chat_reports SET error_at = error_at - {gap} WHERE chat_session_id = '{chat}'",
        gap = paced_ms(MIN_GAP_S) + 1
    ));
    store.conn_execute_for_tests(&format!(
        "UPDATE chat_report_versions SET written_at = written_at - {gap}
         WHERE chat_session_id = '{chat}'",
        gap = paced_ms(MIN_GAP_S) + 1
    ));
}

/// One search/replace edit, as the model writes it.
fn edit(search: &str, replace: &str) -> String {
    format!("<<<<<<< SEARCH\n{search}\n=======\n{replace}\n>>>>>>> REPLACE")
}

const FIRST_LINE: &str = "Training width 512, the best so far at val loss 0.341.";
const REPORT: &str = "Training width 512, the best so far at val loss 0.341.\n\n## Findings\n- Width 512 reached 0.341, below the baseline's 0.388.";

#[cfg(unix)]
#[tokio::test]
async fn a_milestone_mid_turn_writes_the_report_from_the_chat_its_runs_and_its_notes() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let cited = format!("{REPORT} <run id=\"r1\"/>");
    super::super::tests::fake_claude(&dir, &[&format!("```markdown\n{cited}\n```")]);
    let store = Store::open().unwrap();
    let session = opened(&store);
    let token = format!("sk-ant-api03-{}", "a1".repeat(20));
    say(
        &store,
        "chat",
        "m0",
        "user",
        &format!("My key is {token}"),
        9,
    );
    say(&store, "chat", "m1", "user", "Find the best width.", 10);
    say(
        &store,
        "chat",
        "m2",
        "assistant",
        "Width 512 reached val loss 0.341; the baseline was 0.388.",
        20,
    );
    super::super::tests::run(&store, "r1", "Width 512", "chat", "step 1 loss 0.4\n");
    store.put_chat_turn("chat", "turn", "running", 5);
    let host = host();

    noted(&store, &session, "Width 512 beat the baseline");
    settle(&host).await;

    assert_eq!(
        super::super::tests::calls(&dir),
        1,
        "a milestone does not wait for the turn"
    );
    let sent = prompt(&dir, 1);
    for shown in [
        "Width 512 beat the baseline",
        "Find the best width",
        "0.388",
        "\"run\":\"r1\"",
    ] {
        assert!(sent.contains(shown), "{shown} in {sent}");
    }
    assert!(
        !sent.contains("sk-ant-"),
        "a credential never reaches the model: {sent}"
    );
    let args = std::fs::read_to_string(dir.join("args-1")).unwrap();
    assert!(
        args.contains("--tools\n\n"),
        "an update has no tools: {args}"
    );
    assert!(
        args.contains("Delete only what the inputs make out of date"),
        "{args}"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(
        report.body.as_deref(),
        Some(cited.as_str()),
        "the fence around the reply is dropped"
    );
    assert!(report.notes.is_empty() && report.due_at.is_none());
    let shown = report_json(&store, "chat").unwrap();
    assert_eq!(
        shown,
        json!({"body": cited, "version": report.version, "read": false, "updatedAt": report.updated_at, "enabled": true, "writing": false, "error": null, "needsModel": null})
    );
}

#[cfg(unix)]
#[tokio::test]
async fn results_and_launches_wait_for_the_turn_and_updates_keep_their_gap() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let second = edit(FIRST_LINE, "Trying depth 8 after width 512 won.");
    super::super::tests::fake_claude(&dir, &[REPORT, &second]);
    let store = Store::open().unwrap();
    let session = chat(&store, "chat", "claude-code");
    say(
        &store,
        "chat",
        "m1",
        "assistant",
        "Width 512 reached val loss 0.341; the baseline was 0.388.",
        10,
    );
    store.put_chat_turn("chat", "reaction", "running", 1);
    store.set_chat_report_enabled("chat", true, 1).unwrap();
    store.conn_execute_for_tests("UPDATE chat_reports SET due_at = NULL");
    let host = host();

    after_turn(&store, "chat").unwrap();
    settle(&host).await;
    assert_eq!(
        super::super::tests::calls(&dir),
        0,
        "the reaction is still going"
    );

    store.complete_chat_turn("reaction").unwrap();
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 1);
    assert!(store
        .chat_report("chat")
        .unwrap()
        .unwrap()
        .turn_due_at
        .is_none());

    // Inside the gap a milestone waits, and every note made meanwhile reaches the next update.
    noted(&store, &session, "Width 512 won");
    noted(&store, &session, "Depth 8 is next");
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 1, "the gap defers it");
    assert_eq!(store.chat_report("chat").unwrap().unwrap().notes.len(), 2);

    age(&store, "chat");
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 2);
    let sent = prompt(&dir, 2);
    assert!(
        sent.contains("Width 512 won") && sent.contains("Depth 8 is next"),
        "{sent}"
    );
    assert!(
        sent.contains("Training width 512"),
        "the previous report is edited: {sent}"
    );
    assert!(
        !sent.contains("the baseline was 0.388"),
        "only what is new since: {sent}"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    let expected = REPORT.replace(FIRST_LINE, "Trying depth 8 after width 512 won.");
    assert_eq!(report.body.as_deref(), Some(expected.as_str()));
    assert!(report.notes.is_empty());
}

/// The fifos that hold the first model call until the test lets it answer: the call says it is
/// waiting on the first, then reads the second.
#[cfg(unix)]
fn paused(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let (ready, resume) = (dir.join("ready"), dir.join("resume"));
    assert!(crate::process::command("mkfifo")
        .args([&ready, &resume])
        .status()
        .unwrap()
        .success());
    (ready, resume)
}

/// Runs one update of `chat` with `between` done while its first model call waits.
#[cfg(unix)]
async fn while_writing(store: &Store, dir: &std::path::Path, between: impl FnOnce()) {
    let (ready, resume) = paused(dir);
    let session = store.get_chat_session("chat").unwrap().unwrap();
    let watching = watching_model(store, &session).unwrap();
    let update = tokio::spawn(async move { write("chat", &watching).await });
    crate::process::tokio_command("cat")
        .arg(&ready)
        .output()
        .await
        .unwrap();
    between();
    std::fs::write(&resume, "go\n").unwrap();
    update.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn what_lands_while_an_update_is_written_reaches_the_next_one() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &[REPORT, "UNCHANGED"]);
    let store = Store::open().unwrap();
    let session = opened(&store);
    say(
        &store,
        "chat",
        "m1",
        "assistant",
        "Width 512 reached val loss 0.341; the baseline was 0.388.",
        10,
    );
    for note in 0..NOTES_KEPT {
        noted(&store, &session, &format!("Note {note}"));
    }
    let mut delivered = 0;

    while_writing(&store, &dir, || {
        delivered = crate::store::now_ms();
        say(&store, "chat", "m2", "user", "Try depth 8 next.", delivered);
        noted(&store, &session, "Depth 8 is next");
        store
            .insert_run_notice(
                "r9",
                "chat",
                "terminal",
                "end",
                "Run r9 finished.",
                delivered,
            )
            .unwrap();
        store.conn_execute_for_tests(&format!(
            "UPDATE run_notices SET delivered_at = {delivered}"
        ));
    })
    .await;

    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some(REPORT));
    assert!(
        report.updated_at.unwrap() < delivered,
        "it is as of when its inputs were read"
    );
    assert_eq!(
        notes(&report),
        ["Depth 8 is next"],
        "a note made meanwhile outlives the ones the update read"
    );

    age(&store, "chat");
    store
        .chat_report_due("chat", None, crate::store::now_ms())
        .unwrap();
    settle(&host()).await;
    let sent = prompt(&dir, 2);
    for shown in ["Try depth 8 next.", "Depth 8 is next", "Run r9 finished."] {
        assert!(sent.contains(shown), "{shown} in {sent}");
    }
    assert!(store.chat_report("chat").unwrap().unwrap().notes.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn run_watching_turned_off_mid_update_keeps_its_milestones() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &["Width 512 leads by 12.5%."]);
    let store = Store::open().unwrap();
    let session = opened(&store);
    noted(&store, &session, "Width 512 won");

    while_writing(&store, &dir, || store.set_run_watching(false).unwrap()).await;

    assert_eq!(
        super::super::tests::calls(&dir),
        1,
        "no correction is asked for"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert!(report.body.is_none());
    assert!(report.due_at.is_some());
    assert_eq!(notes(&report), ["Width 512 won"]);
    assert!(report.error.unwrap().contains("Run watching is off"));
}

#[cfg(unix)]
#[tokio::test]
async fn an_update_whose_inputs_cant_be_read_is_held_and_tried_again() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &["Width 512 is training. <run id=\"r1\"/>"]);
    let store = Store::open().unwrap();
    let session = opened(&store);
    noted(&store, &session, "Width 512 launched");
    super::super::tests::run(&store, "r1", "Width 512", "chat", "step 1\n");
    store.conn_execute_for_tests("UPDATE runs SET created_at = 'unreadable'");
    let host = host();

    settle(&host).await;

    assert_eq!(super::super::tests::calls(&dir), 0);
    let report = store.chat_report("chat").unwrap().unwrap();
    assert!(report.error.is_none(), "orx's own error is not the user's");
    assert!(report.due_at.is_some() && !report.notes.is_empty());
    assert!(held().lock().unwrap().contains_key("chat"));

    store.conn_execute_for_tests("UPDATE runs SET created_at = 1");
    held().lock().unwrap().get_mut("chat").unwrap().0 = 0;
    settle(&host).await;
    assert_eq!(
        store.chat_report("chat").unwrap().unwrap().body.as_deref(),
        Some("Width 512 is training. <run id=\"r1\"/>")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_update_cut_short_by_a_restart_runs_again() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &["First.", "Width 512 is training."]);
    let (ready, _) = paused(&dir);
    let store = Store::open().unwrap();
    let session = opened(&store);
    noted(&store, &session, "Width 512 launched");
    let watching = watching_model(&store, &session).unwrap();

    let cut = async {
        tokio::select! {
            _ = write("chat", &watching) => panic!("the update was to be cut short"),
            _ = crate::process::tokio_command("cat").arg(&ready).output() => {}
        }
    };
    // Killed with the update, the call never reads `resume`.
    cut.await;
    let report = store.chat_report("chat").unwrap().unwrap();
    assert!(report.body.is_none() && report.due_at.is_some());
    assert_eq!(notes(&report), ["Width 512 launched"]);

    settle(&host()).await;

    assert_eq!(super::super::tests::calls(&dir), 2);
    assert!(prompt(&dir, 2).contains("Width 512 launched"));
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some("Width 512 is training."));
}

#[cfg(unix)]
#[tokio::test]
async fn an_update_that_cannot_be_kept_waits_before_asking_the_model_again() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &["Width 512 is training.", "Width 512 is training."]);
    let store = Store::open().unwrap();
    let session = opened(&store);
    noted(&store, &session, "Width 512 launched");
    store.conn_execute_for_tests(
        "CREATE TRIGGER full BEFORE INSERT ON chat_report_versions
         BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END",
    );
    let host = host();

    settle(&host).await;
    settle(&host).await;

    assert_eq!(super::super::tests::calls(&dir), 1, "held inside the gap");
    assert!(store.chat_report("chat").unwrap().unwrap().due_at.is_some());
    assert_eq!(report_json(&store, "chat").unwrap()["writing"], false);

    store.conn_execute_for_tests("DROP TRIGGER full");
    held().lock().unwrap().get_mut("chat").unwrap().0 = 0;
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 2);
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some("Width 512 is training."));
    assert!(held().lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn a_number_the_chat_never_showed_gets_one_correction_then_the_report_stands() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let invented = "Width 512 leads by 12.5% at 0.341.";
    super::super::tests::fake_claude(
        &dir,
        &[
            invented,
            REPORT,
            &edit(FIRST_LINE, "# Status\nWidth 512 reached 0.29."),
            &edit(FIRST_LINE, "# Status\nStill 0.29."),
        ],
    );
    let store = Store::open().unwrap();
    let session = opened(&store);
    say(
        &store,
        "chat",
        "m1",
        "assistant",
        "Width 512 reached val loss 0.341; the baseline was 0.388.",
        10,
    );
    let host = host();

    noted(&store, &session, "Width 512 won");
    settle(&host).await;

    assert_eq!(super::super::tests::calls(&dir), 2);
    let correction = prompt(&dir, 2);
    assert!(
        correction.contains("12.5%") && correction.contains(invented),
        "{correction}"
    );
    assert!(
        !correction.contains("0.341,"),
        "a number the chat showed is not named"
    );
    assert_eq!(
        store.chat_report("chat").unwrap().unwrap().body.as_deref(),
        Some(REPORT)
    );

    // Two replies that fail leave the report as it was and say why.
    age(&store, "chat");
    noted(&store, &session, "Depth 8 failed");
    settle(&host).await;

    assert_eq!(super::super::tests::calls(&dir), 4);
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some(REPORT));
    let error = report.error.clone().unwrap();
    assert!(
        error.contains("0.29") && error.contains("heading"),
        "{error}"
    );
    assert!(
        report.due_at.is_none(),
        "a failed update waits for the next milestone"
    );
    assert_eq!(
        notes(&report),
        ["Depth 8 failed"],
        "its notes are kept for the next one"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn each_watching_state_keeps_or_refuses_milestones_and_says_why() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &[]);
    let bin = crate::config::home_dir().unwrap().join(".local/bin");
    orx_test_support::write_executable(bin.join("codex"), "#!/bin/sh\nexit 1\n", 0o755).unwrap();
    let store = Store::open().unwrap();
    let host = host();
    let claude = chat(&store, "claude", "claude-code");
    let opencode = chat(&store, "opencode", "opencode");
    let codex = chat(&store, "codex", "codex");
    chat(&store, "sonnet", "opencode");
    store
        .set_chat_session_model("sonnet", "anthropic/claude-sonnet-4-5")
        .unwrap();

    store.set_run_watching(false).unwrap();
    let off = skipped(&store, &claude);
    assert!(off.contains("Turn it on in Settings"), "{off}");
    after_turn(&store, "claude").unwrap();
    assert!(store.chat_report("claude").unwrap().is_none());
    store.set_run_watching(true).unwrap();

    // Kept for when the user picks a model.
    noted(&store, &opencode, "x");
    after_turn(&store, "opencode").unwrap();
    assert_eq!(report_json(&store, "opencode").unwrap(), Value::Null);

    for id in ["claude", "codex", "sonnet"] {
        store.set_chat_report_enabled(id, true, 1).unwrap();
    }
    let revision = sign_in_revision("claude-code", None).unwrap();
    store
        .record_watch_auth_refusal("claude-code", None, &revision)
        .unwrap();
    noted(&store, &claude, "Width 512 won");
    noted(&store, &codex, "Width 512 won");
    settle(&host).await;

    assert_eq!(super::super::tests::calls(&dir), 0);
    let signed_out = store.chat_report("claude").unwrap().unwrap();
    assert!(signed_out.error.unwrap().contains("Sign in to Claude Code"));
    assert!(
        signed_out.due_at.is_some(),
        "it updates once the user signs in"
    );
    let unavailable = store.chat_report("codex").unwrap().unwrap();
    assert!(
        unavailable.error.unwrap().contains("Codex"),
        "the cause is named"
    );
    let reading = store.chat_report("sonnet").unwrap().unwrap();
    assert!(
        reading.error.is_none() && reading.due_at.is_some(),
        "a chat waiting for orx to read its agents' models waits without an error"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_refused_sign_in_waits_for_the_user_and_a_retry_inside_the_gap_is_deferred() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &["API Error: 401 Unauthorized", REPORT]);
    let store = Store::open().unwrap();
    let session = opened(&store);
    say(
        &store,
        "chat",
        "m1",
        "assistant",
        "Width 512 reached val loss 0.341; the baseline was 0.388.",
        10,
    );
    let host = host();

    noted(&store, &session, "Width 512 won");
    settle(&host).await;
    let report = store.chat_report("chat").unwrap().unwrap();
    assert!(report.error.unwrap().contains("Sign in"));
    assert!(report.due_at.is_some());
    assert!(matches!(
        watching_model(&store, &session).unwrap(),
        Watching::NeedsSignIn(_)
    ));

    store.conn_execute_for_tests("DELETE FROM watch_auth_refusals");
    let retried = retry(&host, &store, &session).unwrap().unwrap();
    assert_eq!(
        retried["error"],
        Value::Null,
        "Retry drops the error at once"
    );
    settle(&host).await;
    assert_eq!(
        super::super::tests::calls(&dir),
        1,
        "Retry waits out the gap"
    );

    age(&store, "chat");
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 2);
    assert_eq!(
        store.chat_report("chat").unwrap().unwrap().body.as_deref(),
        Some(REPORT)
    );
}

#[test]
fn delegates_side_chats_and_deleted_chats_keep_no_report() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let store = Store::open().unwrap();
    let parent = chat(&store, "parent", "claude-code");
    let delegate = chat(&store, "delegate", "claude-code");
    store
        .record_chat_delegate("delegate", "parent", false)
        .unwrap();
    store.put_chat_session("side", "claude-code");
    store.conn_execute_for_tests(
        "UPDATE chat_sessions SET side_parent_session_id = 'parent' WHERE id = 'side'",
    );
    let side = store.get_chat_session("side").unwrap().unwrap();

    for session in [&delegate, &side] {
        assert!(skipped(&store, session).contains("Delegates and side chats"));
        after_turn(&store, &session.id).unwrap();
        assert!(store.chat_report(&session.id).unwrap().is_none());
    }

    noted(&store, &parent, "Width 512 won");
    let taken = ReportTaken {
        read_at: 7,
        last_note: None,
        due_at: None,
        turn_due_at: None,
        unread_version: None,
    };
    for version in ["First.", "Second."] {
        let written = ReportOutcome::Written(version);
        store
            .finish_chat_report("parent", written, taken, 7)
            .unwrap();
    }
    let kept: Vec<String> = store
        .chat_report_versions("parent")
        .unwrap()
        .into_iter()
        .map(|(_, body)| body)
        .collect();
    assert_eq!(kept, ["First.", "Second."], "every version is kept");
    assert_eq!(
        store
            .chat_report("parent")
            .unwrap()
            .unwrap()
            .body
            .as_deref(),
        Some("Second."),
        "the report is its newest version"
    );
    store.delete_chat_session("side").unwrap();
    store.delete_chat_session("parent").unwrap();
    assert!(store.chat_report("parent").unwrap().is_none());
    assert!(store.chat_report_versions("parent").unwrap().is_empty());
    let written = ReportOutcome::Written(REPORT);
    store
        .finish_chat_report("parent", written, taken, 7)
        .unwrap();
    assert!(
        store.chat_report("parent").unwrap().is_none(),
        "an update that ends after its chat was deleted writes nothing"
    );
    assert!(store.chat_report_versions("parent").unwrap().is_empty());
}

fn texts(text: &str) -> Vec<String> {
    numbers(text)
        .into_iter()
        .filter(|number| !number.exempt)
        .map(|number| number.text)
        .collect()
}

#[test]
fn numbers_are_decimals_percents_signed_and_scientific_values_but_not_names_or_small_counts() {
    assert_eq!(
        texts("Val loss 0.341 (−0.02) at 12.5% after 1,200 steps, lr 3e-4, +7.1 points, 120ms."),
        ["0.341", "−0.02", "12.5%", "1,200", "3e-4", "+7.1", "120"]
    );
    assert_eq!(
        texts("3 runs, 2 seeds and run r12 on exp-v2.1, commit a1b2c3d, ran 1. fine"),
        Vec::<String>::new(),
        "small counts and the digits of a name need no source"
    );
    assert_eq!(
        texts("1. First\n12. Twelfth\n  14) Fourteenth"),
        Vec::<String>::new()
    );
    assert_eq!(
        texts("Depth 12 and 1,2 and 2026-11-23 on exp-12"),
        ["12", "2026"]
    );
}

#[test]
fn a_number_matches_the_inputs_rounded_as_a_percent_or_without_its_sign() {
    let allowed = Known::of(&json!({
        "conversation": "val loss 0.34127, accuracy 0.912, delta -0.02, 41.5% done",
        "runs": [{"steps": 1234}],
    }));
    let unseen = |text: &str| -> Vec<String> {
        numbers(text)
            .into_iter()
            .filter(|number| {
                !number.exempt && !allowed.numbers.iter().any(|known| number.matches(known))
            })
            .map(|number| number.text)
            .collect()
    };
    assert!(unseen(
        "Loss 0.341, 0.34 and 0.3413; accuracy 91.2%; 0.415 done; 1,234 steps; fell 0.02; −0.02"
    )
    .is_empty());
    assert_eq!(
        unseen("Loss 0.35, 0.342; accuracy 92%; +0.02; 1,200 steps; 0.5e-1"),
        ["0.35", "0.342", "92%", "+0.02", "1,200", "0.5e-1"]
    );
    assert_eq!(
        checked("## Status\nLoss 0.35.", &allowed),
        [
            "its first line is a heading, not one plain sentence",
            "it cites numbers that appear nowhere in the chat: 0.35"
        ]
    );
    assert_eq!(checked("  \n", &allowed), ["it was empty"]);
    assert!(checked(
        "Loss **0.341** after 3 runs.\n\n## Next\n1. Depth 8",
        &allowed
    )
    .is_empty());
}

#[test]
fn citations_must_name_runs_files_and_images_the_chat_shows_and_a_finished_chart_is_shown() {
    let run = "run_5f1c2b7e-3251-4a9b-8e2d-90c4f1a6b3d2";
    let known = Known::of(&json!({
        "report": "Width 512 won. <run id=\"run_old\" label=\"0.341\"/>",
        "conversation": format!("Loss reached 0.341 after editing src/train_v3.py and moving back to fe3c5c0. <run id=\"{run}\" figure/> <file path=\"Makefile\"/>\n![Width sweep](artifacts/sweep 2.png)"),
        "notices": [{"run": "run_notice", "text": "Run finished."}],
        "runs": [
            {"run": run, "experiment": "Width 512", "status": "done", "measures": {"Val loss": "0.341"}},
            {"run": "run_done", "experiment": "Depth 8", "status": "done", "figure": true},
            {"run": "run_live", "experiment": "Depth 12", "status": "running"},
        ],
    }));
    let cited = format!(
        "Depth 8 is training after width 512 won.\n\n## Findings\n- Width 512 reached 0.341. <run id=\"{run}\" label=\"Val loss 0.341\"/>\n- Kept from before. <run id=\"run_old\" label=\"0.341\"/>\n- Depth 8 finished. <run id=\"run_notice\"/> <file path=\"src/train_v3.py\" lines=\"20-40\"/> <file path=\"Makefile\"/>\n- No gain from depth 8 or width 512. <run id=\"run_notice\"/> <run id=\"{run}\"/>\n\n<run id=\"run_done\" figure/>\n[log](https://example.com/run/9137)"
    );
    assert!(
        checked(&cited, &known).is_empty(),
        "{:?}",
        checked(&cited, &known)
    );
    let imaged = "Depth 8 finished.\n\n![Width sweep](<artifacts/sweep 2.png> \"Sweep\")";
    let unchartable = Known::of(&json!({"conversation": "![Width sweep](artifacts/sweep 2.png)"}));
    assert!(
        checked(imaged, &unchartable).is_empty(),
        "an image the agent showed may be the figure"
    );
    assert_eq!(
        checked(&format!("{imaged}\n<run id=\"run_done\" figure/>"), &known),
        ["it shows more than one figure; show one figure: the run whose comparison with its parent best shows the current breakthrough"],
        "an image still counts toward the one figure"
    );
    assert!(
        checked(imaged, &known).is_empty(),
        "an image the agent showed may be the figure while a run has a chart"
    );

    let invented = "Depth 8 is training.\n- Width 512 reached 0.341. <run id=\"run_gone\" label=\"Val loss 0.29\"/>\n- The branch moved back. <file path=\"fe3c5c0\"/>\n![Loss](artifacts/loss.png)";
    assert_eq!(
        checked(invented, &known),
        [
            "its run tags give numbers that are not those runs' own measures: 0.29 for run_gone. Label each with one of that run's own measures, or in the Ruled out line with the change it made, or write the number as plain prose, keeping the line",
            "it cites numbers that appear nowhere in the chat: 0.29",
            "it cites runs that appear nowhere in the chat: run_gone",
            "it cites images that appear nowhere in the chat: artifacts/loss.png",
            "it cites files that appear nowhere in the chat: fe3c5c0",
        ]
    );
    assert_eq!(
        checked(
            "Depth 8 finished. <run id=\"run_done\" label=\"done\"/>",
            &known
        ),
        ["it shows no figure, though these finished runs have one: run_done"]
    );
    assert_eq!(
        checked("Depth 12 is training.\n\n<run id=\"run_live\" figure/>", &known),
        ["it shows run_live as the figure, but only a finished run with a chart can be one: run_done"]
    );
    let running = Known::of(&json!({"runs": [{"run": "run_live", "status": "running"}]}));
    assert_eq!(
        checked("Depth is training.\n\n<run id=\"run_live\" figure/>", &running),
        ["it shows run_live as the figure, but no run has a finished chart; show an image the agent showed, or no figure"]
    );
}

#[test]
fn a_run_tags_label_gives_that_runs_own_numbers() {
    let known = Known::of(&json!({
        "report": "Cosine won. <run id=\"seed0\" label=\"seed 0 val loss 0.01062\"/>",
        "notices": [{"run": "seed1", "text": "Run seed1 finished: val loss 0.01064."}],
        "runs": [
            {"run": "seed0", "experiment": "Cosine decay", "status": "done", "measures": {"Val loss": "0.01062"}},
            {"run": "seed1", "experiment": "Cosine seed 1", "status": "done"},
            {"run": "old", "experiment": "Warmup", "status": "done", "figure": true, "measures": {"Val loss": "0.0136"}},
        ],
    }));
    let figure = "\n\n<run id=\"old\" figure/>";
    let own = format!("Cosine holds across seeds.\n- Seed 0. <run id=\"seed0\" label=\"seed 0 val loss 0.01062\"/>\n- Seed 1. <run id=\"seed1\" label=\"val loss 0.01064\"/>{figure}");
    assert!(
        checked(&own, &known).is_empty(),
        "{:?}",
        checked(&own, &known)
    );
    let crossed = format!("Cosine holds across seeds.\n- Seed 1. <run id=\"seed0\" label=\"seed 1 val loss 0.01064\"/>{figure}");
    assert_eq!(
        checked(&crossed, &known),
        ["its run tags give numbers that are not those runs' own measures: 0.01064 for seed0. Label each with one of that run's own measures, or in the Ruled out line with the change it made, or write the number as plain prose, keeping the line"]
    );
    let ruled_out = format!("Cosine holds across seeds.\n\n## Ruled out\n**No gain** from <run id=\"seed1\" label=\"cosine on seed 1\"/>.{figure}");
    assert!(
        checked(&ruled_out, &known).is_empty(),
        "a Ruled out tag is labeled with the change it made"
    );
    let watched = Known::of(&json!({"runs": [{
        "run": "seed0", "parent": "Cosine, peak LR 0.05", "status": "done",
        "measures": {"Val loss": "0.34"}, "watcher": "Val loss 0.34, 0.02 better than its parent.",
    }]}));
    assert!(
        checked("Cosine won. <run id=\"seed0\" label=\"0.02\"/>", &watched)[0]
            .contains("0.02 for seed0"),
        "a difference the watcher gave is not the run's result"
    );
    let relabelled = format!("Cosine holds across seeds.\n- Warmup was worse. <run id=\"old\" label=\"0.0136\"/>{figure}");
    assert!(
        checked(&relabelled, &known).is_empty(),
        "a cited run takes a new label from its own measures"
    );
    assert_eq!(
        checked("Cosine holds across seeds.", &known),
        ["it shows no figure, though these finished runs have one: old"],
        "a run the report cites can be the figure"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_update_lists_the_runs_the_previous_report_cites_without_their_results() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &["UNCHANGED"]);
    let store = Store::open().unwrap();
    let session = opened(&store);
    super::super::tests::run(&store, "base", "Baseline", "chat", "step 1\n");
    super::super::tests::run(&store, "old", "Warmup", "chat", "step 1\n");
    store.conn_execute_for_tests(
        "UPDATE runs SET status = 'done', updated_at = 1, result_markdown = 'loss 0.0136' WHERE id = 'old'",
    );
    store.conn_execute_for_tests(
        "UPDATE local_experiments SET parent_experiment_id = 'exp-base' WHERE id = 'exp-old'",
    );
    store.chat_report_due("chat", None, 1).unwrap();
    let previous = "Warmup was ruled out. <run id=\"old\" label=\"done\"/>";
    let taken = store.chat_report("chat").unwrap().unwrap();
    let taken = ReportTaken {
        read_at: 2,
        last_note: None,
        due_at: taken.due_at,
        turn_due_at: None,
        unread_version: None,
    };
    store
        .finish_chat_report("chat", ReportOutcome::Written(previous), taken, 2)
        .unwrap();
    noted(&store, &session, "Depth 8 next");

    settle(&host()).await;

    let sent = prompt(&dir, 1);
    assert!(
        sent.contains(r#"{"run":"old","experiment":"Warmup","parent":"Baseline","status":"done"}"#),
        "{sent}"
    );
    assert!(
        !sent.contains("loss 0.0136"),
        "a cited run's results stay out: {sent}"
    );
    assert!(
        sent.contains(r#""run":"base","experiment":"Baseline","parent":null"#),
        "a root has no parent: {sent}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_first_report_may_cite_its_runs_only_in_a_ruled_out_line() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let ruled_out = "The sweep found nothing yet.\n\n## Ruled out\n- No gain from **AdamW decay** or **EMA**. <run id=\"a\"/> <run id=\"b\"/>";
    super::super::tests::fake_claude(&dir, &[ruled_out]);
    let store = Store::open().unwrap();
    opened(&store);
    super::super::tests::run(&store, "a", "AdamW decay", "chat", "step 1\n");
    super::super::tests::run(&store, "b", "EMA", "chat", "step 1\n");

    settle(&host()).await;

    assert_eq!(
        super::super::tests::calls(&dir),
        1,
        "label-less tags cite their runs"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some(ruled_out));
}

#[cfg(unix)]
#[tokio::test]
async fn a_label_correction_keeps_the_previous_reports_findings() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let previous = "Cosine is training.\n\n## Findings\n- Cosine beat the baseline. <run id=\"old\" label=\"0.0106\"/>";
    let line = "- Cosine beat the baseline. <run id=\"old\" label=\"0.0106\"/>";
    let relabelled = edit(line, &line.replace("0.0106", "0.0109"));
    let fixed = edit(line, line);
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &[&relabelled, &fixed]);
    let store = Store::open().unwrap();
    let session = opened(&store);
    super::super::tests::run(&store, "old", "Cosine", "chat", "step 1\n");
    store
        .conn_execute_for_tests("UPDATE runs SET status = 'done', updated_at = 1 WHERE id = 'old'");
    store.chat_report_due("chat", None, 1).unwrap();
    let due = store.chat_report("chat").unwrap().unwrap().due_at;
    let taken = ReportTaken {
        read_at: 2,
        last_note: None,
        due_at: due,
        turn_due_at: None,
        unread_version: None,
    };
    store
        .finish_chat_report("chat", ReportOutcome::Written(previous), taken, 2)
        .unwrap();
    noted(&store, &session, "Cosine finished");

    settle(&host()).await;

    assert_eq!(super::super::tests::calls(&dir), 2);
    let correction = prompt(&dir, 2);
    assert!(
        correction.contains("0.0109 for old") && correction.contains("keeping the line"),
        "{correction}"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(
        report.body.as_deref(),
        Some(previous),
        "the finding is kept"
    );
    assert!(report.error.is_none());
}

/// A run of `chat` that ended at `ended`.
fn ended_run(store: &Store, chat: &str, run: &str, ended: i64) {
    super::super::tests::run(store, run, "Width 512", chat, "step 1\n");
    store.conn_execute_for_tests(&format!(
        "UPDATE runs SET status = 'done', ended_at = {ended}, updated_at = {ended} WHERE id = '{run}'"
    ));
}

/// A finished turn of `chat` that ran from `start` to `end`.
fn worked(store: &Store, chat: &str, turn: &str, start: i64, end: i64) {
    store.put_chat_turn(chat, turn, "completed", start);
    store.conn_execute_for_tests(&format!(
        "UPDATE chat_turns SET updated_at = {end} WHERE id = '{turn}'"
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn a_chat_that_works_a_minute_past_a_runs_end_opens_its_report_and_a_quick_reaction_does_not()
{
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(
        &dir,
        &[
            "Width 512 finished. <run id=\"shared\"/>",
            "Width 512 finished. <run id=\"shared\"/>",
            "Width 512 finished. <run id=\"shared\"/>",
        ],
    );
    let store = Store::open().unwrap();
    let host = host();
    let minute = paced_ms(OPEN_AFTER_S);
    let now = crate::store::now_ms();
    // A run every chat follows, still going, gives each report something to cite.
    chat(&store, "owner", "claude-code");
    super::super::tests::run(&store, "shared", "Width 512", "owner", "step 1\n");
    for id in ["idle", "quick", "long", "ended", "relaunch"] {
        chat(&store, id, "claude-code");
        store.subscribe_run("shared", id, 1).unwrap();
    }

    // No work after the run's end, and work before it, open nothing.
    ended_run(&store, "idle", "r-idle", now - 10 * minute);
    worked(
        &store,
        "idle",
        "t-idle",
        now - 20 * minute,
        now - 11 * minute,
    );
    // A reaction that stops within the minute opens nothing.
    ended_run(&store, "quick", "r-quick", now - 10 * minute);
    worked(
        &store,
        "quick",
        "t-quick",
        now - 10 * minute + 1000,
        now - 10 * minute + minute / 2,
    );
    // A turn that ended a minute past the run's end opens it.
    ended_run(&store, "ended", "r-ended", now - 10 * minute);
    worked(
        &store,
        "ended",
        "t-ended",
        now - 11 * minute,
        now - 10 * minute + minute + 1000,
    );
    // A launch after a run ended opens it, however short the turns around it.
    ended_run(&store, "relaunch", "r-first", now - 10 * minute);
    super::super::tests::run(&store, "r-second", "Width 1024", "relaunch", "step 1\n");
    // A turn still going opens it at the minute, before it ends.
    ended_run(&store, "long", "r-long", now - 2 * minute);
    store.put_chat_turn("long", "t-long", "running", now - minute - 1000);
    store.put_chat_turn("long", "t-long-early", "completed", now - 3 * minute);

    let mut events = host.subscribe();
    settle(&host).await;
    let mut told = HashSet::new();
    while let Ok((name, data)) = events.try_recv() {
        if name == "chat.report" && !data["report"].is_null() {
            told.insert(data["sessionId"].as_str().unwrap().to_owned());
        }
    }

    for (id, opened) in [
        ("relaunch", true),
        ("idle", false),
        ("quick", false),
        ("ended", true),
        ("long", true),
    ] {
        let enabled = store
            .chat_report(id)
            .unwrap()
            .and_then(|report| report.enabled);
        assert_eq!(enabled, opened.then_some(true), "{id}");
        assert_eq!(told.contains(id), opened, "the page hears that {id} opened");
    }
    for id in ["relaunch", "ended"] {
        assert!(
            store.chat_report(id).unwrap().unwrap().body.is_some(),
            "{id}"
        );
    }
    assert!(
        store.chat_report("long").unwrap().unwrap().body.is_none(),
        "the first write waits for the reaction to end"
    );
    store.complete_chat_turn("t-long").unwrap();
    settle(&host).await;
    assert!(store.chat_report("long").unwrap().unwrap().body.is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn a_milestone_before_any_result_waits_and_a_relaunch_writes_once_its_turn_ends() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(
        &dir,
        &["Round 2 of the width sweep is running. <run id=\"round-2\"/>"],
    );
    let store = Store::open().unwrap();
    let host = host();
    let session = chat(&store, "chat", "claude-code");
    super::super::tests::run(&store, "round-1", "Width sweep", "chat", "step 1\n");

    noted(&store, &session, "Round 1/10 launched");
    settle(&host).await;
    assert_eq!(
        report_json(&store, "chat").unwrap(),
        Value::Null,
        "a launch alone opens nothing"
    );
    assert_eq!(super::super::tests::calls(&dir), 0);

    let now = crate::store::now_ms();
    store.conn_execute_for_tests(&format!(
        "UPDATE runs SET status = 'done', ended_at = {} WHERE id = 'round-1'",
        now - 30_000
    ));
    store.put_chat_turn("chat", "reaction", "running", now - 20_000);
    super::super::tests::run(&store, "round-2", "Width sweep", "chat", "step 1\n");
    store.conn_execute_for_tests(&format!(
        "UPDATE runs SET created_at = {} WHERE id = 'round-2'",
        now - 10_000
    ));
    settle(&host).await;
    assert_eq!(report_json(&store, "chat").unwrap()["enabled"], true);
    assert_eq!(
        super::super::tests::calls(&dir),
        0,
        "it waits for the reaction to end"
    );

    store.complete_chat_turn("reaction").unwrap();
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 1);
    assert!(
        prompt(&dir, 1).contains("Round 1/10 launched"),
        "the early note reaches it"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert!(report.body.is_some() && report.notes.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn the_user_turns_a_chats_report_off_and_on() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &["The width sweep is training."]);
    let store = Store::open().unwrap();
    let host = host();
    let session = chat(&store, "chat", "claude-code");
    let opencode = chat(&store, "opencode", "opencode");
    assert_eq!(
        report_json(&store, "chat").unwrap(),
        Value::Null,
        "closed until it opens"
    );

    let off = set_enabled(&host, &store, &session, false)
        .unwrap()
        .unwrap();
    assert_eq!(off["enabled"], false);
    assert!(skipped(&store, &session).contains("turned the status report off"));
    after_turn(&store, "chat").unwrap();
    assert!(retry(&host, &store, &session)
        .unwrap()
        .unwrap_err()
        .contains("off for this chat"));
    settle(&host).await;
    assert_eq!(
        super::super::tests::calls(&dir),
        0,
        "an off report writes nothing"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert!(report.notes.is_empty() && report.due_at.is_none() && report.turn_due_at.is_none());

    store.set_run_watching(false).unwrap();
    let refused = set_enabled(&host, &store, &session, true).unwrap();
    assert!(refused.unwrap_err().contains("Run watching is off"));
    let off = set_enabled(&host, &store, &session, false).unwrap();
    assert_eq!(off.unwrap()["enabled"], false, "off needs no run watching");
    store.set_run_watching(true).unwrap();

    let on = set_enabled(&host, &store, &session, true).unwrap().unwrap();
    assert_eq!(on["enabled"], true);
    settle(&host).await;
    assert_eq!(
        super::super::tests::calls(&dir),
        1,
        "on again, it is due at once"
    );

    set_enabled(&host, &store, &session, false)
        .unwrap()
        .unwrap();
    let kept = report_json(&store, "chat").unwrap();
    assert_eq!(
        (kept["enabled"].clone(), kept["body"].clone()),
        (json!(false), json!("The width sweep is training."))
    );
    let offered = set_enabled(&host, &store, &opencode, true)
        .unwrap()
        .unwrap();
    assert_eq!(offered["needsModel"], json!({"same": false}));
    let told = |events: &mut tokio::sync::broadcast::Receiver<(&'static str, Value)>| {
        std::iter::from_fn(|| events.try_recv().ok())
            .filter(|(name, data)| *name == "chat.report" && data["sessionId"] == "opencode")
            .count()
    };
    let mut events = host.subscribe();
    settle(&host).await;
    assert_eq!(
        told(&mut events),
        1,
        "the page hears once that it needs a pick"
    );
    settle(&host).await;
    assert_eq!(told(&mut events), 0);
    assert!(store
        .chat_report("opencode")
        .unwrap()
        .unwrap()
        .error
        .is_none());
}

const PREVIOUS: &str = "Cosine is training.\n\n## Findings\n- Cosine beat the baseline.\n- Warmup did not help.\n\n## Next\n- Try depth 8.";

#[test]
fn edits_apply_in_order_all_or_nothing() {
    let applied = |reply: &str| edited(PREVIOUS, reply);
    assert_eq!(
        applied(&edit("Cosine is training.", "Depth 8 is training.")),
        Ok(Some(
            PREVIOUS.replace("Cosine is training.", "Depth 8 is training.")
        ))
    );
    let several = format!(
        "Here are the edits.\n{}\n\n{}\nThat is all.",
        edit("- Try depth 8.", "- Try depth 12."),
        edit("- Try depth 12.", "- Try depth 16."),
    );
    assert_eq!(
        applied(&several),
        Ok(Some(PREVIOUS.replace("depth 8", "depth 16"))),
        "each edit applies to the last one's result, and text outside them is ignored"
    );
    assert_eq!(
        applied(&edit("- Warmup did not help.", "")),
        Ok(Some(PREVIOUS.replace("- Warmup did not help.\n", ""))),
        "an empty REPLACE deletes the line and its newline"
    );
    let added = "- Warmup did not help.\n- Depth 8 beat depth 4.";
    assert_eq!(
        applied(&edit("- Warmup did not help.", added)),
        Ok(Some(PREVIOUS.replace("- Warmup did not help.", added)))
    );
    let loose = "<<<<<<<  SEARCH\n- Try depth 8.\n======\n- Try depth 12.\n>>>>>> REPLACE";
    assert_eq!(
        applied(loose),
        Ok(Some(PREVIOUS.replace("depth 8", "depth 12"))),
        "markers of six characters apply"
    );
    assert_eq!(applied("UNCHANGED\n"), Ok(None));

    for (reply, why) in [
        (edit("Cosine", "x"), "found more than once"),
        (edit("- Try depth 4.", "- x"), "not found"),
        (edit("", "- x"), "not found"),
        (
            "<<<<<<< SEARCH\n- Try depth 8.\n=======\n- x".into(),
            "not closed",
        ),
        ("I'm not going to produce that report.".into(), "no edits"),
        (
            format!(
                "{}\n{}",
                edit("- Try depth 8.", "- x"),
                edit("- Try depth 4.", "- y")
            ),
            "not found",
        ),
    ] {
        let failed = applied(&reply).unwrap_err();
        assert!(failed.contains(why), "{reply}: {failed}");
    }
}

/// A chat whose report already says PREVIOUS, due for its next update.
fn previously(store: &Store) -> StoredChatSession {
    let session = opened(store);
    say(
        store,
        "chat",
        "m1",
        "assistant",
        "Depth 8 beat depth 4.",
        10,
    );
    store.chat_report_due("chat", None, 1).unwrap();
    let taken = ReportTaken {
        read_at: 2,
        last_note: None,
        due_at: Some(1),
        turn_due_at: None,
        unread_version: None,
    };
    store
        .finish_chat_report("chat", ReportOutcome::Written(PREVIOUS), taken, 2)
        .unwrap();
    store.chat_report_due("chat", None, 3).unwrap();
    session
}

#[cfg(unix)]
#[tokio::test]
async fn unchanged_confirms_the_report_without_a_version() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    super::super::tests::fake_claude(&dir, &["UNCHANGED"]);
    let store = Store::open().unwrap();
    previously(&store);

    settle(&host()).await;

    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some(PREVIOUS));
    assert!(
        report.updated_at.unwrap() > 2,
        "the time says when it was confirmed"
    );
    assert!(report.due_at.is_none());
    assert_eq!(store.chat_report_versions("chat").unwrap().len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn a_report_marked_read_is_written_again_with_only_what_changed_since() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let since = "Depth 8 is training.";
    super::super::tests::fake_claude(&dir, &[since, "UNCHANGED"]);
    let store = Store::open().unwrap();
    previously(&store);
    let version = store.chat_report("chat").unwrap().unwrap().version.unwrap();
    assert!(!store.mark_chat_report_read("chat", version + 1).unwrap());
    assert!(store.mark_chat_report_read("chat", version).unwrap());
    assert!(store.chat_report("chat").unwrap().unwrap().read);
    let host = host();

    settle(&host).await;
    let given: Value =
        serde_json::from_str(prompt(&dir, 1).strip_prefix(PREAMBLE).unwrap()).unwrap();
    assert_eq!(given["read"], PREVIOUS);
    assert!(given["report"].is_null());
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(
        report.body.as_deref(),
        Some(since),
        "written whole, not edited"
    );
    assert!(!report.read, "a new version is unread");

    store
        .mark_chat_report_read("chat", report.version.unwrap())
        .unwrap();
    store
        .chat_report_due("chat", None, crate::store::now_ms())
        .unwrap();
    age(&store, "chat");
    settle(&host).await;
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some(since));
    assert!(report.read, "nothing new keeps it read");
    assert_eq!(store.chat_report_versions("chat").unwrap().len(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn marking_a_report_read_mid_update_rebuilds_from_the_read_report() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let stale_edit = edit("- Try depth 8.", "- Depth 8 beat depth 4.");
    let since = "Depth 8 beat depth 4.";
    super::super::tests::fake_claude(&dir, &[&stale_edit, since]);
    let store = Store::open().unwrap();
    previously(&store);
    let version = store.chat_report("chat").unwrap().unwrap().version.unwrap();

    while_writing(&store, &dir, || {
        assert!(store.mark_chat_report_read("chat", version).unwrap());
    })
    .await;

    assert_eq!(super::super::tests::calls(&dir), 2);
    let first: Value =
        serde_json::from_str(prompt(&dir, 1).strip_prefix(PREAMBLE).unwrap()).unwrap();
    assert_eq!(first["report"], PREVIOUS);
    assert!(first["read"].is_null());
    let rebuilt: Value =
        serde_json::from_str(prompt(&dir, 2).strip_prefix(PREAMBLE).unwrap()).unwrap();
    assert_eq!(rebuilt["read"], PREVIOUS);
    assert!(rebuilt["report"].is_null());
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(
        report.body.as_deref(),
        Some(since),
        "stale edit is discarded in favour of the read-mode rewrite"
    );
    assert!(!report.read, "the newly written read-mode report is unread");
    assert_eq!(store.chat_report_versions("chat").unwrap().len(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn edits_that_cannot_apply_are_asked_for_again_once() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let added = "- Warmup did not help.\n- Depth 8 beat depth 4.";
    let good = edit("- Warmup did not help.", added);
    super::super::tests::fake_claude(
        &dir,
        &[
            "I'm not going to produce that report.",
            &good,
            &edit("- Try depth 4.", "- x"),
            "<<<<<<< SEARCH\n- Try depth 12.",
        ],
    );
    let store = Store::open().unwrap();
    previously(&store);
    let host = host();

    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 2);
    let again = prompt(&dir, 2);
    assert!(
        again.starts_with(&prompt(&dir, 1))
            && again.contains("the reply held no edits")
            && again.contains("SEARCH lines match the previous report exactly"),
        "the same request with the reason: {again}"
    );
    let applied = PREVIOUS.replace("- Warmup did not help.", added);
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some(applied.as_str()));

    age(&store, "chat");
    store
        .chat_report_due("chat", None, crate::store::now_ms())
        .unwrap();
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 4);
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(
        report.body.as_deref(),
        Some(applied.as_str()),
        "two failures keep the report"
    );
    assert!(report.error.unwrap().contains("could not be applied"));
    assert_eq!(store.chat_report_versions("chat").unwrap().len(), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn edits_that_fail_the_checks_are_corrected_as_edits() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let invented = edit(
        "- Warmup did not help.",
        "- Warmup did not help.\n- Depth 8 won by 12.5%.",
    );
    let fixed = edit(
        "- Warmup did not help.",
        "- Warmup did not help.\n- Depth 8 beat depth 4.",
    );
    super::super::tests::fake_claude(&dir, &[&invented, &fixed]);
    let store = Store::open().unwrap();
    previously(&store);

    settle(&host()).await;

    let correction = prompt(&dir, 2);
    assert!(
        correction.contains("12.5%")
            && correction.contains("reply with edits to the same previous report"),
        "{correction}"
    );
    assert!(
        correction.contains("\"report\":\"Cosine is training."),
        "the same previous report"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert!(report
        .body
        .unwrap()
        .ends_with("- Depth 8 beat depth 4.\n\n## Next\n- Try depth 8."));
}

#[test]
fn a_report_condenses_once_it_would_fill_one_and_a_half_cards() {
    let line = "- Cosine decay beat the baseline on both seeds."; // one row
    let report = |lines: usize, figures: usize| {
        let figures = "\n<run id=\"r1\" figure/>".repeat(figures);
        format!(
            "Cosine is training.\n\n## Findings\n{}{figures}",
            [line; 40][..lines].join("\n")
        )
    };
    // A first line, a heading and one figure take 24 + 36 + 205 = 265px of the 645.
    assert_eq!(height(&report(15, 1)), 625);
    assert!(!outgrown(&report(15, 1)), "625px fits");
    assert!(outgrown(&report(16, 1)), "649px does not");
    assert!(
        outgrown(&report(8, 2)),
        "a second figure takes what text would"
    );
    let wide = format!("- {}", "word ".repeat(50));
    assert_eq!(height(&wide), 3 * ROW_PX, "250 characters take three rows");
    let tagged = format!(
        "- **Done.** <run id=\"{}\" label=\"0.0106\"/> [log](artifacts/{}.txt)",
        "a".repeat(2000),
        "b".repeat(2000)
    );
    assert_eq!(
        height(&tagged),
        ROW_PX,
        "a tag reads as its label, a link as its text"
    );
}

/// A chat whose report is PREVIOUS made too long for the card, due for its next update.
fn overgrown(store: &Store) -> String {
    let session = opened(store);
    say(
        store,
        "chat",
        "m1",
        "assistant",
        "Depth 8 beat depth 4.",
        10,
    );
    let long = format!(
        "{PREVIOUS}\n{}",
        "- Depth 8 beat depth 4.\n".repeat(CARD_PX / ROW_PX * 2)
    );
    store.chat_report_due(&session.id, None, 1).unwrap();
    let taken = ReportTaken {
        read_at: 2,
        last_note: None,
        due_at: Some(1),
        turn_due_at: None,
        unread_version: None,
    };
    store
        .finish_chat_report("chat", ReportOutcome::Written(long.trim()), taken, 2)
        .unwrap();
    store.chat_report_due("chat", None, 3).unwrap();
    long.trim().to_owned()
}

#[cfg(unix)]
#[tokio::test]
async fn an_overgrown_report_is_rewritten_whole_and_checked() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let invented = "Cosine is training.\n\n## Findings\n- Depth 8 won by 12.5%.";
    let condensed = "Cosine is training.\n\n## Findings\n- Depth 8 beat depth 4.";
    super::super::tests::fake_claude(&dir, &[invented, condensed]);
    let store = Store::open().unwrap();
    overgrown(&store);

    settle(&host()).await;

    let sent = prompt(&dir, 1);
    assert!(sent.contains("\"condense\":true"), "{sent}");
    let correction = prompt(&dir, 2);
    assert!(
        correction.contains("12.5%") && correction.contains("reply with the whole report"),
        "{correction}"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some(condensed));
    let args = std::fs::read_to_string(dir.join("args-1")).unwrap();
    assert!(args.contains("When \"condense\" is true"), "{args}");
}

#[cfg(unix)]
#[tokio::test]
async fn edits_under_condense_apply_and_a_failed_condense_keeps_the_report() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let edits = edit("Cosine is training.", "Depth 8 is training.");
    super::super::tests::fake_claude(&dir, &[&edits, "UNCHANGED", "UNCHANGED"]);
    let store = Store::open().unwrap();
    let long = overgrown(&store);
    let host = host();

    settle(&host).await;
    assert_eq!(
        super::super::tests::calls(&dir),
        1,
        "edits are a valid update"
    );
    let edited = long.replace("Cosine is training.", "Depth 8 is training.");
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(report.body.as_deref(), Some(edited.as_str()));

    age(&store, "chat");
    store
        .chat_report_due("chat", None, crate::store::now_ms())
        .unwrap();
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 3);
    assert!(
        prompt(&dir, 2).contains("\"condense\":true"),
        "still too long"
    );
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(
        report.body.as_deref(),
        Some(edited.as_str()),
        "a failed condense keeps it"
    );
    let error = report.error.unwrap();
    assert!(error.contains("UNCHANGED"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_whole_reply_that_cites_nothing_is_not_a_report() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let refusal = "I'm not going to produce that report here.";
    super::super::tests::fake_claude(&dir, &[refusal, refusal, refusal, refusal]);
    let store = Store::open().unwrap();
    opened(&store);
    super::super::tests::run(&store, "old", "Cosine", "chat", "step 1\n");
    let host = host();

    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 2);
    let first = store.chat_report("chat").unwrap().unwrap();
    assert!(first.body.is_none(), "no first report from a refusal");
    assert!(first.error.unwrap().contains("cites no run"));

    let previous = format!(
        "Cosine is training.\n\n## Findings\n- Cosine beat the baseline. <run id=\"old\" label=\"0.0106\"/>\n{}",
        "- Depth 8 beat depth 4.\n".repeat(CARD_PX / ROW_PX * 2)
    );
    let previous = previous.trim();
    let taken = ReportTaken {
        read_at: 2,
        last_note: None,
        due_at: None,
        turn_due_at: None,
        unread_version: None,
    };
    store
        .finish_chat_report("chat", ReportOutcome::Written(previous), taken, 2)
        .unwrap();
    store.chat_report_due("chat", None, 3).unwrap();
    settle(&host).await;
    assert_eq!(super::super::tests::calls(&dir), 4);
    assert!(prompt(&dir, 3).contains("\"condense\":true"));
    let report = store.chat_report("chat").unwrap().unwrap();
    assert_eq!(
        report.body.as_deref(),
        Some(previous),
        "a refused condense keeps it"
    );
    assert!(report.error.unwrap().contains("cites no run"));

    let dropped = "Cosine is training.\n\n## Findings\n- Depth 8 won. <run id=\"new\"/>";
    let known = Known::of(&json!({"report": previous, "runs": [{"run": "new"}]}));
    let mut errors = checked(dropped, &known);
    errors.retain(|error| !error.contains("figure"));
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_report_shows_one_figure() {
    let known = Known::of(&json!({
        "runs": [{"run": "r1", "status": "done", "figure": true}, {"run": "r2", "status": "done", "figure": true}],
    }));
    let one = "Depth 8 won.\n\n<run id=\"r2\" figure/>";
    assert!(
        checked(one, &known).is_empty(),
        "{:?}",
        checked(one, &known)
    );
    let two = "Depth 8 won.\n\n<run id=\"r1\" figure/>\n<run id=\"r2\" figure/>";
    let errors = checked(two, &known);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("show one figure: the run whose comparison with its parent"));
}

#[cfg(unix)]
#[tokio::test]
async fn with_status_off_for_new_chats_only_open_reports_and_the_users_choice_write() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let dir = crate::config::home_dir().unwrap().join("claude-calls");
    let tagged = "Width 512 finished. <run id=\"r-second\"/>";
    super::super::tests::fake_claude(&dir, &["Width 512 finished.", tagged]);
    let store = Store::open().unwrap();
    let host = host();
    store.set_status_for_new_chats(false).unwrap();
    let now = crate::store::now_ms();
    let fresh = chat(&store, "fresh", "claude-code");
    ended_run(
        &store,
        "fresh",
        "r-first",
        now - 10 * paced_ms(OPEN_AFTER_S),
    );
    super::super::tests::run(&store, "r-second", "Width 512", "fresh", "step 1\n");
    let open = chat(&store, "open", "claude-code");
    store.set_chat_report_enabled("open", true, 1).unwrap();
    store.conn_execute_for_tests("UPDATE chat_reports SET due_at = NULL");

    settle(&host).await;
    assert_eq!(
        report_json(&store, "fresh").unwrap(),
        Value::Null,
        "nothing opens on its own"
    );

    noted(&store, &open, "Width 512 finished");
    settle(&host).await;
    assert_eq!(
        super::super::tests::calls(&dir),
        1,
        "an open report keeps updating"
    );

    set_enabled(&host, &store, &fresh, true).unwrap().unwrap();
    settle(&host).await;
    assert_eq!(
        super::super::tests::calls(&dir),
        2,
        "the user's own Turn on still works"
    );
    assert!(store.chat_report("fresh").unwrap().unwrap().body.is_some());
}
