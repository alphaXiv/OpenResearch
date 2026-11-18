//! Taking back what a previous server kept ([restarts](../../../arc/restarts.md)): each live
//! kept agent goes back to its harness, and the turn it was running resumes from its
//! checkpoint. What cannot be taken back is stopped, and its turn recovers as it always has.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::{
    materialize_unfinished_turns, message_json, stored_to_wire, turn_ctx_from_stored, ChatHost,
    TurnCtx, TurnGuard, TurnState, TurnSubmission, WireMessage,
};
use crate::error::{anyhow, bail, Result};
use crate::keep::{self, Liveness, Owner, Record};
use crate::local::harness::{session_harness, Adoption, Harness};
use crate::store::{Store, StoredChatTurn};

/// How long a harness may take to take back one agent before it is stopped instead.
const ADOPT_TIMEOUT: Duration = Duration::from_secs(30);

const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// A running turn whose agent was taken back to resume it from `offset` in `keep_id`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Resumable {
    turn_id: String,
    keep_id: String,
    offset: u64,
}

/// One chat's live keepers: its agents and the commands they started.
#[derive(Default)]
struct Kept {
    agents: Vec<(String, Value)>,
    commands: Vec<(String, String, Value)>,
}

fn unfinished(turn: &StoredChatTurn) -> bool {
    matches!(turn.state.as_str(), "preparing" | "retrying" | "running")
}

/// Gives every live kept agent back to its harness, stops those none takes back, clears out
/// keepers that are gone, then resumes the running turns that can and fails the rest.
/// `reachable` is false when this server lacks the port or bearer its agents' environments
/// name, so none could call it.
pub async fn adopt_kept_work(chat: &Arc<ChatHost>, reachable: bool) -> Result<()> {
    // A crashed server's turns resume now, not once their leases run out.
    Store::open()?.clear_chat_turn_leases()?;
    let mut kept: BTreeMap<(String, String), Kept> = BTreeMap::new();
    let mut strays = Vec::new();
    for entry in keep::entries()? {
        match (entry.owner, entry.liveness) {
            // The page reattaches its terminals.
            (Record::Known(Owner::Terminal { .. }), _) => {}
            (_, Liveness::Dead) => {
                if let Err(error) = keep::remove(&entry.id) {
                    eprintln!("orx up: {error:#}");
                }
            }
            // Whatever could not be read was logged; it stays as it is.
            (Record::Unreadable, _) | (_, Liveness::Unknown) => {}
            (
                Record::Known(Owner::Agent {
                    harness,
                    session_id,
                    state,
                }),
                Liveness::Alive,
            ) => kept
                .entry((harness, session_id))
                .or_default()
                .agents
                .push((entry.id, state)),
            (
                Record::Known(Owner::AgentCommand {
                    harness,
                    session_id,
                    command_id,
                    state,
                }),
                Liveness::Alive,
            ) => kept
                .entry((harness, session_id))
                .or_default()
                .commands
                .push((entry.id, command_id, state)),
            (Record::Known(Owner::Unknown) | Record::Missing, Liveness::Alive) => {
                strays.push(entry.id)
            }
        }
    }
    let strays = strays.into_iter().map(|id| {
        stop(
            id,
            "child whose owner this version of orx does not know".into(),
        )
    });
    let adoptions = kept
        .into_iter()
        .map(|((harness, session_id), kept)| adopt(chat, harness, session_id, kept, reachable));
    futures::future::join(
        futures::future::join_all(strays),
        futures::future::join_all(adoptions),
    )
    .await;

    resume_turns(chat, true).await
}

async fn adopt(
    chat: &Arc<ChatHost>,
    harness_id: String,
    session_id: String,
    kept: Kept,
    reachable: bool,
) {
    let keepers: Vec<&str> = kept.agents.iter().map(|(id, _)| id.as_str()).collect();
    let claimed = if reachable {
        claim(chat, &harness_id, &session_id, &keepers)
    } else {
        Err("this server lacks the port or bearer its environment names".into())
    };
    let (harness, resume) = match claimed {
        Ok(claimed) => claimed,
        Err(reason) => {
            let ids = kept
                .agents
                .into_iter()
                .map(|(id, _)| id)
                .chain(kept.commands.into_iter().map(|(id, ..)| id));
            let reason = format!("{harness_id} agent of chat {session_id}: {reason}");
            futures::future::join_all(ids.map(|id| stop(id, reason.clone()))).await;
            return;
        }
    };

    let mut commands = Some(kept.commands);
    for (keep_id, state) in kept.agents {
        let resume_at = resume
            .as_ref()
            .filter(|resume| resume.keep_id == keep_id)
            .map(|resume| resume.offset);
        let adoption = Adoption {
            keep_id: keep_id.clone(),
            session_id: session_id.clone(),
            state,
            resume_at,
            commands: commands.take().unwrap_or_default(),
        };
        let command_ids: Vec<String> = adoption
            .commands
            .iter()
            .map(|(id, ..)| id.clone())
            .collect();
        let adopted = tokio::time::timeout(ADOPT_TIMEOUT, harness.adopt(chat, adoption))
            .await
            .unwrap_or_else(|_| {
                Err(anyhow!(
                    "it was not taken back within {} seconds",
                    ADOPT_TIMEOUT.as_secs()
                ))
            });
        match adopted {
            Ok(()) => {
                if let Some(resume) = resume.clone().filter(|_| resume_at.is_some()) {
                    chat.resumable
                        .lock()
                        .unwrap()
                        .insert(session_id.clone(), resume);
                }
            }
            Err(error) => {
                let reason = format!("{harness_id} agent of chat {session_id}: {error:#}");
                let ids = std::iter::once(keep_id).chain(command_ids);
                futures::future::join_all(ids.map(|id| stop(id, reason.clone()))).await;
            }
        }
    }
    if let Some(orphans) = commands.filter(|commands| !commands.is_empty()) {
        let reason = format!("command of chat {session_id}, whose {harness_id} agent is gone");
        futures::future::join_all(orphans.into_iter().map(|(id, ..)| stop(id, reason.clone())))
            .await;
    }
}

/// The harness that takes back `session_id`'s kept agents, and the running turn that resumes
/// on one of `keepers`; `Err` says why they are stopped instead. A running turn that cannot
/// resume takes its agent with it, as a restart always did, so its recovery starts afresh.
fn claim(
    chat: &Arc<ChatHost>,
    harness_id: &str,
    session_id: &str,
    keepers: &[&str],
) -> std::result::Result<(Box<dyn Harness>, Option<Resumable>), String> {
    let store = Store::open().map_err(|error| format!("{error:#}"))?;
    let session = match store.get_chat_session(session_id) {
        Ok(Some(session)) => session,
        Ok(None) => return Err("its chat was deleted".into()),
        Err(error) => return Err(format!("its chat could not be read: {error:#}")),
    };
    if session.harness != harness_id {
        return Err(format!("the chat now runs {}", session.harness));
    }
    let harness = session_harness(harness_id, session_id)
        .ok_or_else(|| format!("no harness of this version runs {harness_id}"))?;
    let turn = store
        .latest_chat_turn(session_id)
        .map_err(|error| format!("its turns could not be read: {error:#}"))?
        .filter(unfinished);
    let Some(turn) = turn else {
        return Ok((harness, None));
    };
    if !harness.supports_resume() {
        return Err(format!("its running turn {} cannot resume", turn.id));
    }
    let ctx = resumed_turn(chat, &store, &turn)
        .map_err(|error| format!("its running turn cannot resume: {error:#}"))?;
    let resume = ctx.resume.as_ref().expect("a restored checkpoint resumes");
    if !keepers.contains(&resume.keep_id.as_str()) {
        return Err(format!(
            "its running turn {} resumes on keeper {}, which is gone",
            turn.id, resume.keep_id
        ));
    }
    Ok((
        harness,
        Some(Resumable {
            turn_id: turn.id,
            keep_id: resume.keep_id.clone(),
            offset: resume.offset,
        }),
    ))
}

/// A turn as its last checkpoint left it, on the transcript committed with that checkpoint.
fn resumed_turn(chat: &Arc<ChatHost>, store: &Store, turn: &StoredChatTurn) -> Result<TurnCtx> {
    let checkpoint = store
        .chat_turn_checkpoint(&turn.id)?
        .ok_or_else(|| anyhow!("turn {} has no checkpoint", turn.id))?;
    let session = store
        .get_chat_session(&turn.session_id)?
        .ok_or_else(|| anyhow!("chat {} is gone", turn.session_id))?;
    let project = store
        .get_local_project(&session.project_id)?
        .ok_or_else(|| anyhow!("project {} is gone", session.project_id))?;
    let assistant = match store.get_chat_message(&turn.assistant_message_id)? {
        Some(message) => stored_to_wire(&message),
        None => WireMessage {
            id: turn.assistant_message_id.clone(),
            role: "assistant".into(),
            parts: Vec::new(),
            created_at: turn.created_at,
            completed_at: None,
            parent_id: turn.user_message_id.clone(),
        },
    };
    let mut ctx = turn_ctx_from_stored(chat.clone(), store, &session, project, turn, assistant)?;
    ctx.restore_checkpoint(&checkpoint)?;
    Ok(ctx)
}

/// Resumes each turn adoption made resumable once no other process holds its lease, then fails
/// the unfinished turns no one holds, as a server has always recovered them: at `startup` all
/// of them, later those whose lease expired. A resumed turn holds its lease, so it is spared.
pub(super) async fn resume_turns(chat: &Arc<ChatHost>, startup: bool) -> Result<()> {
    let store = Store::open()?;
    let pending: Vec<(String, Resumable)> = chat
        .resumable
        .lock()
        .unwrap()
        .iter()
        .map(|(session_id, resume)| (session_id.clone(), resume.clone()))
        .collect();
    let mut claimed = Vec::new();
    for (session_id, resume) in pending {
        // Refused while another claim is live, which past startup is a turn this server runs.
        let Some(guard) = TurnGuard::claim(chat, &session_id, None).await else {
            continue;
        };
        chat.resumable.lock().unwrap().remove(&session_id);
        match resumed(chat, &store, &session_id, &resume) {
            Ok(ctx) => claimed.push((ctx, guard)),
            Err(error) => {
                // Its agent waits for the turn to go on, so recovery starts on a fresh one.
                let reason = format!(
                    "agent of chat {session_id}, whose turn {} cannot resume: {error:#}",
                    resume.turn_id
                );
                stop(resume.keep_id.clone(), reason).await;
                // Without passing the slot to a queued message: the turn has yet to fail.
                guard.defuse();
                chat.turns.lock().await.remove(&session_id);
                chat.release_durable_turn(&session_id);
            }
        }
    }

    let unfinished = if startup {
        store.reconcile_unfinished_chat_turns()?
    } else {
        store.reconcile_expired_unfinished_chat_turns(&chat.held_turn_leases())?
    };
    for (session_id, message) in materialize_unfinished_turns(&store, unfinished)? {
        chat.emit("chat.message", message_json(&message, &session_id));
    }

    for (ctx, guard) in claimed {
        let (session_id, turn_id) = (ctx.session_id.clone(), ctx.turn_id.clone());
        if let Some(TurnState::Reserved { turn_id: reserved }) =
            chat.turns.lock().await.get_mut(&session_id)
        {
            *reserved = Some(turn_id.clone());
        }
        chat.emit(
            "chat.busy",
            json!({ "sessionId": &session_id, "busy": true }),
        );
        match chat.launch_turn_ctx(ctx, guard, None).await {
            Ok(TurnSubmission::Started(_)) => {}
            Ok(_) => eprintln!("orx up: turn {turn_id} of chat {session_id} did not resume"),
            Err(error) => {
                eprintln!("orx up: could not resume turn {turn_id} of chat {session_id}: {error:#}")
            }
        }
    }
    Ok(())
}

/// The turn `resume` names, if it is still the chat's running turn and its checkpoint is still
/// where its agent was told it resumes.
fn resumed(
    chat: &Arc<ChatHost>,
    store: &Store,
    session_id: &str,
    resume: &Resumable,
) -> Result<TurnCtx> {
    let turn = store
        .latest_chat_turn(session_id)?
        .filter(|turn| turn.id == resume.turn_id && unfinished(turn))
        .ok_or_else(|| anyhow!("it is no longer the chat's running turn"))?;
    let ctx = resumed_turn(chat, store, &turn)?;
    let restored = ctx.resume.as_ref().expect("a restored checkpoint resumes");
    if restored.keep_id != resume.keep_id || restored.offset != resume.offset {
        bail!("its checkpoint moved after its agent was taken back");
    }
    Ok(ctx)
}

async fn stop(id: String, reason: String) {
    eprintln!("orx up: stopping kept child {id}, the {reason}");
    match keep::attach(&id).await {
        Ok(kept) => {
            if kept.handle().stop(STOP_TIMEOUT).await.is_none() {
                eprintln!("orx up: kept child {id} did not stop within {STOP_TIMEOUT:?}");
            }
        }
        Err(error) => eprintln!("orx up: could not stop kept child {id}: {error:#}"),
    }
}

/// Stops every live kept child whose owner `which` selects (`None` for a missing record):
/// what a quit or a data-directory move leaves running once the harnesses stopped theirs. A
/// child whose record or liveness could not be read is left running.
pub async fn stop_kept(which: impl Fn(Option<&Owner>) -> bool, because: &str) {
    let entries = match keep::entries() {
        Ok(entries) => entries,
        Err(error) => return eprintln!("orx up: leaving kept children running: {error:#}"),
    };
    let stops = entries
        .into_iter()
        .filter(|entry| {
            entry.liveness == Liveness::Alive
                && match &entry.owner {
                    Record::Known(owner) => which(Some(owner)),
                    Record::Missing => which(None),
                    Record::Unreadable => false,
                }
        })
        .map(|entry| stop(entry.id, format!("child left running when {because}")));
    futures::future::join_all(stops).await;
}

/// Which chats' work a restart leaves for the next server: those whose harness resumes turns
/// and whose agent is kept, with a running turn checkpointed in that agent's keeper.
pub(super) struct Survivors {
    keepers: HashMap<String, HashSet<String>>,
    store: Option<Store>,
}

impl Survivors {
    pub(super) fn now() -> Self {
        let mut keepers: HashMap<String, HashSet<String>> = HashMap::new();
        let entries = keep::entries().unwrap_or_else(|error| {
            eprintln!("orx up: no kept child counts as surviving a restart: {error:#}");
            Vec::new()
        });
        for entry in entries {
            if let (Liveness::Alive, Record::Known(Owner::Agent { session_id, .. })) =
                (entry.liveness, entry.owner)
            {
                keepers.entry(session_id).or_default().insert(entry.id);
            }
        }
        // Unreadable, nothing survives: a restart is held back rather than let it stop work.
        Self {
            keepers,
            store: Store::open().ok(),
        }
    }

    pub(super) fn resume(&self, session_id: &str, turn: Option<&TurnState>) -> bool {
        let (Some(keepers), Some(store)) = (self.keepers.get(session_id), &self.store) else {
            return false;
        };
        let resumes = store
            .get_chat_session(session_id)
            .ok()
            .flatten()
            .and_then(|session| session_harness(&session.harness, session_id))
            .is_some_and(|harness| harness.supports_resume());
        resumes
            && match turn {
                None => true,
                Some(TurnState::Active(active)) => store
                    .chat_turn_checkpoint(&active.turn_id)
                    .ok()
                    .flatten()
                    .is_some_and(|checkpoint| keepers.contains(&checkpoint.keep_id)),
                Some(_) => false,
            }
    }
}

#[cfg(test)]
mod tests;
