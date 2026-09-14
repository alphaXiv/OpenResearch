//! The `orx` CLI schema: the top-level command tree plus every subcommand's
//! argument/subcommand types, split by domain. `main.rs` re-exports every
//! public item here at the crate root (`pub use cli::*;`), so the rest of the
//! codebase keeps addressing them as `crate::LoginArgs`, `crate::Command`, etc.
//! — module boundaries here are purely organizational.

mod agent;
mod auth;
mod compute;
mod daemon;
mod discover;
mod experiments;
mod library;
mod projects;
mod system;

pub use agent::*;
pub use auth::*;
pub use compute::*;
pub use daemon::*;
pub use discover::*;
pub use experiments::*;
pub use library::*;
pub use projects::*;
pub use system::*;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "orx",
    about = "OpenResearch CLI",
    version,
    disable_help_subcommand = true
)]
pub struct Cli {
    // Optional so a bare `orx` prints USAGE to stdout and exits 0 (like the TS
    // `if (!command) { console.log(USAGE); return; }`) instead of clap's exit-2.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Disable anonymous usage analytics for this run. To disable it
    /// persistently, run `orx telemetry off`.
    #[arg(long, global = true)]
    pub no_telemetry: bool,
}

#[derive(Subcommand, Debug)]
// NOTE: `local::harness::plan_gate` keeps a hand-maintained allowlist of the
// read-only verbs here (what Claude plan mode may run without approval). When
// you add a *read-only* subcommand, add it there too, or it stays gated in plan
// mode. `readonly_verbs_are_real_commands` catches renames but not additions.
pub enum Command {
    /// Log in via the browser and store a token.
    Login(LoginArgs),

    /// Remove the stored token.
    Logout,

    /// List projects registered in the local orx store.
    Projects(ProjectsArgs),

    /// List organizations available for OpenResearch compute.
    Orgs(OrgsArgs),

    /// Operate on one local project.
    Project(ProjectArgs),

    /// Delegate a task to a second agent session.
    Agent(AgentArgs),

    /// List a project's runs.
    Runs(RunsArgs),

    /// Read a run's terminal log (tail by default).
    Logs(LogsArgs),

    /// Add an experiment node to a local `orx up` project.
    #[command(name = "create-experiment")]
    CreateExperiment(CreateExperimentArgs),

    /// List the GPU compute catalog.
    Compute(ComputeArgs),

    /// Spin up standalone compute in an organization (no experiment).
    Instance(InstanceArgs),

    /// Register this computer's SSH key so the boxes you provision accept it.
    #[command(name = "ssh-key")]
    SshKey(SshKeyArgs),

    /// Operate on one local experiment node.
    Exp(ExpArgs),

    /// Print CLI usage for agents, or fetch a skill doc.
    Skill(SkillArgs),

    /// Add reusable skills to the local OpenResearch library.
    Skills(LibraryArgs),

    /// Add reusable LaTeX templates to the local OpenResearch library.
    Templates(LibraryArgs),

    /// Install the OpenResearch skill into local coding agents (Claude Code, Codex, OpenCode, Cursor).
    #[command(name = "install-skills")]
    InstallSkills(InstallSkillsArgs),

    /// Call one paper-retrieval primitive; the caller owns the search loop.
    Discover(DiscoverArgs),

    /// Fetch a paper: alphaXiv report/full-text, or OpenAlex/bioRxiv metadata.
    /// The source is auto-detected from the id (override with `--source`).
    Paper(PaperArgs),

    /// Show the CLI version; `--check` compares it to the latest release.
    Version(VersionArgs),

    /// Update orx to the latest release (installer-script installs only).
    Update(UpdateArgs),

    /// Link the macOS app's `orx` onto your PATH (macOS app installs only).
    InstallCli(InstallCliArgs),

    /// Permanently delete the local database, CLI executable, or both.
    Delete(DeleteArgs),

    /// Loopback HTTP/SSE daemon over the local run store (jobs sibling of
    /// `opencode serve`); the api tunnels to it on agent boxes.
    Serve(ServeArgs),

    /// Supervise one local run: tail backend logs, persist status, and honor
    /// local cancel intent. Spawned detached by `exp run`.
    Supervise(SuperviseArgs),

    /// Start the local autoresearch dashboard on 127.0.0.1: embedded UI,
    /// JSON/SSE API over the local store, and the opencode agent proxy.
    Up(UpArgs),

    /// Turn anonymous usage analytics on or off, or show current status.
    Telemetry(TelemetryArgs),

    /// Internal: the Claude plan-mode `PreToolUse` hook body. Reads the hook
    /// payload on stdin and prints an allow decision for read-only `orx`
    /// inspection; not a user command.
    #[command(name = "plan-gate", hide = true)]
    PlanGate,

    /// Internal: the plan-mode permission bridge. A stdio MCP server Claude
    /// Code spawns (`--mcp-config`) and consults (`--permission-prompt-tool`);
    /// relays each permission request to the running `orx up`, which surfaces
    /// an approval card and blocks until answered. Not a user command.
    #[command(name = "mcp-gate", hide = true)]
    McpGate,

    /// Internal: detached worker for optional local-project publication.
    #[command(name = "publish-branch", hide = true)]
    PublishBranch(PublishBranchArgs),

    /// Internal: manage a persistent SSH remote host.
    #[command(name = "remote-host", hide = true)]
    RemoteHost(RemoteHostArgs),
}

#[derive(Args, Debug)]
pub struct PublishBranchArgs {
    pub repo_path: std::path::PathBuf,
    pub branch: String,
    pub owner: String,
    pub repo: String,
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    /// A double-clicked orx.exe reaches `up` through this parse; an argv clap
    /// rejected would panic there instead of opening the dashboard.
    #[test]
    fn a_double_click_parses_as_a_local_up() {
        assert!(matches!(
            Cli::parse_from(["orx", "up"]).command,
            Some(Command::Up(args)) if args.remote.is_none()
        ));
    }

    #[test]
    fn library_add_commands_are_distinct_from_reading_skill_docs() {
        for name in ["skills", "templates"] {
            let cli = Cli::try_parse_from(["orx", name, "add", "./package.zip"]).unwrap();
            let args = match cli.command.unwrap() {
                Command::Skills(args) | Command::Templates(args) => args,
                _ => panic!("expected a library command"),
            };
            let LibraryCommand::Add { path } = args.command;
            assert_eq!(path, std::path::PathBuf::from("./package.zip"));
            assert!(Cli::try_parse_from(["orx", name, "add"]).is_err());
        }
    }

    #[test]
    fn internal_commands_do_not_emit_command_telemetry() {
        assert!(!crate::should_capture_command(&Command::Supervise(
            SuperviseArgs {
                run_id: "run-1".into(),
            }
        )));
        assert!(!crate::should_capture_command(&Command::Update(
            UpdateArgs {
                background: true,
                dry_run: false,
                force: false,
            }
        )));
        assert!(crate::should_capture_command(&Command::Update(
            UpdateArgs {
                background: false,
                dry_run: true,
                force: false,
            }
        )));
    }

    #[test]
    fn discover_parses_independent_retrieval_options() {
        let cli = Cli::try_parse_from([
            "orx",
            "discover",
            "embedding",
            "test-time compute",
            "--published-after",
            "2024-01-01",
            "--published-before",
            "2025-12-31",
            "--prioritize",
            "historical",
            "--limit",
            "9",
        ])
        .expect("discover embedding should parse");

        let Some(Command::Discover(DiscoverArgs {
            command: DiscoverCommand::Embedding(args),
        })) = cli.command
        else {
            panic!("expected discover embedding command");
        };
        assert_eq!(args.query, "test-time compute");
        assert_eq!(args.published_after.as_deref(), Some("2024-01-01"));
        assert_eq!(args.published_before.as_deref(), Some("2025-12-31"));
        assert_eq!(args.prioritize, DiscoveryPriority::Historical);
        assert_eq!(args.limit, 9);
    }

    #[test]
    fn discover_parses_openalex_and_biorxiv_primitives() {
        for (source, expected) in [
            ("openalex", LitSource::Openalex),
            ("biorxiv", LitSource::Biorxiv),
        ] {
            let cli = Cli::try_parse_from(["orx", "discover", source, "protein folding"])
                .expect("source discovery should parse");
            let Some(Command::Discover(DiscoverArgs { command })) = cli.command else {
                panic!("expected discover command");
            };
            let actual = match command {
                DiscoverCommand::Openalex(_) => LitSource::Openalex,
                DiscoverCommand::Biorxiv(_) => LitSource::Biorxiv,
                _ => panic!("expected non-alphaXiv discovery source"),
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn run_accepts_only_supervised_backend_flags() {
        for flag in ["--gpu", "--cpu", "--sandbox"] {
            let error = Cli::try_parse_from(["orx", "exp", "run", "exp-1", flag, "value"])
                .expect_err("unsupported run flag should not parse");
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::UnknownArgument,
                "{flag}"
            );
        }
    }

    #[test]
    fn research_state_commands_are_local_only() {
        for command in [
            "explore",
            "experiments",
            "env",
            "search-logs",
            "artifacts",
            "artifact",
            "wandb",
            "query",
            "chart",
            "report",
        ] {
            let error = Cli::try_parse_from(["orx", command])
                .expect_err("removed research command should not parse");
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
        }

        let error = Cli::try_parse_from(["orx", "exp", "cmd", "exp-1"])
            .expect_err("per-experiment run command should not parse");
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);

        let error = Cli::try_parse_from(["orx", "projects", "--all"])
            .expect_err("local projects have no archived state");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);

        Cli::try_parse_from(["orx", "orgs", "--json"]).expect("orgs should parse");
    }

    #[test]
    fn openresearch_backend_and_flavor_still_parse() {
        let cli = Cli::try_parse_from([
            "orx",
            "exp",
            "run",
            "exp-1",
            "--backend",
            "openresearch",
            "--flavor",
            "h100_sxm",
        ])
        .expect("local OpenResearch launch should parse");

        let Some(Command::Exp(ExpArgs {
            command: ExpCommand::Run(args),
        })) = cli.command
        else {
            panic!("expected exp run command");
        };
        assert_eq!(args.backend.as_deref(), Some("openresearch"));
        assert_eq!(args.flavor.as_deref(), Some("h100_sxm"));
    }
}
