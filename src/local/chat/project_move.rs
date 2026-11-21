//! Making a chat a project of its own: a chat without a project becomes one, and
//! a chat in a project moves into a new one. Both change where the chat's files
//! live, so they wait until the chats involved are idle; a request made while one
//! runs is kept in the store and carried out when its turn ends.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::json;

use super::{
    session_json, ChatHost, OrxDisplay, OrxKind, OrxSource, TurnGuard, TurnOverrides, TurnState,
    ORX_MESSAGE_PREFIX,
};
use crate::error::{anyhow, Result};
use crate::local::model::LocalProject;
use crate::local::projects;
use crate::store::Store;

/// Takes a project for one change, or refuses while another change to it or a
/// data-directory move is under way; the change lasts until the box drops.
pub type ProjectChangeGuard = Box<dyn Fn(&str) -> Option<Box<dyn Send>> + Send + Sync>;

use projects::MOVE_CANCELLED as CANCELLED;
const CHANGED: &str =
    "This chat's project changed in another window; open the chat again to try again.";
/// A chat started a turn, or the project another change, before the move could begin.
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
    /// `seen`, when given, is the project the asker saw and whether it was a chat
    /// without a project; a move made meanwhile elsewhere refuses the request
    /// instead of moving the chat again. `proposal` is the agent proposal the
    /// user confirmed, kept on the project.
    pub async fn move_to_new_project(
        self: &Arc<Self>,
        session_id: &str,
        name: &str,
        path: &str,
        seen: Option<(&str, bool)>,
        proposal: Option<&str>,
    ) -> Result<Option<LocalProject>> {
        let (name, _) = projects::new_project_place(name, path)?;
        let store = Store::open()?;
        let (project, _, sessions) = scope(&store, session_id)?;
        if seen.is_some_and(|seen| seen != (project.id.as_str(), project.is_scratch())) {
            return Err(anyhow!(CHANGED));
        }
        // A second move of the same files would act on whatever the first left.
        for other in sessions.iter().filter(|id| *id != session_id) {
            if let Some(waiting) = store.chat_project_move(other)? {
                return Err(anyhow!(
                    "A chat sharing these files is already waiting to become \"{}\"; cancel that first.",
                    waiting.name
                ));
            }
        }
        // Every move starts as a waiting one, which the move takes as it records
        // itself, so a crash at any point leaves the request or the move.
        let requested_at = store.set_chat_project_move(
            session_id,
            &name,
            path,
            proposal,
            Some(project.is_scratch()),
        )?;
        drop(store);
        // Set before anything can stop this request, so a later pass finds the move.
        self.project_moves_waiting.store(true, Ordering::SeqCst);
        if !self.idle(&sessions).await? {
            self.emit_sessions(&sessions).await;
            return Ok(None);
        }
        // Its own task, so a request that goes away doesn't let go of the chats mid-move.
        let host = self.clone();
        let id = session_id.to_string();
        let made = tokio::spawn(async move { host.make_project(&id, Some(requested_at)).await })
            .await
            .unwrap_or_else(|error| Err(anyhow!("The move stopped unexpectedly: {error}")));
        match made {
            // A turn started first: wait for it like any request made mid-turn.
            Err(error) if error.to_string() == BUSY => {
                self.emit_sessions(&sessions).await;
                Ok(None)
            }
            moved => moved.map(Some),
        }
    }

    /// Whether there was a waiting move to cancel, the one requested at
    /// `requested_at` when given; `false` once it has started or been replaced.
    pub async fn cancel_project_move(
        &self,
        session_id: &str,
        requested_at: Option<i64>,
    ) -> Result<bool> {
        let cancelled = Store::open()?
            .take_chat_project_move(session_id, requested_at)?
            .is_some();
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
            let (source, sessions) = match scope(&store, &session_id) {
                Ok((source, _, sessions)) => (source, sessions),
                Err(_) if matches!(store.get_chat_session(&session_id), Ok(None)) => {
                    let _ = store.take_chat_project_move(&session_id, None);
                    continue;
                }
                Err(error) => {
                    self.project_moves_waiting.store(true, Ordering::SeqCst);
                    eprintln!("orx up: could not read the chats moving with {session_id}: {error}");
                    continue;
                }
            };
            drop(store);
            if !self.idle(&sessions).await.unwrap_or(false) {
                self.project_moves_waiting.store(true, Ordering::SeqCst);
                continue;
            }
            let note = match self.make_project(&session_id, None).await {
                Ok(project) => {
                    // A chat split from a project leaves its experiments there.
                    let experiments = if project.id == source.id {
                        String::new()
                    } else {
                        format!(
                            " Experiments run before the move stay in \"{}\", so new ones here can't build on them as parents.",
                            source.name
                        )
                    };
                    format!(
                        "The user made this chat the project \"{}\", at {}, and this chat now belongs to it. Your worktree keeps every file, uncommitted ones included; the project folder holds what you have committed.{experiments} Continue from where you left off.",
                        project.name, project.repo_path
                    )
                }
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

    /// Carries out the chat's waiting move once its chats can be held; with
    /// `requested_at`, only that request, not one that replaced it.
    async fn make_project(
        self: &Arc<Self>,
        session_id: &str,
        requested_at: Option<i64>,
    ) -> Result<LocalProject> {
        let (guarded, _, checked) = scope(&Store::open()?, session_id)?;
        // Held until the store names the new project; another change waits its turn like a busy chat.
        let _changing = match self.project_change.get() {
            Some(begin) => Some(begin(&guarded.id).ok_or_else(|| anyhow!(BUSY))?),
            None => None,
        };
        // Each chat whose files move is held until they are in place, in a state
        // Stop leaves alone; a message sent meanwhile is refused until it ends.
        let mut reserved = Vec::new();
        for id in &checked {
            let reservation = TurnGuard::claim_idle(self, id, TurnState::Switching);
            reserved.push(reservation.await.ok_or_else(|| anyhow!(BUSY))?);
        }
        // Stopped so their next turn starts with the new project's playbook. A Claude
        // child still owing a reply for background work would lose it, so the move waits.
        for id in &checked {
            if self.claude.stop_idle(id).await.is_err() {
                return Err(anyhow!(BUSY));
            }
            self.opencode.kill_session(id).await;
            self.codex.kill_session(id).await;
            self.acp.stop(id).await;
        }
        let moving = {
            let (session_id, checked) = (session_id.to_string(), checked.clone());
            let guarded = guarded.id.clone();
            tokio::task::spawn_blocking(move || -> Result<(LocalProject, Vec<String>)> {
                let store = Store::open()?;
                let request = store
                    .chat_project_move(&session_id)?
                    .filter(|request| requested_at.is_none_or(|at| at == request.requested_at))
                    .ok_or_else(|| anyhow!(CANCELLED))?;
                let moved = (|| {
                    // Read again: another move may have just changed it.
                    let (project, owner, sessions) = scope(&store, &session_id)?;
                    let unchanged = project.id == guarded
                        && sessions.len() == checked.len()
                        && sessions.iter().all(|id| checked.contains(id));
                    if !unchanged {
                        return Err(anyhow!("This chat just moved; try again."));
                    }
                    if request
                        .scratch
                        .is_some_and(|scratch| scratch != project.is_scratch())
                    {
                        return Err(anyhow!(CHANGED));
                    }
                    let created = if project.is_scratch() {
                        let worktree =
                            crate::local::git::existing_session_worktree_path(&project, &owner);
                        projects::promote_scratch_project(
                            &store,
                            &project,
                            &session_id,
                            Some(&worktree),
                            &request,
                        )
                    } else {
                        projects::split_chat_project(
                            &store,
                            &project,
                            &session_id,
                            &owner,
                            &sessions,
                            &request,
                        )
                    }?;
                    Ok((created, sessions))
                })();
                // A move refused before it took its request takes it now, so it isn't tried again.
                if moved
                    .as_ref()
                    .is_err_and(|error| error.to_string() != CANCELLED)
                {
                    if let Err(error) =
                        store.take_chat_project_move(&session_id, Some(request.requested_at))
                    {
                        eprintln!(
                            "orx up: could not drop the failed move of chat {session_id}: {error}"
                        );
                    }
                }
                moved
            })
        };
        let moved = moving
            .await
            .unwrap_or_else(|error| Err(anyhow!("The move stopped unexpectedly: {error}")));
        for reservation in &mut reserved {
            reservation.release().await;
        }
        let (created, sessions) = match moved {
            Err(error) => {
                self.emit_sessions(&checked).await;
                return Err(error);
            }
            Ok(moved) => moved,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    use super::*;

    struct Held(Arc<AtomicUsize>);

    impl Drop for Held {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// A host whose projects are busy with another change while `busy` holds,
    /// and the number of changes it holds open.
    fn host() -> (Arc<ChatHost>, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let host = Arc::new(ChatHost::new(
            Arc::new(crate::local::opencode::AgentHost::new(None)),
            Arc::new(crate::local::codex::CodexHost::new()),
            Arc::new(crate::local::claude::ClaudeHost::new()),
        ));
        let busy = Arc::new(AtomicBool::new(true));
        let held = Arc::new(AtomicUsize::new(0));
        host.set_project_change_guard(Box::new({
            let (busy, held) = (busy.clone(), held.clone());
            move |_| {
                if busy.load(Ordering::SeqCst) {
                    return None;
                }
                held.fetch_add(1, Ordering::SeqCst);
                Some(Box::new(Held(held.clone())))
            }
        }));
        (host, busy, held)
    }

    /// A chat without a project, with `chats` chats in it.
    fn scratch_chats(store: &Store, chats: usize) -> (LocalProject, Vec<String>) {
        // `scratch_project` reuses one no chat uses yet, so tests take turns claiming it.
        static CLAIMING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _claiming = CLAIMING.lock().unwrap();
        let scratch = projects::scratch_project(store).unwrap();
        let sessions = (0..chats)
            .map(|_| {
                let session = format!("ses_{}", uuid::Uuid::new_v4());
                crate::local::chat::steering_tests::add_chat_session(store, &session);
                store.conn_execute_for_tests(&format!(
                    "UPDATE chat_sessions SET project_id = '{}' WHERE id = '{session}'",
                    scratch.id
                ));
                session
            })
            .collect();
        (scratch, sessions)
    }

    fn target() -> PathBuf {
        std::env::temp_dir()
            .join(format!("orx-project-move-{}", uuid::Uuid::new_v4()))
            .join("probes")
    }

    #[tokio::test]
    async fn a_move_waits_while_the_project_is_busy_and_runs_once_it_is_free() {
        let (host, busy, held) = host();
        let store = Store::open().unwrap();
        let (scratch, sessions) = scratch_chats(&store, 1);
        let session = &sessions[0];
        let target = target();
        let path = target.to_str().unwrap();

        let waiting = host
            .move_to_new_project(session, "Probes", path, Some((&scratch.id, true)), None)
            .await
            .unwrap();

        assert!(waiting.is_none());
        let waiting = store.chat_project_move(session).unwrap().unwrap();
        assert_eq!(
            (waiting.name.as_str(), waiting.path.as_str()),
            ("Probes", path)
        );
        assert!(store
            .get_local_project(&scratch.id)
            .unwrap()
            .unwrap()
            .is_scratch());
        assert!(!target.exists());

        busy.store(false, Ordering::SeqCst);
        let created = host
            .move_to_new_project(session, "Probes", path, Some((&scratch.id, true)), None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(created.id, scratch.id);
        assert!(!created.is_scratch());
        assert!(target.join(".git").is_dir());
        assert_eq!(held.load(Ordering::SeqCst), 0);
        assert_eq!(store.chat_project_move(session).unwrap(), None);
        let _ = std::fs::remove_dir_all(target.parent().unwrap());
    }

    #[tokio::test]
    async fn a_move_refuses_requests_made_against_what_has_changed() {
        let (host, _busy, _held) = host();
        let store = Store::open().unwrap();
        let (scratch, sessions) = scratch_chats(&store, 2);
        let path = target();
        let path = path.to_str().unwrap();

        // A dialog opened before the chat became a project asks for the other kind of move.
        let stale = host
            .move_to_new_project(
                &sessions[0],
                "Probes",
                path,
                Some((&scratch.id, false)),
                None,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(stale.to_string(), CHANGED);
        let elsewhere = host
            .move_to_new_project(&sessions[0], "Probes", path, Some(("other", true)), None)
            .await
            .err()
            .unwrap();
        assert_eq!(elsewhere.to_string(), CHANGED);

        host.move_to_new_project(
            &sessions[0],
            "Probes",
            path,
            Some((&scratch.id, true)),
            None,
        )
        .await
        .unwrap();
        let requested_at = store
            .chat_project_move(&sessions[0])
            .unwrap()
            .unwrap()
            .requested_at;

        // The files already wait to move with the first chat.
        let second = host
            .move_to_new_project(&sessions[1], "Other", path, Some((&scratch.id, true)), None)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            second.contains("already waiting to become \"Probes\""),
            "{second}"
        );
        assert_eq!(store.chat_project_move(&sessions[1]).unwrap(), None);

        // Requests made faster than the clock ticks, or after one was cancelled,
        // each name only themselves, so a stale cancel can't call off the newer one.
        let replaced = store
            .set_chat_project_move(&sessions[0], "Probes", path, None, Some(true))
            .unwrap();
        assert!(host
            .cancel_project_move(&sessions[0], Some(replaced))
            .await
            .unwrap());
        let again = store
            .set_chat_project_move(&sessions[0], "Probes", path, None, Some(true))
            .unwrap();
        assert!(requested_at < replaced && replaced < again);
        for stale in [requested_at, replaced] {
            assert!(!host
                .cancel_project_move(&sessions[0], Some(stale))
                .await
                .unwrap());
        }
        assert!(host
            .cancel_project_move(&sessions[0], Some(again))
            .await
            .unwrap());
        assert_eq!(store.chat_project_move(&sessions[0]).unwrap(), None);
        assert!(store
            .get_local_project(&scratch.id)
            .unwrap()
            .unwrap()
            .is_scratch());
    }

    #[tokio::test]
    async fn a_waiting_move_keeps_the_proposal_it_confirmed() {
        let (host, busy, _held) = host();
        let store = Store::open().unwrap();
        let (scratch, sessions) = scratch_chats(&store, 1);
        let session = &sessions[0];
        let target = target();
        let path = target.to_str().unwrap();

        host.move_to_new_project(
            session,
            "Probes",
            path,
            Some((&scratch.id, true)),
            Some("toolu_create"),
        )
        .await
        .unwrap();

        let waiting = store.chat_project_move(session).unwrap().unwrap();
        assert_eq!(waiting.proposal.as_deref(), Some("toolu_create"));
        let chat = store.get_chat_session(session).unwrap().unwrap();
        let shown = crate::local::chat::session_json_with(Some(&store), &chat, false);
        assert_eq!(shown["pendingProject"]["proposal"], "toolu_create");

        busy.store(false, Ordering::SeqCst);
        let created = host.make_project(session, None).await.unwrap();

        assert_eq!(created.id, scratch.id);
        assert_eq!(created.proposal.as_deref(), Some("toolu_create"));
        assert_eq!(
            store
                .get_local_project(&scratch.id)
                .unwrap()
                .unwrap()
                .proposal,
            created.proposal
        );
        let _ = std::fs::remove_dir_all(target.parent().unwrap());
    }
}
