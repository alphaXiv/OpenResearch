//! Making a chat a project of its own: a chat without a project becomes one, and
//! a chat in a project moves into a new one. Both change where the chat's files
//! live, so they wait until the chats involved are idle; a request made while one
//! runs is kept in the store and carried out when its turn ends.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::json;

use super::{
    session_json, ChatHost, OrxDisplay, OrxKind, OrxSource, TurnOverrides, ORX_MESSAGE_PREFIX,
};
use crate::error::{anyhow, Result};
use crate::local::model::LocalProject;
use crate::local::projects;
use crate::store::Store;

const CANCELLED: &str = "The move was cancelled.";
/// A chat started a turn before the move could begin.
const BUSY: &str = "A chat is busy.";

/// The chats whose files move with `session_id`: every chat of a scratch
/// project, else the chat and the side chats in its worktree.
fn scope(store: &Store, session_id: &str) -> Result<(LocalProject, String, Vec<String>)> {
    let session = store
        .get_chat_session(session_id)?
        .ok_or_else(|| anyhow!("Chat {session_id} was not found."))?;
    let project = store
        .get_local_project(&session.project_id)?
        .ok_or_else(|| anyhow!("Project {} was not found.", session.project_id))?;
    let owner = store.chat_worktree_owner(session_id)?;
    let mut sessions = Vec::new();
    for chat in store.list_chat_sessions_by_project(&project.id)? {
        if project.is_scratch() || store.chat_worktree_owner(&chat.id)? == owner {
            sessions.push(chat.id);
        }
    }
    Ok((project, owner, sessions))
}

impl ChatHost {
    /// Makes `session_id`'s chat a project named `name` at `path` now if its
    /// chats are idle, else once its turn ends, returning `None` meanwhile.
    pub async fn move_to_new_project(
        self: &Arc<Self>,
        session_id: &str,
        name: &str,
        path: &str,
    ) -> Result<Option<LocalProject>> {
        let (name, _) = projects::new_project_place(name, path)?;
        let (_, _, sessions) = scope(&Store::open()?, session_id)?;
        if !self.idle(&sessions).await? {
            Store::open()?.set_chat_project_move(session_id, &name, path)?;
            self.project_moves_waiting.store(true, Ordering::SeqCst);
            self.emit_sessions(&sessions).await;
            return Ok(None);
        }
        let place = Some((name.clone(), path.to_string()));
        match self.make_project(session_id, place).await {
            // A turn started first: wait for it like any request made mid-turn.
            Err(error) if error.to_string() == BUSY => {
                Store::open()?.set_chat_project_move(session_id, &name, path)?;
                self.project_moves_waiting.store(true, Ordering::SeqCst);
                self.emit_sessions(&sessions).await;
                Ok(None)
            }
            moved => moved.map(Some),
        }
    }

    /// Whether there was a waiting move to cancel; `false` once it has started.
    pub async fn cancel_project_move(&self, session_id: &str) -> Result<bool> {
        let cancelled = Store::open()?.take_chat_project_move(session_id)?.is_some();
        self.emit_sessions(&[session_id.to_string()]).await;
        Ok(cancelled)
    }

    /// Carries out every waiting move whose chats are idle now: as a turn ends,
    /// and when the dashboard starts.
    pub async fn apply_project_moves(self: &Arc<Self>) {
        // Cleared first, so a move stored meanwhile sets it again.
        if !self.project_moves_waiting.swap(false, Ordering::SeqCst) {
            return;
        }
        let waiting = match Store::open().and_then(|store| store.chat_project_move_sessions()) {
            Ok(waiting) => waiting,
            Err(error) => {
                self.project_moves_waiting.store(true, Ordering::SeqCst);
                eprintln!("orx up: could not read waiting project moves: {error}");
                return;
            }
        };
        for session_id in waiting {
            let Ok(store) = Store::open() else {
                self.project_moves_waiting.store(true, Ordering::SeqCst);
                return;
            };
            let Ok((_, _, sessions)) = scope(&store, &session_id) else {
                let _ = store.take_chat_project_move(&session_id);
                continue;
            };
            drop(store);
            if !self.idle(&sessions).await.unwrap_or(false) {
                self.project_moves_waiting.store(true, Ordering::SeqCst);
                continue;
            }
            let note = match self.make_project(&session_id, None).await {
                Ok(project) => format!(
                    "The user made this chat the project \"{}\", at {}, and this chat now belongs to it. Your worktree keeps every file, uncommitted ones included; the project folder holds what you have committed. Continue from where you left off.",
                    project.name, project.repo_path
                ),
                Err(error) if error.to_string() == CANCELLED => continue,
                // A turn that started first keeps it waiting.
                Err(error) if error.to_string() == BUSY => {
                    self.project_moves_waiting.store(true, Ordering::SeqCst);
                    continue;
                }
                Err(error) => format!(
                    "Making this chat a project of its own failed: {error}. The chat stays where it was; tell the user."
                ),
            };
            let shown = OrxDisplay {
                text: note.clone(),
                source: OrxSource {
                    kind: OrxKind::Message,
                    from: None,
                    status: None,
                },
            };
            if let Err(error) = self
                .steer_message(
                    &session_id,
                    format!("{ORX_MESSAGE_PREFIX}{note}"),
                    TurnOverrides::default(),
                    Vec::new(),
                    Vec::new(),
                    None,
                    Some(shown),
                )
                .await
            {
                eprintln!("orx up: could not tell chat {session_id} about its project: {error}");
            }
        }
    }

    async fn idle(&self, sessions: &[String]) -> Result<bool> {
        let busy = self.busy_sessions().await;
        let store = Store::open()?;
        for id in sessions {
            if busy.contains(id) || store.chat_turn_leased(id)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn make_project(
        self: &Arc<Self>,
        session_id: &str,
        // `None` takes the waiting move for the chat, if it's still there.
        place: Option<(String, String)>,
    ) -> Result<LocalProject> {
        let (_, _, checked) = scope(&Store::open()?, session_id)?;
        // Stopped first, while the chats are idle, so none restarts its agent
        // mid-turn afterwards; their next turn starts in the new project.
        for id in &checked {
            self.opencode.kill_session(id).await;
            self.codex.kill_session(id).await;
            self.claude.kill_session(id).await;
            self.acp.stop(id).await;
        }
        let mut ran = false;
        let moved = self
            .while_idle(&checked, || {
                ran = true;
                let store = Store::open()?;
                // Read again under the guard: another move may have just changed it.
                // Taken while no turn can start, so neither a turn nor a cancel can race it.
                // A request made now replaces one that was waiting.
                let waiting = store.take_chat_project_move(session_id)?;
                let (name, path) = match place.or(waiting) {
                    Some(place) => place,
                    None => return Err(anyhow!(CANCELLED)),
                };
                let (project, owner, sessions) = scope(&store, session_id)?;
                let unchanged = sessions.len() == checked.len()
                    && sessions.iter().all(|id| checked.contains(id));
                if !unchanged {
                    return Err(anyhow!("This chat just moved; try again."));
                }
                let created = if project.is_scratch() {
                    let worktree =
                        crate::local::git::existing_session_worktree_path(&project, &owner);
                    projects::promote_scratch_project(
                        &store,
                        &project,
                        Some(&worktree),
                        &name,
                        &path,
                    )
                } else {
                    projects::split_chat_project(&store, &project, &owner, &sessions, &name, &path)
                }?;
                Ok((created, sessions))
            })
            .await;
        let (created, sessions) = match moved {
            Err(_) if !ran => return Err(anyhow!(BUSY)),
            moved => moved?,
        };
        self.emit_sessions(&sessions).await;
        Ok(created)
    }

    async fn emit_sessions(&self, sessions: &[String]) {
        let busy = self.busy_sessions().await;
        let Ok(store) = Store::open() else { return };
        for id in sessions {
            if let Ok(Some(session)) = store.get_chat_session(id) {
                let session = session_json(&session, busy.contains(id));
                self.emit("chat.session", json!({ "session": session }));
            }
        }
    }
}
