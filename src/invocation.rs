//! How to spell "run orx" in a hint the reader can actually paste.
//!
//! Plain `orx` for anyone who installed the CLI, and for agents the app spawns
//! (`chat::prepare_env` puts the bundle's `orx` first on their PATH). For a
//! human driving the macOS bundle directly there is no `orx` anywhere on PATH,
//! and a hint saying `orx exp wait …` is a command that cannot be run.

use std::sync::OnceLock;

static ORX: OnceLock<String> = OnceLock::new();

/// The command to print in hints: `orx` when that resolves, else this binary's
/// own path, shell-quoted if it needs it.
pub fn orx() -> &'static str {
    ORX.get_or_init(|| {
        if resolves_on_path("orx") {
            return "orx".to_string();
        }
        match crate::paths::spawnable_exe() {
            // Not canonicalized: invoked through the bundle's `orx` alias, that
            // alias is the friendlier thing to echo back.
            Ok(exe) => quote_for_shell(&exe.to_string_lossy()),
            Err(_) => "orx".to_string(),
        }
    })
}

/// The "what to run next" line every backend prints after launching a run.
/// What to do after a launch: an agent in a chat ends its turn, since the result comes to it.
pub fn follow_up(experiment_id: &str, run_id: &str, in_chat: bool) -> String {
    let orx = orx();
    if in_chat {
        format!(
            "  Its result arrives in this chat as a message when it ends. Check its first minutes once \
             with `{orx} exp view check {run_id}`, then end your turn. After its first data arrives, \
             read `{orx} exp view show {run_id}` to inspect what its card shows. Never wait or poll for it \
             (no sleep loops over `{orx} runs`, no `{orx} exp wait`)."
        )
    } else {
        format!("  Follow it with `{orx} exp wait {experiment_id}` or `{orx} logs {run_id}`.")
    }
}

/// The error every backend raises when neither the experiment nor its project
/// has a run command.
pub fn no_run_command(project_id: &str) -> String {
    let orx = orx();
    format!(
        "No run command set for this experiment or its project. Set the project default with \
         `{orx} project edit {project_id} --run-command '<cmd>'`, or pass `--run-command '<cmd>'` \
         to `{orx} create-experiment` — then relaunch."
    )
}

fn resolves_on_path(bin: &str) -> bool {
    crate::local::shell_env::find_on_path(bin).is_some()
}

fn quote_for_shell(path: &str) -> String {
    if path.is_empty() || path.contains(|c: char| c.is_whitespace() || c == '\'') {
        format!("'{}'", path.replace('\'', r"'\''"))
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_path_is_left_alone() {
        assert_eq!(
            quote_for_shell("/Applications/OpenResearch.app/Contents/MacOS/orx"),
            "/Applications/OpenResearch.app/Contents/MacOS/orx"
        );
    }

    #[test]
    fn a_path_needing_escaping_is_quoted() {
        assert_eq!(
            quote_for_shell("/My Apps/OpenResearch.app/MacOS/orx"),
            "'/My Apps/OpenResearch.app/MacOS/orx'"
        );
        assert_eq!(quote_for_shell("/it's/orx"), r"'/it'\''s/orx'");
    }

    #[test]
    fn the_hint_uses_one_spelling_throughout() {
        let orx = orx();
        assert_eq!(
            follow_up("exp_1", "run_1", false),
            format!("  Follow it with `{orx} exp wait exp_1` or `{orx} logs run_1`.")
        );
        let agent = follow_up("exp_1", "run_1", true);
        assert!(
            agent.contains(&format!("`{orx} exp view check run_1`")),
            "{agent}"
        );
        assert!(agent.contains("end your turn"), "{agent}");
    }
}
