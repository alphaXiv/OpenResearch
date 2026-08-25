//! The `agent` command group: delegate work to a second agent session.
//!
//!   orx agent spawn "<task>"   start a helper agent on its own top-level session
//!
//! Only meaningful inside a local `orx up` agent session. `ORX_LOCAL_SESSION`
//! marks the process as one; `ORX_CHAT_SESSION_ID` names the session doing the
//! spawning (see `local::chat::set_chat_session_env`). Both are needed — the
//! cloud opencode plugin exports the session id too, for run attribution.
//!
//! This command only writes the child's session row and a `chat_spawns` record;
//! it never runs the child itself. The resident `orx up` picks the record up,
//! starts the helper's first turn, and (unless `--no-wake`) wakes the parent
//! when the helper is done. Same store-and-watcher split as `orx exp wake`, and for
//! the same reason: the CLI is a short-lived subprocess with no harness of its
//! own to run a turn on.

use std::io::Read;

use crate::error::{anyhow, Result};
use crate::local::harness::PermissionMode;
use crate::store::{now_ms, ChatSpawn, Store, StoredChatSession};

use crate::AgentCommand;

/// Helpers one session may have in flight at once.
pub(crate) const MAX_LIVE_SPAWNS: i64 = 5;

pub async fn run(args: crate::AgentArgs) -> Result<()> {
    let store = Store::open()?;
    match args.command {
        AgentCommand::Spawn {
            task,
            stdin,
            title,
            harness,
            model,
            no_wake,
        } => spawn(&store, task, stdin, title, harness, model, !no_wake),
    }
}

/// Read the task from the positional argument or, with `--stdin`, from the
/// whole of stdin (agents write multi-paragraph briefs as heredocs).
fn task_text(task: Option<String>, stdin: bool) -> Result<String> {
    if stdin {
        if task.is_some() {
            return Err(anyhow!(
                "Pass the task as an argument or --stdin, not both."
            ));
        }
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| anyhow!("Could not read the task from stdin: {e}"))?;
        return non_empty(buf);
    }
    non_empty(task.unwrap_or_default())
}

fn non_empty(text: String) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(anyhow!(
            "Describe the task for the spawned agent: `orx agent spawn \"<task>\"`."
        ));
    }
    Ok(trimmed.to_string())
}

/// Why this session may not spawn right now, if it may not. Depth and breadth
/// are the two ways one request becomes an unbounded tree of paid sessions, and
/// nothing downstream of here bounds either.
fn spawn_refusal(parent: &StoredChatSession, live: i64) -> Option<String> {
    if parent.parent_session_id.is_some() {
        return Some(
            "This session was itself spawned by another agent, and spawned agents cannot spawn \
             their own. Do the task here, or report back so the session that spawned you can \
             delegate it."
                .to_string(),
        );
    }
    (live >= MAX_LIVE_SPAWNS).then(|| {
        format!(
            "You already have {live} agents in flight, the most one session may run at once. \
             Wait for one to report back before spawning another."
        )
    })
}

fn spawn(
    store: &Store,
    task: Option<String>,
    stdin: bool,
    title: Option<String>,
    harness: Option<String>,
    model: Option<String>,
    wake_parent: bool,
) -> Result<()> {
    if !crate::local::chat::in_local_session() {
        return Err(anyhow!(
            "`orx agent spawn` is only available inside a local `orx up` agent session."
        ));
    }
    let parent_id = crate::local::chat::launching_chat_session()
        .ok_or_else(|| anyhow!("This agent session has no chat id to spawn from."))?;
    let parent = store
        .get_chat_session(&parent_id)?
        .ok_or_else(|| anyhow!("The current chat session no longer exists."))?;
    let prompt = task_text(task, stdin)?;
    if let Some(refusal) = spawn_refusal(&parent, store.count_live_chat_spawns(&parent_id)?) {
        return Err(anyhow!(refusal));
    }
    let harness = harness.unwrap_or_else(|| parent.harness.clone());
    if !crate::local::harness::is_chat_harness(&harness) {
        return Err(anyhow!("unknown harness: {harness}"));
    }
    // Settings only carry over when the child runs the same harness; a model or
    // permission-mode id from one CLI is meaningless to another.
    let inherits = harness == parent.harness;
    // Claude activates Plan through its permission mode, not the plan axis, so
    // clearing `plan_mode` alone would still hand a planning parent's helper a
    // mode that only ever produces a plan.
    let plan_permission =
        crate::local::harness::permission_id_for_mode(&harness, PermissionMode::Plan);
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let session = StoredChatSession {
        id: format!("chat_{}", uuid::Uuid::new_v4()),
        project_id: parent.project_id.clone(),
        harness,
        native_session_id: None,
        // "user" = explicitly chosen, so auto-titling leaves it alone.
        title_source: title.is_some().then(|| "user".to_string()),
        title,
        model: model.or_else(|| inherits.then(|| parent.model.clone()).flatten()),
        permission_mode: inherits
            .then(|| parent.permission_mode.clone())
            .flatten()
            .filter(|mode| Some(mode) != plan_permission.as_ref()),
        plan_mode: false,
        plan_reset_pending: false,
        reasoning_level: inherits.then(|| parent.reasoning_level.clone()).flatten(),
        archived: false,
        context_usage_json: None,
        bootstrap_context: None,
        active_leaf_id: None,
        parent_session_id: Some(parent_id.clone()),
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    // One transaction: a session row without its spawn row is an empty session
    // in the user's sidebar that nothing will ever start.
    let tx = store.begin()?;
    store.create_chat_session(&session)?;
    store.create_chat_spawn(&ChatSpawn {
        session_id: session.id.clone(),
        parent_session_id: parent_id,
        prompt,
        wake_parent,
        attempts: 0,
        finished_at: None,
    })?;
    tx.commit()?;
    println!("Spawned agent session {}.", session.id);
    println!("It starts within a few seconds and works in its own git worktree.");
    if wake_parent {
        println!("This chat will be resumed with its result when it finishes.");
    } else {
        println!("You will NOT be told when it finishes; check its session yourself.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{spawn_refusal, task_text, MAX_LIVE_SPAWNS};
    use crate::store::StoredChatSession;

    fn parent(parent_session_id: Option<&str>) -> StoredChatSession {
        StoredChatSession {
            id: "chat_parent".into(),
            project_id: "p1".into(),
            harness: "codex".into(),
            native_session_id: None,
            title: None,
            title_source: None,
            model: None,
            permission_mode: None,
            plan_mode: false,
            plan_reset_pending: false,
            reasoning_level: None,
            archived: false,
            context_usage_json: None,
            bootstrap_context: None,
            active_leaf_id: None,
            parent_session_id: parent_session_id.map(str::to_string),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn a_spawned_session_may_not_spawn_its_own() {
        let refusal = spawn_refusal(&parent(Some("chat_grandparent")), 0)
            .expect("a spawned session must be refused");
        assert!(refusal.contains("cannot spawn"), "{refusal}");
        // Depth is refused regardless of how few helpers are in flight.
        assert!(spawn_refusal(&parent(None), 0).is_none());
    }

    #[test]
    fn one_session_may_only_run_so_many_helpers_at_once() {
        assert!(spawn_refusal(&parent(None), MAX_LIVE_SPAWNS - 1).is_none());
        let refusal =
            spawn_refusal(&parent(None), MAX_LIVE_SPAWNS).expect("the cap must refuse one more");
        assert!(refusal.contains("in flight"), "{refusal}");
    }

    #[test]
    fn a_task_is_required_and_comes_from_one_place() {
        assert_eq!(
            task_text(Some("  Sweep the literature  ".into()), false).unwrap(),
            "Sweep the literature"
        );
        assert!(task_text(None, false).is_err());
        assert!(task_text(Some("   ".into()), false).is_err());
        // --stdin and a positional together are ambiguous, so neither is used.
        assert!(task_text(Some("from the args".into()), true).is_err());
    }
}
