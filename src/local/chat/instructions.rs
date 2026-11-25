//! The user's OpenResearch instructions: one file in the config directory that
//! orx appends to every agent's playbook. Agents change it only through a card
//! the user approves.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::{
    ChatHost, OrxDisplay, OrxKind, OrxSource, PromptAnswer, TurnOverrides, WirePrompt,
    ORX_MESSAGE_PREFIX,
};
use crate::error::{anyhow, bail, Context, Result};
use crate::store::StoredChatSession;

pub fn path() -> PathBuf {
    let dir = crate::config::config_dir();
    // Each test thread gets its own file, so one test's instructions never reach another's turns.
    #[cfg(test)]
    let dir = dir.join(
        std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .replace("::", "-"),
    );
    dir.join("AGENTS.md")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub content: String,
    pub version: String,
    pub exists: bool,
}

pub fn version(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// The file as it is; a missing file reads as empty.
pub fn read(path: &Path) -> Result<Snapshot> {
    let (content, exists) = match std::fs::read_to_string(path) {
        Ok(content) => (content, true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(error) => bail!(
            "Couldn't read your OpenResearch instructions at {}: {error}. Fix the file, or delete it to start over.",
            path.display()
        ),
    };
    Ok(Snapshot {
        version: version(&content),
        content,
        exists,
    })
}

/// Saves `content` if the file is still at `base_version`; otherwise returns the file as it is now.
pub fn write(
    path: &Path,
    content: &str,
    base_version: &str,
) -> Result<std::result::Result<Snapshot, Snapshot>> {
    // ponytail: guards writers in this process only; an editor saving at the same instant can still race.
    static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _write = WRITE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let current = read(path)?;
    // Already saved, as when an approval is answered again after the agent couldn't be told.
    if current.exists && current.content == content {
        return Ok(Ok(current));
    }
    if current.version != base_version {
        return Ok(Err(current));
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Couldn't create {}", parent.display()))?;
    // Writes through a symlink, so instructions kept with the user's dotfiles stay there.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    crate::local::git::atomic_write_with_mode(&target, content.as_bytes(), None).with_context(
        || {
            format!(
                "Couldn't save your OpenResearch instructions to {}",
                target.display()
            )
        },
    )?;
    Ok(Ok(Snapshot {
        version: version(content),
        content: content.to_string(),
        exists: true,
    }))
}

/// The change as `git diff` shows it, every line in context, for the card.
fn diff(old: &str, new: &str) -> Result<String> {
    let dir = crate::local::git::TemporaryDirectory::new("orx-instructions-diff")?;
    for (side, content) in [("a", old), ("b", new)] {
        std::fs::create_dir(dir.path().join(side))?;
        std::fs::write(dir.path().join(side).join("AGENTS.md"), content)?;
    }
    let out = crate::local::git::git_command()
        .current_dir(dir.path())
        .args([
            "diff",
            "--no-index",
            "--no-color",
            "--no-ext-diff",
            "--no-prefix",
        ])
        .args(["--unified=100000", "a/AGENTS.md", "b/AGENTS.md"])
        .output()
        .context("Couldn't run git to show the proposed change")?;
    // `--no-index` exits 1 when the files differ.
    if out.status.code() != Some(1) {
        bail!(
            "git couldn't compare the proposed instructions: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl ChatHost {
    /// Shows the user a card offering `content` as their new instructions. Returns what the agent is told.
    pub async fn propose_instructions(
        self: &Arc<Self>,
        session_id: &str,
        content: String,
    ) -> Result<String> {
        let current = read(&path())?;
        if current.content == content {
            return Ok("Your draft matches the user's OpenResearch instructions, so there is nothing to propose.".into());
        }
        let prompt = WirePrompt {
            kind: "instructions".into(),
            diff: Some(diff(&current.content, &content)?),
            instructions: Some(content),
            instructions_version: Some(current.version),
            ..Default::default()
        };
        let prompt_id = format!("instructions_{}", uuid::Uuid::new_v4());
        self.post_card(session_id, &prompt_id, prompt).await?;
        Ok("Your proposed OpenResearch instructions are waiting for the user on a card in this chat. \
            Nothing has changed yet, so don't say it has. A message here will tell you when they approve or decline it."
            .into())
    }

    /// Saves an approved proposal, resolves its card and tells the agent the outcome.
    pub(super) async fn answer_instructions(
        self: &Arc<Self>,
        session: StoredChatSession,
        prompt: WirePrompt,
        req: PromptAnswer,
    ) -> Result<()> {
        if req.from.is_some() {
            bail!("Only the user can change their OpenResearch instructions.");
        }
        let told = if req.approve {
            let content = prompt.instructions.unwrap_or_default();
            let base = prompt.instructions_version.unwrap_or_default();
            let file = path();
            let written = tokio::task::spawn_blocking(move || write(&file, &content, &base))
                .await
                .context("Couldn't save your OpenResearch instructions")??;
            if written.is_err() {
                bail!("Your OpenResearch instructions changed after the agent proposed this, so approving it would undo that change. Decline it and ask the agent to propose again.");
            }
            "The user approved your proposed OpenResearch instructions, and they are saved. New OpenResearch chats start with them; chats already open, this one included, keep the instructions they started with."
        } else {
            "The user declined your proposed change to their OpenResearch instructions, so they stay as they were."
        };
        // The card stays open until the agent is told, so a failed send can be answered again.
        self.send_message(
            &session.id,
            format!("{ORX_MESSAGE_PREFIX}{told}"),
            TurnOverrides::default(),
            Vec::new(),
            Vec::new(),
            None,
            Some(OrxDisplay {
                text: told.into(),
                source: OrxSource {
                    kind: OrxKind::Message,
                    from: None,
                    status: None,
                },
            }),
        )
        .await
        .context("The agent couldn't be told your answer. Answer the card again to retry")?;
        self.resolve_prompt_card(&req);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::chat::{TurnState, WirePart};
    use crate::store::Store;

    fn temp_path() -> PathBuf {
        crate::config::test_home()
            .join(format!("instructions-{}", uuid::Uuid::new_v4().simple()))
            .join("AGENTS.md")
    }

    #[test]
    fn a_write_against_a_stale_version_returns_the_newer_file() {
        let path = temp_path();
        let base = read(&path).unwrap();
        let saved = write(&path, "first", &base.version).unwrap().unwrap();
        assert_eq!(read(&path).unwrap(), saved);

        let stale = write(&path, "second", &base.version).unwrap().unwrap_err();
        assert_eq!(stale.content, "first");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        // Answering an approval again after it was saved succeeds without a second write.
        assert_eq!(
            write(&path, "first", &base.version).unwrap().unwrap(),
            saved
        );

        assert!(write(&path, "", &saved.version).unwrap().is_ok());
        assert_eq!(read(&path).unwrap().version, version(""));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_names_itself_rather_than_reading_as_empty() {
        let path = temp_path();
        std::fs::create_dir_all(&path).unwrap();
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    #[test]
    fn the_diff_marks_only_changed_lines() {
        let changed = diff("a\nb\nc\n", "a\nB\nc\nd\n").unwrap();
        assert!(
            changed.ends_with("@@ -1,3 +1,4 @@\n a\n-b\n+B\n c\n+d\n"),
            "{changed}"
        );
        let created = diff("", "new\n").unwrap();
        assert!(created.ends_with("@@ -0,0 +1 @@\n+new\n"), "{created}");
    }

    fn host() -> Arc<ChatHost> {
        Arc::new(ChatHost::new(
            Arc::new(crate::local::opencode::AgentHost::new(None)),
            Arc::new(crate::local::codex::CodexHost::new()),
            Arc::new(crate::local::claude::ClaudeHost::new()),
        ))
    }

    /// A chat whose agent is mid-turn, so the outcome message queues instead of starting an agent.
    async fn busy_chat(host: &ChatHost) -> StoredChatSession {
        let store = Store::open().unwrap();
        let project = format!("proj_{}", uuid::Uuid::new_v4().simple());
        store
            .create_local_project(&crate::store::tests::local_project_fixture(&project))
            .unwrap();
        let session = StoredChatSession {
            project_id: project,
            ..crate::store::tests::chat_session_fixture(&format!(
                "chat_{}",
                uuid::Uuid::new_v4().simple()
            ))
        };
        store.create_chat_session(&session).unwrap();
        host.turns
            .lock()
            .await
            .insert(session.id.clone(), TurnState::Reserved { turn_id: None });
        session
    }

    fn card(session: &StoredChatSession) -> (String, WirePrompt) {
        Store::open()
            .unwrap()
            .list_chat_messages(&session.id)
            .unwrap()
            .iter()
            .find_map(|message| {
                let parts: Vec<WirePart> = serde_json::from_str(&message.parts_json).ok()?;
                parts
                    .into_iter()
                    .find(|part| {
                        part.prompt
                            .as_ref()
                            .is_some_and(|p| p.kind == "instructions")
                    })
                    .map(|part| (part.id, part.prompt.unwrap()))
            })
            .unwrap()
    }

    fn answer(session: &StoredChatSession, prompt_id: &str, approve: bool) -> PromptAnswer {
        PromptAnswer {
            session_id: session.id.clone(),
            prompt_id: prompt_id.into(),
            approve,
            resume_mode: None,
            answers: Vec::new(),
            note: None,
            annotations: Vec::new(),
            from: None,
            env: Vec::new(),
        }
    }

    fn told(host: &ChatHost, session: &StoredChatSession) -> Vec<String> {
        host.queued_items(&session.id)
            .iter()
            .filter_map(|item| item["text"].as_str().map(str::to_string))
            .collect()
    }

    #[tokio::test]
    async fn a_proposal_writes_only_once_approved_against_the_file_it_was_made_from() {
        let host = host();
        let file = path();
        let _ = std::fs::remove_file(&file);

        let session = busy_chat(&host).await;
        let said = host
            .propose_instructions(&session.id, "Name runs after the idea.\n".into())
            .await
            .unwrap();
        assert!(said.contains("waiting for the user"), "{said}");
        assert!(!file.exists(), "proposing writes nothing");
        let (prompt_id, prompt) = card(&session);
        assert!(prompt.diff.unwrap().contains("+Name runs after the idea."));
        assert!(prompt.native_id.is_none(), "the card outlives the turn");

        host.respond(answer(&session, &prompt_id, true))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Name runs after the idea.\n"
        );
        assert_eq!(card(&session).1.approved, Some(true));
        assert!(told(&host, &session)[0].contains("approved"));

        let declined = busy_chat(&host).await;
        host.propose_instructions(&declined.id, "Something else.".into())
            .await
            .unwrap();
        let (prompt_id, _) = card(&declined);
        host.respond(answer(&declined, &prompt_id, false))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Name runs after the idea.\n"
        );
        assert_eq!(card(&declined).1.approved, Some(false));
        assert!(told(&host, &declined)[0].contains("declined"));

        let stale = busy_chat(&host).await;
        host.propose_instructions(&stale.id, "From the agent.".into())
            .await
            .unwrap();
        std::fs::write(&file, "Edited by the user.").unwrap();
        let (prompt_id, _) = card(&stale);
        let error = host
            .respond(answer(&stale, &prompt_id, true))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("changed after"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Edited by the user."
        );
        assert!(!card(&stale).1.resolved, "the user can still decline it");
        assert!(told(&host, &stale).is_empty());

        let mut for_user = answer(&stale, &prompt_id, true);
        for_user.from = Some("chat_other".into());
        assert!(host.respond(for_user).await.is_err());

        let same = busy_chat(&host).await;
        let said = host
            .propose_instructions(&same.id, "Edited by the user.".into())
            .await
            .unwrap();
        assert!(said.contains("nothing to propose"), "{said}");
        let _ = std::fs::remove_file(&file);
    }
}
