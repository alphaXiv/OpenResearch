//! Direct-human removal of the local SQLite database and/or CLI executable.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};
use crate::{DeleteArgs, DeleteCommand};

const CONFIRM_DATABASE: &str = "DELETE DATABASE";
const CONFIRM_CLI: &str = "DELETE CLI";
const CONFIRM_ALL: &str = "DELETE ALL";
const AGENT_SESSION_ENV_VARS: &[&str] = &[
    crate::local::chat::CHAT_HARNESS_ENV,
    crate::local::chat::CHAT_SESSION_ENV,
    crate::local::chat::LOCAL_SESSION_ENV,
    "CODEX_THREAD_ID",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "OPENCODE",
    "CURSOR_AGENT",
];

pub async fn run(args: DeleteArgs) -> Result<()> {
    require_direct_human_session(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        agent_session_present(),
    )?;

    let confirmed_targets = DeleteTargets::resolve(args.command)?;
    confirmed_targets.print();
    confirm(args.command)?;

    let mut lifecycle_lock = crate::store::open_lifecycle_lock()?;
    let _lifecycle_guard = lifecycle_lock.try_write().map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            anyhow!(
                "Another OpenResearch process is running. Quit the OpenResearch app, close `orx up` and `orx serve`, and end any active runs before trying again."
            )
        } else {
            anyhow!("Could not lock OpenResearch for deletion: {error}")
        }
    })?;
    let targets = DeleteTargets::resolve(args.command)?;
    if targets != confirmed_targets {
        return Err(anyhow!(
            "The deletion targets changed while awaiting confirmation. Review them and run the command again."
        ));
    }

    if let Some(database) = &targets.database {
        remove_database(database)?;
        println!("✓ Deleted {} and its SQLite sidecars.", database.display());
    }
    if let Some(executable) = &targets.executable {
        std::fs::remove_file(executable)
            .map_err(|error| anyhow!("Could not delete {}: {error}", executable.display()))?;
        println!("✓ Deleted the orx CLI at {}.", executable.display());
        if let Some(receipt) = &targets.receipt {
            if let Err(error) = remove_if_exists(receipt) {
                eprintln!(
                    "Warning: the CLI was deleted, but its installer receipt remains: {error}"
                );
            }
        }
    }
    Ok(())
}

fn agent_session_present() -> bool {
    AGENT_SESSION_ENV_VARS
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}

fn require_direct_human_session(terminal: bool, agent_session: bool) -> Result<()> {
    if agent_session {
        return Err(anyhow!(
            "`orx delete` cannot run from an OpenResearch agent session. Run it directly in your terminal."
        ));
    }
    if !terminal {
        return Err(anyhow!(
            "`orx delete` requires an interactive terminal and cannot be piped."
        ));
    }
    Ok(())
}

fn confirm(command: DeleteCommand) -> Result<()> {
    let phrase = match command {
        DeleteCommand::Database => CONFIRM_DATABASE,
        DeleteCommand::Cli => CONFIRM_CLI,
        DeleteCommand::All => CONFIRM_ALL,
    };
    print!("Type {phrase} to continue: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if answer.trim_end() != phrase {
        return Err(anyhow!("Confirmation did not match; nothing was deleted."));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct DeleteTargets {
    database: Option<PathBuf>,
    executable: Option<PathBuf>,
    receipt: Option<PathBuf>,
}

impl DeleteTargets {
    fn resolve(command: DeleteCommand) -> Result<Self> {
        let delete_database = matches!(command, DeleteCommand::Database | DeleteCommand::All);
        let delete_cli = matches!(command, DeleteCommand::Cli | DeleteCommand::All);
        let executable = delete_cli
            .then(std::env::current_exe)
            .transpose()?
            .map(|path| path.canonicalize().unwrap_or(path));
        // Canonicalizing means `orx delete --cli` run through the app's PATH
        // link (see `orx install-cli`) resolves to the bundle's executable —
        // deleting it would gut the app the user is running, from a command
        // they think removes a CLI.
        if let Some(executable) = &executable {
            if let Ok(crate::updates::InstallChannel::AppBundle(root)) =
                crate::updates::detect_channel(executable)
            {
                return Err(anyhow!(
                    "This `orx` is the OpenResearch app's binary ({}).\n\
                     Deleting it would break the app, so `orx delete` won't.\n\
                     To remove the app, drag {} to the Trash. `orx delete database` \
                     still works.",
                    executable.display(),
                    root.display()
                ));
            }
        }
        let receipt = match &executable {
            Some(executable) => match crate::updates::load_receipt() {
                Ok(receipt) => receipt.and_then(|receipt| {
                    let prefix = PathBuf::from(receipt.install_prefix);
                    let prefix = prefix.canonicalize().unwrap_or(prefix);
                    crate::updates::exe_matches_prefix(executable, &prefix)
                        .then(crate::updates::receipt_path)
                }),
                Err(error) => {
                    eprintln!("Warning: could not read the installer receipt: {error}");
                    None
                }
            },
            None => None,
        };
        Ok(Self {
            database: delete_database.then(|| crate::store::data_dir().join("orx.db")),
            executable,
            receipt,
        })
    }

    fn print(&self) {
        println!("This permanently deletes:");
        if let Some(database) = &self.database {
            println!("  Database: {}", database.display());
            println!("  External project folders will not be deleted.");
        }
        if let Some(executable) = &self.executable {
            println!("  CLI executable: {}", executable.display());
        }
        if let Some(receipt) = &self.receipt {
            println!("  Installer receipt: {}", receipt.display());
        }
    }
}

fn remove_database(database: &Path) -> Result<()> {
    remove_if_exists(database)?;
    remove_if_exists(&database.with_extension("db-wal"))?;
    remove_if_exists(&database.with_extension("db-shm"))?;
    remove_if_exists(&database.with_extension("db-journal"))?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow!("Could not delete {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_human_guard_rejects_agents_and_non_terminals() {
        assert!(require_direct_human_session(true, false).is_ok());
        assert!(require_direct_human_session(false, false).is_err());
        assert!(require_direct_human_session(true, true).is_err());
        for marker in ["CODEX_THREAD_ID", "CLAUDECODE", "OPENCODE", "CURSOR_AGENT"] {
            assert!(AGENT_SESSION_ENV_VARS.contains(&marker));
        }
    }

    #[test]
    fn database_removal_never_touches_sibling_data() {
        let root = std::env::temp_dir().join(format!("orx-delete-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("files")).unwrap();
        for name in ["orx.db", "orx.db-wal", "orx.db-shm", "orx.db-journal"] {
            std::fs::write(root.join(name), name).unwrap();
        }
        std::fs::write(root.join("files/result.txt"), "keep").unwrap();

        remove_database(&root.join("orx.db")).unwrap();

        assert!(!root.join("orx.db").exists());
        assert!(!root.join("orx.db-wal").exists());
        assert!(!root.join("orx.db-shm").exists());
        assert!(!root.join("orx.db-journal").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("files/result.txt")).unwrap(),
            "keep"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn database_deletion_lock_rejects_shared_users() {
        let root = std::env::temp_dir().join(format!("orx-delete-lock-{}", uuid::Uuid::new_v4()));
        let path = root.join("database.lock");
        let shared = crate::store::open_lifecycle_lock_at(&path).unwrap();
        let shared_guard = shared.read().unwrap();
        let mut exclusive = crate::store::open_lifecycle_lock_at(&path).unwrap();

        assert_eq!(
            exclusive.try_write().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(shared_guard);
        drop(exclusive.try_write().unwrap());
        drop(exclusive);
        drop(shared);
        std::fs::remove_dir_all(root).unwrap();
    }
}
