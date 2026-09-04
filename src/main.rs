//! OpenResearch CLI (`orx`) — Rust port entry point.
//!
//! A clap-derive command tree mirroring the USAGE
//! block, dispatched from an async `tokio::main`. Each subcommand routes to one
//! module fn in `commands::<name>`. The six fs verbs (read/write/str-replace/
//! ls/grep/rm) all route into `commands::fs`.
//!
//! Error handling: command fns return `anyhow::Result<()>`. `main` prints the
//! error's `Display` to stderr and exits 1 — matching the TS
//! `main().catch(err => { console.error(err.message); process.exit(1) })`.

mod browser;
mod editors;
// DTOs faithfully mirror every API wire field; not all are read by the CLI yet.
#[allow(dead_code)]
mod client;
mod commands;
mod compute;
mod config;
mod error;
mod folder_picker;
mod invocation;
mod jobs;
// Local mode (`orx up`): builds out across stages; not all of it is wired yet.
#[allow(dead_code)]
mod local;
mod output;
mod plane;
mod remote;
mod store;
mod telemetry;
mod updates;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "orx",
    about = "OpenResearch CLI",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    // Optional so a bare `orx` prints USAGE to stdout and exits 0 (like the TS
    // `if (!command) { console.log(USAGE); return; }`) instead of clap's exit-2.
    #[command(subcommand)]
    command: Option<Command>,

    /// Disable anonymous usage analytics for this run. To disable it
    /// persistently, run `orx telemetry off`.
    #[arg(long, global = true)]
    no_telemetry: bool,
}

#[derive(Subcommand, Debug)]
// NOTE: `local::harness::plan_gate` keeps a hand-maintained allowlist of the
// read-only verbs here (what Claude plan mode may run without approval). When
// you add a *read-only* subcommand, add it there too, or it stays gated in plan
// mode. `readonly_verbs_are_real_commands` catches renames but not additions.
enum Command {
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
struct PublishBranchArgs {
    repo_path: std::path::PathBuf,
    branch: String,
    owner: String,
    repo: String,
}

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// Override the API base URL (or set OPENRESEARCH_API_URL).
    #[arg(long = "api-url")]
    pub api_url: Option<String>,
}

#[derive(Args, Debug)]
pub struct ProjectsArgs {
    /// Emit local project records as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct OrgsArgs {
    /// Emit organization records as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    /// Show a local project's details and experiment tree.
    View { project_id: String },

    /// Edit a local project's name or run command.
    Edit {
        project_id: String,
        /// Rename the project.
        #[arg(long)]
        name: Option<String>,
        /// Set the project's default run command.
        /// New experiments inherit it; pass '' to clear.
        #[arg(long = "run-command")]
        run_command: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct RunsArgs {
    pub project_id: String,
    /// Filter to one experiment.
    #[arg(long)]
    pub experiment: Option<String>,
}

#[derive(Args, Debug)]
pub struct LogsArgs {
    pub run_id: String,
    /// Read from the start instead of the tail.
    #[arg(long)]
    pub head: bool,
    /// Max bytes to read.
    #[arg(long)]
    pub bytes: Option<String>,
    /// Exact byte window `<start>:<end>`.
    #[arg(long)]
    pub range: Option<String>,
}

#[derive(Args, Debug)]
pub struct CreateExperimentArgs {
    /// Local project id from `orx projects`.
    pub project_id: String,
    /// Experiment title (required).
    #[arg(long)]
    pub title: Option<String>,
    /// Experiment description.
    #[arg(long)]
    pub description: Option<String>,
    /// Parent experiment id -> create a child. Omit on an empty project to
    /// create the baseline (root); once a root exists, attach under it.
    #[arg(long)]
    pub parent: Option<String>,
    /// Create a new baseline (root) even when the project already has one.
    /// Conflicts with --parent. Projects may hold multiple baselines.
    #[arg(long, conflicts_with = "parent")]
    pub baseline: bool,
    /// Run command for the node. Omit to inherit from the parent/project default.
    #[arg(long = "run-command")]
    pub run_command: Option<String>,
}

#[derive(Args, Debug)]
pub struct ComputeArgs {
    /// List CPU-only instance offers instead of the GPU catalog. CPU instances
    /// suit GPU-less experiments (data prep, eval harnesses, CPU-bound papers).
    #[arg(long)]
    pub cpu: bool,
    /// Filter to one GPU id (e.g. `H100_SXM`). Case-insensitive. GPU mode only.
    #[arg(long)]
    pub gpu: Option<String>,
    /// Filter to a specific GPU count per instance. GPU mode only.
    #[arg(long)]
    pub count: Option<i64>,
    /// Filter to one provider (e.g. `runpod`, `vast`, `lambda`). Case-insensitive. GPU mode only.
    #[arg(long)]
    pub provider: Option<String>,
}

#[derive(Args, Debug)]
pub struct SshKeyArgs {
    #[command(subcommand)]
    pub command: SshKeyCommand,
}

#[derive(Subcommand, Debug)]
pub enum SshKeyCommand {
    /// Register a public key on your account. Every box in your orgs — including
    /// ones already running — starts accepting it.
    Add(SshKeyAddArgs),
    /// List registered keys, marking the ones usable from this computer.
    List,
}

#[derive(Args, Debug)]
pub struct SshKeyAddArgs {
    /// Path to the PUBLIC key (defaults to `~/.ssh/id_ed25519.pub`).
    pub path: Option<String>,
}

#[derive(Args, Debug)]
pub struct InstanceArgs {
    #[command(subcommand)]
    pub command: InstanceCommand,
}

#[derive(Subcommand, Debug)]
pub enum InstanceCommand {
    /// Provision a standalone instance in an org (GPU with `--gpu`, or CPU with
    /// `--cpu`). Not tied to an experiment — like the dashboard's "Spin up".
    Create(InstanceCreateArgs),
    /// List an org's instances (status, SSH endpoint, price) — including any
    /// `--backend openresearch` box a failed teardown left behind.
    List(InstanceListArgs),
    /// Terminate an instance (destroys the provider machine). The manual
    /// cleanup path when a run's automatic teardown failed.
    Delete(InstanceDeleteArgs),
}

#[derive(Args, Debug)]
pub struct InstanceCreateArgs {
    /// Organization id (from `orx orgs`).
    pub org_id: String,
    /// Provision a GPU instance with this GPU id, e.g. `H100_SXM` — the exact id
    /// from `orx compute`, not a family name like `H100`.
    #[arg(long)]
    pub gpu: Option<String>,
    /// GPUs per instance (with `--gpu`; default 1).
    #[arg(long)]
    pub count: Option<i64>,
    /// Disk in GB (with `--gpu`; default 100).
    #[arg(long)]
    pub disk: Option<i64>,
    /// Provider to provision from (with `--gpu`), e.g. runpod, vast, lambda.
    /// Omit to pick the cheapest matching offer across providers (like the
    /// dashboard). See `orx compute` for providers; validated server-side.
    #[arg(long)]
    pub provider: Option<String>,
    /// Provision a CPU-only instance with this flavor: cpu5c (compute), cpu5g
    /// (general), or cpu5m (memory-optimized). Mutually exclusive with `--gpu`.
    #[arg(long)]
    pub cpu: Option<String>,
    /// vCPUs for a CPU instance (with `--cpu`): 2, 8, or 32 (default 8).
    #[arg(long)]
    pub vcpus: Option<i64>,
}

#[derive(Args, Debug)]
pub struct InstanceListArgs {
    /// Organization id (from `orx orgs`).
    pub org_id: String,
}

#[derive(Args, Debug)]
pub struct InstanceDeleteArgs {
    /// The instance (sandbox) id to terminate.
    pub sandbox_id: String,
}

#[derive(Args, Debug)]
pub struct AgentArgs {
    #[command(subcommand)]
    pub command: AgentCommand,
}

#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    /// Hand a task to a helper agent running in its own top-level session.
    Spawn {
        /// What the helper agent should do. Write it as a self-contained brief:
        /// the helper starts with an empty transcript and cannot see this chat.
        task: Option<String>,
        /// Read the task from stdin instead, for long multi-paragraph briefs.
        #[arg(long)]
        stdin: bool,
        /// Name the session in the sidebar. Defaults to an auto-generated title.
        #[arg(long)]
        title: Option<String>,
        /// Harness for the helper (defaults to this session's).
        #[arg(long)]
        harness: Option<String>,
        /// Model for the helper (defaults to this session's).
        #[arg(long)]
        model: Option<String>,
        /// Do not resume this chat when the helper finishes.
        #[arg(long)]
        no_wake: bool,
    },
}

#[derive(Args, Debug)]
pub struct ExpArgs {
    #[command(subcommand)]
    pub command: ExpCommand,
}

#[derive(Subcommand, Debug)]
pub enum ExpCommand {
    /// Show the experiment's status, run command, and latest run.
    Status { exp_id: String },

    /// View the experiment's description/notes, or overwrite it with `--set` / `--stdin`.
    Desc {
        exp_id: String,
        /// Overwrite the description with this value.
        #[arg(long)]
        set: Option<String>,
        /// Overwrite the description with the whole of stdin (for long markdown docs).
        #[arg(long)]
        stdin: bool,
    },

    /// Launch a locally initialized experiment through an orx-supervised backend.
    Run(Box<ExpRunArgs>),

    /// Cancel the in-flight run.
    Cancel { exp_id: String },

    /// Resume this agent after the experiment's latest run succeeds or fails.
    Wake { exp_id: String },

    /// Wait for a run to finish: one experiment (`<expId>`) or the next completion in a project (`--project`).
    Wait {
        /// Experiment to watch; its latest run is polled until it reaches a
        /// terminal state. Omit and pass `--project` to watch a whole project.
        exp_id: Option<String>,
        /// Watch every run in this project and return on the FIRST one to
        /// complete (reach done/failed/cancelled) — a "slot freed" signal. Call
        /// it in a loop, re-listing `orx runs` on each return to catch all
        /// finished runs. Returns immediately ("drained: no runs in flight") if
        /// none are in flight. Mutually exclusive with `<expId>`.
        #[arg(long)]
        project: Option<String>,
        /// Give up and exit non-zero after this many seconds (default 1800).
        #[arg(long)]
        timeout: Option<u64>,
        /// Seconds between polls (default 5).
        #[arg(long)]
        interval: Option<u64>,
    },
}

#[derive(Args, Debug)]
pub struct ExpRunArgs {
    pub exp_id: String,
    /// Disk in GB for a `--backend openresearch` instance (default 100).
    #[arg(long)]
    pub disk: Option<i64>,
    /// Provider for a `--backend openresearch` GPU flavor. When omitted, the
    /// cheapest qualified offer is selected.
    #[arg(long)]
    pub provider: Option<String>,
    /// orx-supervised executor: `hf` (Hugging Face Jobs,
    /// billed to your HF account), `modal` (a Modal Sandbox on your own Modal
    /// account, billed per second), `k8s` (a Job on your own Kubernetes
    /// cluster), `ssh` (a detached process on one of your own boxes), `slurm`
    /// (a batch job on your Slurm cluster, submitted via its login node),
    /// `ray` (a job on your Ray cluster, via the Ray Jobs API), `openresearch`
    /// (an ephemeral OpenResearch GPU/CPU box billed to your org; needs
    /// `orx login`), `tinker` (a local controller using remote Tinker model
    /// compute), or `local` (a detached process on this machine). k8s,
    /// ssh, slurm, ray, openresearch, tinker, and local are local
    /// experiments only. orx submits the job and a detached supervisor
    /// records status and logs locally. Omitted on a local experiment: launches on
    /// the configured default compute target, if set.
    #[arg(long)]
    pub backend: Option<String>,
    /// Hardware flavor. With `--backend hf`: t4-small, a10g-small, a100-large,
    /// h200, … With `--backend modal`: a Modal GPU (t4, l4, a10g, a100,
    /// a100-80gb, l40s, h100, h200, or e.g. h100:2) or cpu/cpu-large. With
    /// `--backend slurm`: a GPU request as a GRES spec (h100:2 → --gres=gpu:h100:2;
    /// plain `gpu` → one GPU; omit for CPU-only). With `--backend ray`: optional
    /// entrypoint resources (`cpu:2`, `gpu:1`, `gpu:1,mem:8GiB`; omit to reserve
    /// nothing). With `--backend openresearch`: a GPU id from `orx compute`
    /// (h100_sxm, or h100_sxm:2 for two) or a CPU flavor (cpu5c/cpu5g/cpu5m, or
    /// cpu5c:32 for the vCPU tier). Not used by k8s (see --manifest) or ssh
    /// (see --host).
    #[arg(long)]
    pub flavor: Option<String>,
    /// The org to bill the box to (with `--backend openresearch`). Omit when
    /// you belong to exactly one org.
    #[arg(long)]
    pub org: Option<String>,
    /// The ~/.ssh/config host alias to run on (with `--backend ssh`), or the
    /// cluster login node (with `--backend slurm`; defaults to the slurm
    /// settings' host).
    #[arg(long)]
    pub host: Option<String>,
    /// Repo-relative path to the k8s manifest on the experiment branch (with
    /// `--backend k8s`; default .orx/k8s.yaml). The manifest declares the run's
    /// resources — image, GPUs, topology — and orx injects the run script, env
    /// Secret, labels, and a default timeout. See `orx skill` for the contract.
    #[arg(long)]
    pub manifest: Option<String>,
    /// Docker image for the job (with `--backend hf/modal`). Defaults to
    /// python:3.12 on CPU flavors, a CUDA pytorch image otherwise. With
    /// `--backend k8s`, set the image in the manifest instead.
    #[arg(long)]
    pub image: Option<String>,
    /// Job timeout (with `--backend hf/modal/k8s/slurm/openresearch`): 90s,
    /// 30m, 4h, 1d. Default 4h (HF's own default is only 30 minutes). With
    /// `--backend k8s` it becomes activeDeadlineSeconds unless the manifest
    /// sets its own. With `--backend slurm` it becomes `#SBATCH --time=` and
    /// has no 4h default — unset falls back to the slurm settings, then the
    /// cluster's own limit. With `--backend openresearch` it bounds the run's
    /// wall clock on the box (the box itself is deleted when the run ends).
    /// Not supported with `--backend ray` (Ray Jobs have no time limit).
    #[arg(long)]
    pub timeout: Option<String>,
    /// Launch even when another run is already in flight for this experiment.
    #[arg(long)]
    pub force: bool,
    /// Internal attribution forwarded through the local orx up API.
    #[arg(skip)]
    pub chat_session_id: Option<String>,
}

impl ExpRunArgs {
    pub fn launching_chat_session(&self) -> Option<String> {
        self.chat_session_id
            .clone()
            .or_else(crate::local::chat::launching_chat_session)
    }
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Port to bind on 127.0.0.1 (default 4790 — what the api proxies to).
    #[arg(long)]
    pub port: Option<u16>,
}

#[derive(Args, Debug)]
pub struct SuperviseArgs {
    /// The run to supervise (must exist in the local store).
    pub run_id: String,
}

#[derive(Args, Debug)]
pub struct UpArgs {
    /// Port to bind on 127.0.0.1. With `--remote`, the local presentation port.
    #[arg(long, default_value_t = 4791)]
    pub port: u16,
    /// Run `orx up` on a remote box over SSH and forward it here. The value is
    /// an `~/.ssh/config` host alias, or `user@host` (append `:PORT` for a
    /// non-standard SSH port, e.g. `root@1.2.3.4:38455`). Only user@host + port
    /// are reconstructed; a custom key or jump host must come from `~/.ssh/config`.
    /// Starts an authenticated server there, tunnels it through a hidden local
    /// port, and opens a dedicated local presentation gateway in your browser.
    #[arg(long, value_name = "HOST")]
    pub remote: Option<String>,
    /// Don't open the dashboard in the browser on startup.
    #[arg(long)]
    pub no_browser: bool,
    /// Don't spawn the opencode agent on startup (for tests).
    #[arg(long)]
    pub no_agent: bool,
    /// opencode model override, e.g. `anthropic/claude-sonnet-4-5`.
    #[arg(long)]
    pub model: Option<String>,
    /// Internal authenticated dashboard mode used behind a local SSH gateway.
    #[arg(long, hide = true)]
    pub remote_session_stdin: bool,
    /// Internal persistent dashboard/agent-host mode.
    #[arg(long, hide = true, conflicts_with = "remote_session_stdin")]
    pub remote_host: bool,
}

#[derive(Args, Clone, Debug)]
pub struct RemoteHostArgs {
    #[command(subcommand)]
    pub command: RemoteHostCommand,
}

#[derive(Clone, Subcommand, Debug)]
pub enum RemoteHostCommand {
    Ensure {
        #[arg(long)]
        expected_instance: Option<String>,
    },
    Status,
    Attach {
        #[arg(long)]
        expected_instance: String,
    },
    Stop,
}

#[derive(Args, Debug)]
pub struct SkillArgs {
    pub path: Option<String>,
}

#[derive(Args, Debug)]
pub struct InstallSkillsArgs {
    /// Which agent(s) to install into: `claude`, `codex`, `opencode`, `cursor`,
    /// or `all`. Defaults to every agent already set up on this machine.
    #[arg(long)]
    pub agent: Option<String>,

    /// Also install the full set of modular `orx` skills (~8 always-listed
    /// skills) into the agent's global skills dir, not just the thin shim.
    /// Intended for dedicated/orx-only environments. In a general-purpose setup
    /// the always-on skills add noise, so the default is the shim alone.
    #[arg(long)]
    pub full: bool,
}

#[derive(Args, Debug)]
pub struct TelemetryArgs {
    #[command(subcommand)]
    pub command: TelemetryCommand,
}

#[derive(Subcommand, Debug)]
pub enum TelemetryCommand {
    /// Show whether analytics is on, why, and the anonymous install id.
    Status,
    /// Enable anonymous usage analytics.
    On,
    /// Disable anonymous usage analytics on this machine.
    Off,
}

/// Which corpus a literature command searches or reads from.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum LitSource {
    /// alphaXiv (arXiv corpus: CS, math, physics, stats — the default).
    Alphaxiv,
    /// OpenAlex (general scholarly graph across all disciplines).
    Openalex,
    /// bioRxiv biology preprints (searched via OpenAlex, fetched via bioRxiv).
    Biorxiv,
}

impl LitSource {
    /// Lowercase wire name used to enforce against the Settings disable-set.
    /// Matches the `--source` flag values (clap `rename_all = "lower"`) and the
    /// `LitHit.source` JSON labels.
    pub fn as_str(&self) -> &'static str {
        match self {
            LitSource::Alphaxiv => "alphaxiv",
            LitSource::Openalex => "openalex",
            LitSource::Biorxiv => "biorxiv",
        }
    }

    /// Human-facing name for error/UI text.
    pub fn display_name(&self) -> &'static str {
        match self {
            LitSource::Alphaxiv => "alphaXiv",
            LitSource::Openalex => "OpenAlex",
            LitSource::Biorxiv => "bioRxiv",
        }
    }
}

#[derive(Args, Debug)]
pub struct DiscoverArgs {
    #[command(subcommand)]
    pub command: DiscoverCommand,
}

#[derive(Subcommand, Debug)]
pub enum DiscoverCommand {
    /// alphaXiv full-text BM25 retrieval with match snippets.
    Keyword(DiscoverySearchArgs),
    /// alphaXiv semantic title/abstract retrieval with similarity/popularity reranking.
    Embedding(DiscoverySearchArgs),
    /// OpenAlex scholarly-graph search across disciplines.
    Openalex(DiscoverySearchArgs),
    /// bioRxiv preprint search through OpenAlex's bioRxiv source index.
    Biorxiv(DiscoverySearchArgs),
}

#[derive(Args, Debug)]
pub struct DiscoverySearchArgs {
    /// Exact keyword query or semantic description, depending on the strategy.
    pub query: String,
    /// Include papers first published on or after this date (YYYY-MM-DD).
    #[arg(long = "published-after")]
    pub published_after: Option<String>,
    /// Include papers first published on or before this date (YYYY-MM-DD). Older
    /// or narrow embedding windows can return a thin candidate set.
    #[arg(long = "published-before")]
    pub published_before: Option<String>,
    /// Ranking policy after topical relevance is accounted for.
    #[arg(long, value_enum, default_value = "default")]
    pub prioritize: DiscoveryPriority,
    /// Maximum results to emit (default 15). alphaXiv uses its fixed server-side candidate pool.
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u32).range(1..=200))]
    pub limit: u32,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum DiscoveryPriority {
    Historical,
    Default,
    Recency,
    Popular,
}

impl DiscoveryPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Historical => "historical",
            Self::Default => "default",
            Self::Recency => "recency",
            Self::Popular => "popular",
        }
    }
}

#[derive(Args, Debug)]
pub struct VersionArgs {
    /// Print the embedded telemetry build channel.
    #[arg(long, hide = true, conflicts_with_all = ["check", "json"])]
    pub build_channel: bool,
    /// Also check the latest released version on GitHub.
    #[arg(long)]
    pub check: bool,
    /// Emit a JSON object instead of text (implies --check).
    #[arg(long)]
    pub json: bool,
    /// Print the dashboard protocol understood by this binary.
    #[arg(long, hide = true, conflicts_with_all = ["check", "json", "build_channel"])]
    pub dashboard_protocol: bool,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Report whether an update is available without installing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Update even when the binary doesn't match the install receipt
    /// (multiple copies, or a `cargo install` overwrote it).
    #[arg(long)]
    pub force: bool,
    /// Internal: the detached auto-updater. Silent, and records its outcome so
    /// repeated failures back off.
    #[arg(long, hide = true)]
    pub background: bool,
}

#[derive(Args, Debug)]
pub struct InstallCliArgs {
    /// Replace an existing `orx` on your PATH.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct DeleteArgs {
    #[command(subcommand)]
    pub command: DeleteCommand,
}

#[derive(Subcommand, Debug, Clone, Copy)]
pub enum DeleteCommand {
    /// Delete only orx.db and its SQLite sidecars. Project folders are untouched.
    #[command(alias = "db")]
    Database,
    /// Delete the running orx executable and its matching installer receipt.
    Cli,
    /// Delete both the database and CLI executable.
    All,
}

#[derive(Args, Debug)]
pub struct PaperArgs {
    /// Paper id: an arXiv id / URL (alphaXiv), a DOI (bioRxiv `10.1101/…` or any
    /// other), or an OpenAlex `W…` id. The source is auto-detected.
    pub id: String,
    /// Force the source instead of auto-detecting it from the id.
    #[arg(long, value_enum)]
    pub source: Option<LitSource>,
    /// Fetch the full extracted paper text instead of the report (alphaXiv only;
    /// OpenAlex/bioRxiv have no extracted full text and point you at the PDF).
    #[arg(long)]
    pub full: bool,
}

// The default multi-thread runtime is load-bearing for macOS app mode: it blocks
// the main thread in the AppKit run loop while the dashboard server runs on
// worker threads. A `current_thread` flavor would deadlock. See commands::app.
#[tokio::main]
async fn main() {
    // Double-clicked as the macOS .app? Enter GUI app mode (Dock icon, dashboard
    // server, browser) instead of parsing CLI args. Also require an empty argv so
    // the bundled binary stays usable as a CLI (`…/MacOS/OpenResearch up`), since
    // the bundle itself launches it with no arguments. See commands::app.
    #[cfg(target_os = "macos")]
    if commands::app::launched_as_app_bundle() && std::env::args_os().len() == 1 {
        // Shell hydration may change XDG_CONFIG_HOME; settle it before telemetry or the lifecycle lock.
        commands::app::hydrate_shell_env().await;
        telemetry::set_flag(false);
        let _session = telemetry::TelemetrySession::start_app();
        // AppKit owns process shutdown; the durable outbox covers termination before delivery.
        commands::app::run().await;
        return;
    }

    let cli = Cli::parse();
    let Some(command) = cli.command else {
        // Bare `orx`: print the command overview to stdout and exit 0.
        use clap::CommandFactory;
        Cli::command().print_help().ok();
        return;
    };
    // Outdated-version warning (skipped for the commands that manage updates
    // themselves). `start` prints the cached warning to stderr *now*,
    // before the command runs, so it shows even for commands that
    // `std::process::exit` on their own (e.g. the "not logged in" path) instead
    // of returning here. Never touches stdout or the exit code. Silence it with
    // ORX_NO_UPDATE_CHECK / NO_UPDATE_NOTIFIER.
    // `plan-gate` is a per-tool-call hook body (fires on every Bash call during
    // plan mode): it must stay fast and touch neither stdout nor the network, so
    // skip the update check and telemetry and run it directly.
    if matches!(command, Command::PlanGate) {
        // The hook fires on every Bash call during plan mode; it must NEVER
        // block the turn. Swallow any error to stderr and still exit 0 — a
        // non-zero exit here would fail every Bash tool call. (`run` is
        // infallible today; this keeps the invariant if that ever changes.)
        if let Err(err) = commands::plan_gate::run().await {
            eprintln!("orx plan-gate: {err}");
        }
        return;
    }
    // `mcp-gate` is Claude's stdio MCP child for the turn: stdout is the MCP
    // channel (nothing else may write to it) and startup must be instant or
    // Claude times the server out — skip the update check and telemetry.
    if matches!(command, Command::McpGate) {
        if let Err(err) = commands::mcp_gate::run().await {
            // stderr only; a failed bridge degrades plan mode, never the CLI.
            eprintln!("orx mcp-gate: {err}");
            std::process::exit(1);
        }
        return;
    }
    if let Command::RemoteHost(args) = &command {
        if let Err(err) = commands::remote_host::run(args.clone()).await {
            eprintln!("orx remote-host: {err}");
            std::process::exit(1);
        }
        return;
    }
    if let Command::PublishBranch(args) = &command {
        if let Err(err) =
            local::git::push_branch(&args.repo_path, &args.branch, &args.owner, &args.repo)
        {
            eprintln!("orx publish-branch: {err}");
            std::process::exit(1);
        }
        return;
    }

    let warning = (!matches!(
        command,
        Command::Version(_) | Command::Update(_) | Command::Delete(_)
    ))
    .then(updates::UpdateWarning::start);

    // Anonymous usage analytics. Record the flag process-globally so command
    // modules can fire events without threading it through, then fire the
    // per-invocation event *before* dispatch so commands that exit on their own
    // (e.g. the "not logged in" path) are still counted. Opt out with
    // --no-telemetry or `orx telemetry off`.
    telemetry::set_flag(cli.no_telemetry);
    let session = telemetry::TelemetrySession::start(
        should_capture_command(&command).then(|| command_name(&command)),
    );

    let result = dispatch(command).await;
    if let Some(warning) = warning {
        warning.finish().await;
    }
    session.finish(result.is_ok()).await;

    if let Err(err) = result {
        // Match the TS: print only the message, exit 1.
        eprintln!("{}", err);
        std::process::exit(1);
    }
}

fn should_capture_command(command: &Command) -> bool {
    !matches!(
        command,
        Command::Supervise(_)
            | Command::Update(UpdateArgs {
                background: true,
                ..
            })
    )
}

/// A stable, PII-free event label for each command, decoupled from the enum
/// variant name so renames don't silently break analytics continuity.
fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Login(_) => "login",
        Command::Logout => "logout",
        Command::Projects(_) => "projects",
        Command::Orgs(_) => "orgs",
        Command::Project(_) => "project",
        Command::Agent(_) => "agent",
        Command::Runs(_) => "runs",
        Command::Logs(_) => "logs",
        Command::CreateExperiment(_) => "create-experiment",
        Command::Compute(_) => "compute",
        Command::Instance(_) => "instance",
        Command::SshKey(_) => "ssh-key",
        Command::Exp(_) => "exp",
        Command::Skill(_) => "skill",
        Command::InstallSkills(_) => "install-skills",
        Command::Discover(_) => "discover",
        Command::Paper(_) => "paper",
        Command::Version(_) => "version",
        Command::Update(_) => "update",
        Command::InstallCli(_) => "install-cli",
        Command::Delete(_) => "delete",
        Command::Serve(_) => "serve",
        Command::Supervise(_) => "supervise",
        Command::Up(_) => "up",
        Command::Telemetry(_) => "telemetry",
        Command::PlanGate => "plan-gate",
        Command::McpGate => "mcp-gate",
        Command::PublishBranch(_) => "publish-branch",
        Command::RemoteHost(_) => "remote-host",
    }
}

async fn dispatch(command: Command) -> error::Result<()> {
    let lifecycle_lock = command_uses_lifecycle_lock(&command)
        .then(store::open_lifecycle_lock)
        .transpose()?;
    let _lifecycle_guard = lifecycle_lock
        .as_ref()
        .map(|lock| lock.read())
        .transpose()?;

    match command {
        Command::Login(args) => commands::login::run(args).await,
        Command::Logout => commands::logout::run().await,
        Command::Projects(args) => commands::projects::run(args).await,
        Command::Orgs(args) => commands::orgs::run(args).await,
        Command::Project(args) => commands::project::run(args).await,
        Command::Agent(args) => commands::agent::run(args).await,
        Command::Runs(args) => commands::runs::run(args).await,
        Command::Logs(args) => commands::logs::run(args).await,
        Command::CreateExperiment(args) => commands::create_experiment::run(args).await,
        Command::Compute(args) => commands::compute::run(args).await,
        Command::Instance(args) => commands::instance::run(args).await,
        Command::SshKey(args) => match args.command {
            SshKeyCommand::Add(a) => commands::ssh_key::add(a.path).await,
            SshKeyCommand::List => commands::ssh_key::list().await,
        },
        Command::Exp(args) => commands::exp::run(args).await,
        Command::Skill(args) => commands::skill::run(args).await,
        Command::InstallSkills(args) => commands::install_skills::run(args).await,
        Command::Discover(args) => commands::discover::run(args).await,
        Command::Paper(args) => commands::paper::run(args).await,
        Command::Version(args) => commands::version::run(args).await,
        Command::Update(args) => commands::update::run(args).await,
        Command::InstallCli(args) => commands::install_cli::run(args).await,
        Command::Delete(args) => commands::delete::run(args).await,
        Command::Serve(args) => commands::serve::run(args).await,
        Command::Supervise(args) => commands::supervise::run(args).await,
        Command::Up(args) => match args.remote.clone() {
            Some(host) => commands::up_remote::run(&host, args).await,
            None => commands::up::run(args).await,
        },
        Command::Telemetry(args) => commands::telemetry::run(args).await,
        // Handled before dispatch (fast path, no telemetry/update check).
        Command::PlanGate => commands::plan_gate::run().await,
        Command::McpGate => commands::mcp_gate::run().await,
        Command::PublishBranch(_) => unreachable!("handled before dispatch"),
        Command::RemoteHost(_) => unreachable!("handled before dispatch"),
    }
}

fn command_uses_lifecycle_lock(command: &Command) -> bool {
    !matches!(
        command,
        Command::Login(_)
            | Command::Logout
            | Command::InstallSkills(_)
            | Command::Discover(_)
            | Command::Paper(_)
            | Command::Version(_)
            | Command::Delete(_)
            | Command::Telemetry(_)
            | Command::PlanGate
            | Command::McpGate
            | Command::PublishBranch(_)
            | Command::RemoteHost(_)
    )
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn internal_commands_do_not_emit_command_telemetry() {
        assert!(!should_capture_command(&Command::Supervise(
            SuperviseArgs {
                run_id: "run-1".into(),
            }
        )));
        assert!(!should_capture_command(&Command::Update(UpdateArgs {
            background: true,
            dry_run: false,
            force: false,
        })));
        assert!(should_capture_command(&Command::Update(UpdateArgs {
            background: false,
            dry_run: true,
            force: false,
        })));
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
