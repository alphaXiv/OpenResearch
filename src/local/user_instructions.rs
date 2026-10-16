//! User-authored instructions shared by every OpenResearch project on this host.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{anyhow, Context, Result};

#[derive(Debug, Serialize)]
pub(crate) struct InstructionsFile {
    path: PathBuf,
    content: String,
    // Missing and empty files have different revisions, so deletion also
    // invalidates an editor's snapshot.
    revision: Option<String>,
}

#[derive(Debug)]
pub(crate) struct InstructionsChanged;

impl std::fmt::Display for InstructionsChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AGENTS.md changed on disk. Reload it before saving; your draft has been kept.")
    }
}

impl std::error::Error for InstructionsChanged {}

pub(crate) fn path() -> PathBuf {
    crate::config::config_dir().join("AGENTS.md")
}

/// Read on demand, so edits are picked up without restarting OpenResearch.
pub(crate) async fn load() -> Result<Option<String>> {
    read(&path()).await
}

async fn read(path: &Path) -> Result<Option<String>> {
    Ok(read_content(tokio::fs::read_to_string(path).await, path)?
        .filter(|content| !content.trim().is_empty()))
}

fn read_content(result: std::io::Result<String>, path: &Path) -> Result<Option<String>> {
    match result {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("Could not read global instructions at {}", path.display())),
    }
}

fn snapshot(path: &Path, content: Option<String>) -> InstructionsFile {
    let revision = content
        .as_ref()
        .map(|content| format!("{:x}", Sha256::digest(content.as_bytes())));
    InstructionsFile {
        path: path.to_path_buf(),
        content: content.unwrap_or_default(),
        revision,
    }
}

pub(crate) async fn inspect() -> Result<InstructionsFile> {
    let path = path();
    let content = read_content(tokio::fs::read_to_string(&path).await, &path)?;
    Ok(snapshot(&path, content))
}

pub(crate) fn save(content: &str, expected_revision: Option<&str>) -> Result<InstructionsFile> {
    save_at(&path(), content, expected_revision)
}

fn save_at(
    path: &Path,
    content: &str,
    expected_revision: Option<&str>,
) -> Result<InstructionsFile> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("AGENTS.md has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(".AGENTS.md.lock"))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write()?;
    let current = snapshot(path, read_content(std::fs::read_to_string(path), path)?);
    if current.revision.as_deref() != expected_revision {
        return Err(anyhow!(InstructionsChanged));
    }
    crate::local::git::atomic_write_with_mode(path, content.as_bytes(), Some(0o600))
        .with_context(|| format!("Could not save global instructions at {}", path.display()))?;
    Ok(snapshot(path, Some(content.to_string())))
}

pub(crate) fn prepend(instructions: Option<&str>, text: String) -> String {
    match instructions.filter(|content| !content.trim().is_empty()) {
        Some(content) => {
            format!("<orx-user-instructions>\n{content}\n</orx-user-instructions>\n\n{text}")
        }
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InstructionsDir(PathBuf);

    impl InstructionsDir {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("orx-user-instructions-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("AGENTS.md")
        }
    }

    impl Drop for InstructionsDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn missing_and_blank_files_add_no_instructions() {
        let dir = InstructionsDir::new();
        assert_eq!(read(&dir.file()).await.unwrap(), None);
        assert!(!dir.file().exists());
        for content in ["", " \n\t\r\n"] {
            std::fs::write(dir.file(), content).unwrap();
            assert_eq!(read(&dir.file()).await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn every_read_picks_up_edits_and_removal() {
        let dir = InstructionsDir::new();
        for content in ["# Rules\n\nUse free models.\n", "Отвечай по-русски.\n"] {
            std::fs::write(dir.file(), content).unwrap();
            assert_eq!(read(&dir.file()).await.unwrap().as_deref(), Some(content));
        }
        std::fs::remove_file(dir.file()).unwrap();
        assert_eq!(read(&dir.file()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn invalid_utf8_and_unreadable_paths_report_the_source() {
        let dir = InstructionsDir::new();
        std::fs::write(dir.file(), [0xff]).unwrap();
        let error = read(&dir.file()).await.unwrap_err().to_string();
        assert!(error.contains(dir.file().to_str().unwrap()));
        assert!(error.contains("global instructions"));

        // A directory is unreadable as text on all supported platforms,
        // including when tests run as root and permission bits are ineffective.
        std::fs::remove_file(dir.file()).unwrap();
        std::fs::create_dir(dir.file()).unwrap();
        assert!(read(&dir.file()).await.is_err());
    }

    #[test]
    fn the_block_contains_only_the_user_authored_content() {
        assert_eq!(
            prepend(Some("Use free models."), "continue".into()),
            "<orx-user-instructions>\nUse free models.\n</orx-user-instructions>\n\ncontinue"
        );
        for instructions in [None, Some(""), Some(" \n")] {
            assert_eq!(prepend(instructions, "continue".into()), "continue");
        }
    }

    #[tokio::test]
    async fn editor_creates_the_shared_file_and_preserves_exact_text() {
        let dir = InstructionsDir::new();
        let path = dir.0.join("config/openresearch/AGENTS.md");
        let saved = save_at(&path, "# Правила\n\nUse free models.\n", None).unwrap();
        assert_eq!(saved.path, path);
        assert_eq!(
            read(&path).await.unwrap().as_deref(),
            Some(saved.content.as_str())
        );
        let empty = save_at(&path, " \n", saved.revision.as_deref()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), " \n");
        assert_eq!(empty.content, " \n");
        assert!(empty.revision.is_some());
        assert_eq!(read(&path).await.unwrap(), None);
    }

    #[test]
    fn editor_rejects_external_edits_and_deletion_without_overwriting_them() {
        let dir = InstructionsDir::new();
        let saved = save_at(&dir.file(), "first", None).unwrap();
        std::fs::write(dir.file(), "external edit").unwrap();
        let error = save_at(&dir.file(), "draft", saved.revision.as_deref()).unwrap_err();
        assert!(error.is::<InstructionsChanged>());
        assert_eq!(
            std::fs::read_to_string(dir.file()).unwrap(),
            "external edit"
        );
        std::fs::remove_file(dir.file()).unwrap();
        assert!(save_at(&dir.file(), "draft", saved.revision.as_deref()).is_err());
        assert!(!dir.file().exists());
    }

    #[test]
    fn concurrent_editors_cannot_overwrite_each_other() {
        let dir = InstructionsDir::new();
        let original = save_at(&dir.file(), "original", None).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = ["first editor", "second editor"]
            .into_iter()
            .map(|content| {
                let path = dir.file();
                let revision = original.revision.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    save_at(&path, content, revision.as_deref())
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result
                    .as_ref()
                    .is_err_and(|error| error.is::<InstructionsChanged>()))
                .count(),
            1
        );
        let saved = results.into_iter().find_map(Result::ok).unwrap();
        assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), saved.content);
    }
}
