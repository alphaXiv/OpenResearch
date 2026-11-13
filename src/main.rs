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

mod alphaxiv;
mod browser;
mod editors;
// DTOs faithfully mirror every API wire field; not all are read by the CLI yet.
#[allow(dead_code)]
mod client;
mod commands;
mod compute;
mod config;
mod engine;
mod error;
mod file_lock;
mod folder_picker;
mod invocation;
mod jobs;
// Local mode (`orx up`): builds out across stages; not all of it is wired yet.
#[allow(dead_code)]
mod local;
mod net;
mod output;
mod paths;
mod plane;
mod process;
mod remote;
mod store;
mod telemetry;
mod updates;
mod workspace_state;

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

    /// Read your alphaXiv library.
    Alphaxiv(AlphaXivArgs),

    /// List projects registered in the local orx store.
    Projects(ProjectsArgs),

    /// List organizations available for OpenResearch compute.
    Orgs(OrgsArgs),

    /// Operate on one local project.
    Project(ProjectArgs),

    /// List a project's runs.
    Runs(RunsArgs),

    /// Show a run's compact log summary (path, size, and preview) by default.
    Logs(LogsArgs),

    /// Print the command that opens a run's TensorBoard logs.
    Tensorboard(TensorboardArgs),

    /// Summarize the metrics runs logged to Trackio: local, a Hugging Face Space, or a server.
    Metrics(MetricsArgs),

    /// Add an experiment node to a local `orx up` project.
    #[command(name = "create-experiment")]
    CreateExperiment(CreateExperimentArgs),

    /// Configure compute backends, test connections, and browse offers.
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

    /// Install the OpenResearch skill into local coding agents (Claude Code, Codex, OpenCode, Cursor, Antigravity).
    #[command(name = "install-skills")]
    InstallSkills(InstallSkillsArgs),

    /// Call one paper-retrieval primitive; the caller owns the search loop.
    Discover(DiscoverArgs),

    /// Fetch a paper: alphaXiv report/full-text, or OpenAlex/bioRxiv/PubMed metadata.
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

    /// Start the local autoresearch dashboard on 127.0.0.1: embedded UI,
    /// JSON/SSE API over the local store, and the opencode agent proxy.
    Up(UpArgs),

    /// Turn anonymous usage analytics on or off, or show current status.
    Telemetry(TelemetryArgs),

    /// Report a bug, feature request, or frustration with OpenResearch itself
    /// to its maintainers. Filed by the agent; see the `orx-feedback` skill.
    Feedback(FeedbackArgs),

    /// Internal: the Claude plan-mode `PreToolUse` hook body. Reads the hook
    /// payload on stdin and prints an allow decision for read-only `orx`
    /// inspection; not a user command.
    #[command(name = "plan-gate", hide = true)]
    PlanGate,

    #[command(name = "invocation-gate", hide = true)]
    InvocationGate,

    /// Internal: orx's stdio MCP server, which every agent harness spawns:
    /// Claude's permission bridge and the orchestration tools, relayed to the
    /// running `orx up`. `mcp-gate`, its old name, stays for configs an older
    /// `orx up` still writes. Not a user command.
    #[command(name = "mcp", alias = "mcp-gate", hide = true)]
    Mcp,

    #[command(name = "alphaxiv-mcp", hide = true)]
    AlphaxivMcp,

    #[command(name = "antigravity-gate", hide = true)]
    AntigravityGate,

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
    /// The app's sign-in: skips the agent-skills offer.
    #[arg(long, hide = true)]
    pub non_interactive: bool,
    /// Connect alphaXiv to an existing OpenResearch sign-in.
    #[arg(long)]
    pub alphaxiv: bool,
    /// Approve the sign-in with a code on any device, instead of this computer's
    /// browser. The default inside an SSH session.
    #[arg(long)]
    pub device: bool,
}

#[derive(Args, Debug)]
pub struct AlphaXivArgs {
    #[command(subcommand)]
    command: AlphaXivCommand,
}

#[derive(Subcommand, Debug)]
enum AlphaXivCommand {
    /// List folders and papers.
    Library {
        #[arg(long)]
        json: bool,
    },
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
    View {
        project_id: String,
        /// Include archived experiments in the listing.
        #[arg(long)]
        all: bool,
    },

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

    /// Save a project as a .orx file a collaborator can open in OpenResearch,
    /// or publish it as an openresearch.sh link with --publish.
    /// Every chat is included unless narrowed with --chat or left out with --no-chats.
    Share {
        project_id: String,
        /// File to write (default: ./<project>.orx).
        #[arg(long, conflicts_with = "publish")]
        out: Option<String>,
        /// Upload the file to openresearch.sh and print a link anyone can open
        /// (needs `orx login`). Publishing again updates the same link.
        #[arg(long)]
        publish: bool,
        /// Include only this chat's transcript (repeatable).
        #[arg(long = "chat")]
        chats: Vec<String>,
        /// Leave every chat out.
        #[arg(long, conflicts_with = "chats")]
        no_chats: bool,
        /// Leave run logs out.
        #[arg(long)]
        no_logs: bool,
        /// Leave the artifacts folder out.
        #[arg(long)]
        no_artifacts: bool,
    },

    /// Delete a project's published openresearch.sh link and its file.
    Unpublish { project_id: String },

    /// Open a shared .orx file, or an openresearch.sh/p/... link, as a new project.
    Import {
        file: String,
        /// Folder to create the project in (default: ./<file or project name>).
        #[arg(long)]
        path: Option<String>,
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
    #[arg(short, long, hide = true)]
    pub follow: bool,
}

#[derive(Args, Debug)]
pub struct TensorboardArgs {
    pub run_id: String,
}

#[derive(Args, Debug)]
pub struct MetricsArgs {
    /// One or more run ids; several print one after another for comparison.
    #[arg(required = true)]
    pub run_ids: Vec<String>,
    /// Only metrics whose name contains this text (case-insensitive). Repeatable.
    #[arg(long = "metric")]
    pub metrics: Vec<String>,
    /// Sampled points per metric, first and last included.
    #[arg(long, default_value_t = 20)]
    pub points: usize,
    /// Print JSON instead of the text summary.
    #[arg(long)]
    pub json: bool,
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
    #[command(subcommand)]
    pub command: Option<commands::compute::ComputeCommand>,
    /// Machine-readable output.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(flatten)]
    pub catalog: commands::compute::CatalogArgs,
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
    /// Public key path. Without a path, reuse ~/.ssh/id_ed25519.pub or create a key pair.
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
pub struct ExpArgs {
    #[command(subcommand)]
    pub command: ExpCommand,
}

#[derive(Subcommand, Debug)]
pub enum ExpCommand {
    /// Hide ancestors, the selected experiment, or descendants.
    /// A later no-parent create starts a new baseline if all roots are archived.
    #[command(group(clap::ArgGroup::new("archive_scope").required(true)))]
    Archive {
        exp_id: String,
        /// Archive ancestors, excluding the selected experiment.
        #[arg(long, group = "archive_scope")]
        ancestors: bool,
        /// Archive only the selected experiment.
        #[arg(long, group = "archive_scope")]
        only: bool,
        /// Archive descendants, excluding the selected experiment.
        #[arg(long, group = "archive_scope")]
        descendants: bool,
    },

    /// Restore archived experiments using the same scopes.
    #[command(group(clap::ArgGroup::new("unarchive_scope").required(true)))]
    Unarchive {
        exp_id: String,
        /// Restore ancestors, excluding the selected experiment.
        #[arg(long, group = "unarchive_scope")]
        ancestors: bool,
        /// Restore only the selected experiment.
        #[arg(long, group = "unarchive_scope")]
        only: bool,
        /// Restore descendants, excluding the selected experiment.
        #[arg(long, group = "unarchive_scope")]
        descendants: bool,
    },

    /// Permanently delete an experiment with no child experiments or active runs, its runs, and
    /// its orx/ branch (refused while any worktree has it checked out).
    Delete { exp_id: String },

    /// Re-parent an experiment and its subtree. Only the tree changes; git branches stay put.
    #[command(group(clap::ArgGroup::new("move_target").required(true)))]
    Move {
        exp_id: String,
        /// New parent experiment in the same project.
        #[arg(long, group = "move_target")]
        parent: Option<String>,
        /// Make the experiment a new root (baseline).
        #[arg(long, group = "move_target")]
        baseline: bool,
    },

    /// Show the experiment's status, run command, and latest run.
    Status {
        exp_id: String,
        /// Query live Slurm accounting in addition to locally stored status.
        #[arg(long)]
        scheduler: bool,
    },

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

    /// Launch a locally initialized experiment through the run engine.
    Run(Box<ExpRunArgs>),

    /// Cancel in-flight runs in an experiment, or one run with --run.
    Cancel {
        #[arg(required_unless_present = "run")]
        exp_id: Option<String>,
        /// Cancel only this run. With an experiment id, the run must belong to it.
        #[arg(long)]
        run: Option<String>,
    },

    /// Abandon a run whose outcome cannot be recovered; keep its cancel pending.
    Abandon { run: String },

    /// Send this chat the result of the experiment's latest run when it finishes.
    Wake { exp_id: String },

    /// Wait for a run to finish: one experiment (`<expId>`) or the next completion in a project (`--project`).
    Wait {
        /// Experiment to watch until its latest run reaches a terminal state.
        /// Omit and pass `--project` to watch a whole project.
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
        /// Seconds between retries while the local database is busy (default 5).
        #[arg(long, hide = true)]
        interval: Option<u64>,
    },
}

#[derive(Args, Debug, serde::Serialize, serde::Deserialize)]
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
    /// `ray` (a job on your Ray cluster, via the Ray Jobs API), `vercel` (a
    /// CPU-only Vercel Sandbox microVM on your own Vercel project), `openresearch`
    /// (an ephemeral OpenResearch GPU/CPU box billed to your org; needs
    /// `orx login`), `tinker` (a local controller using remote Tinker model
    /// compute), or `local` (a detached process on this machine). k8s,
    /// ssh, slurm, ray, openresearch, tinker, and local are local
    /// experiments only. The run engine in the OpenResearch server submits
    /// the job and records its status and logs locally. Omitted on a local experiment: launches on
    /// the configured default compute target, if set.
    #[arg(long)]
    pub backend: Option<String>,
    /// Hardware flavor. With `--backend hf`: t4-small, a10g-small, a100-large,
    /// h200, … With `--backend modal`: a Modal GPU (t4, l4, a10g, a100,
    /// a100-80gb, l40s, h100, h200, or e.g. h100:2) or cpu/cpu-large. With
    /// `--backend slurm`: a GPU request as a GRES spec (h100:2 → --gres=gpu:h100:2;
    /// plain `gpu` → one GPU; `cpu` → none; omitted uses the default flavor, if
    /// the default backend is slurm, else none). With `--backend ray`: optional
    /// entrypoint resources (`cpu:2`, `gpu:1`, `gpu:1,mem:8GiB`; omit to reserve
    /// nothing). With `--backend vercel`: optional vCPUs (cpu = 2, cpu-<n> or
    /// <n>: 1 or an even number up to 32; 2 GB memory each; plans cap this at
    /// 4 on Hobby, 8 on Pro, and 32 on Enterprise). With `--backend openresearch`: a GPU id from `orx compute`
    /// (h100_sxm, or h100_sxm:2 for two) or a CPU flavor (cpu5c/cpu5g/cpu5m, or
    /// cpu5c:32 for the vCPU tier). Not used by k8s (see --manifest) or ssh
    /// (see --host).
    #[arg(long)]
    pub flavor: Option<String>,
    /// The org to bill the box to (with `--backend openresearch`). Omit when
    /// you belong to exactly one org.
    #[arg(long)]
    pub org: Option<String>,
    /// The ~/.ssh/config host alias (SSH defaults to its saved default host), or the
    /// cluster login node (with `--backend slurm`; defaults to the slurm
    /// settings' host).
    #[arg(long)]
    pub host: Option<String>,
    /// Existing running Docker container on the SSH host (name or ID).
    #[arg(long, conflicts_with = "no_container")]
    pub container: Option<String>,
    /// Run directly on the SSH host, overriding its saved container.
    #[arg(long)]
    pub no_container: bool,
    /// Repo-relative path to the k8s manifest on the experiment branch (with
    /// `--backend k8s`; default .orx/k8s.yaml). The manifest declares the run's
    /// resources — image, GPUs, topology — and orx injects the run script, env
    /// Secret, labels, and a default timeout. See `orx skill` for the contract.
    #[arg(long)]
    pub manifest: Option<String>,
    /// Docker image for the job (with `--backend hf/modal`). Defaults to
    /// python:3.12 on CPU flavors, a CUDA pytorch image otherwise. With
    /// `--backend vercel`, a Vercel Container Registry image (default
    /// vercel/sandbox/python:3.14). With
    /// `--backend k8s`, set the image in the manifest instead.
    #[arg(long)]
    pub image: Option<String>,
    /// Job timeout (with `--backend hf/modal/vercel/k8s/slurm/openresearch`): 90s,
    /// 30m, 4h, 1d. Default 4h (HF's own default is only 30 minutes; Vercel
    /// defaults to 40m on Hobby so a 45-minute session still includes 5 minutes
    /// to read logs and the exit status, and caps a session at 24h otherwise). With
    /// `--backend k8s` it counts from the run's start, excluding the wait for a
    /// node or image, unless the manifest sets activeDeadlineSeconds. With `--backend slurm` it becomes `#SBATCH --time=` and
    /// has no 4h default — unset falls back to the slurm settings, then the
    /// cluster's own limit. With `--backend openresearch` it bounds the run's
    /// wall clock on the box (the box itself is deleted when the run ends).
    /// Not supported with `--backend ray` (Ray Jobs have no time limit).
    #[arg(long)]
    pub timeout: Option<String>,
    /// Cores for the job (with `--backend slurm`): `#SBATCH --cpus-per-task=`.
    /// Omitted falls back to the slurm settings, then the partition default —
    /// which on many clusters is a single core, whatever the GPU request.
    #[arg(long)]
    pub cpus: Option<u32>,
    /// Host memory for the job (with `--backend slurm`): `#SBATCH --mem=`, in
    /// Slurm's own syntax (64G, 4000M, or a bare number for megabytes).
    /// Omitted falls back to the slurm settings, then the partition default,
    /// which is usually `DefMemPerCPU` × the core count — so raising `--cpus`
    /// raises memory with it.
    #[arg(long)]
    pub mem: Option<String>,
    /// Slurm partition for this run; overrides the saved default. Accepts comma-separated names.
    #[arg(long)]
    pub partition: Option<String>,
    /// Slurm quality of service for this run (`#SBATCH --qos=`); overrides the saved default.
    #[arg(long)]
    pub qos: Option<String>,
    /// Slurm nodes to run on (`#SBATCH --nodelist=`), e.g. node[01-04].
    #[arg(long)]
    pub nodelist: Option<String>,
    /// Slurm node features to require (`#SBATCH --constraint=`), e.g. a100&ib.
    #[arg(long)]
    pub constraint: Option<String>,
    /// Slurm nodes to avoid (`#SBATCH --exclude=`), e.g. node07.
    #[arg(long)]
    pub exclude: Option<String>,
    /// An extra `#SBATCH` line for this run, such as --sbatch-arg='--rsc g=1'; repeatable,
    /// written after the saved ones. Options orx writes itself are refused.
    #[arg(long = "sbatch-arg", value_name = "ARG", allow_hyphen_values = true)]
    #[serde(default)]
    pub sbatch_args: Vec<String>,
    /// Launch even when another run is already in flight for this experiment.
    #[arg(long)]
    pub force: bool,
    /// Internal attribution forwarded through the local orx up API.
    #[arg(skip)]
    pub chat_session_id: Option<String>,
    #[arg(long, hide = true)]
    pub invocation_context: Option<String>,
    /// The native agent CLI whose shell ran `orx exp run`, read once in that process.
    #[arg(skip)]
    pub agent_origin: Option<String>,
    /// Handled by `orx up` for a caller: launch evidence comes only from the request.
    #[arg(skip)]
    pub forwarded: bool,
    #[arg(skip)]
    pub telemetry_suppressed: bool,
    /// The caller's compute settings, which must be the server's.
    #[arg(skip)]
    #[serde(default)]
    pub caller_config_dir: Option<std::path::PathBuf>,
    /// The caller's interpreter overrides (`ORX_MODAL_PYTHON`, …), which must be the server's.
    #[arg(skip)]
    #[serde(default)]
    pub caller_python_overrides: std::collections::BTreeMap<String, String>,
    /// What the caller's config dir sets for this backend; absent from an orx that sends only
    /// the dir.
    #[arg(skip)]
    #[serde(default)]
    pub caller_compute_settings: Option<crate::commands::up::LaunchSettings>,
}

/// Markers the native agent CLIs export to their shells; nested agents are `unknown`.
/// Antigravity exports none.
pub(crate) fn agent_origin() -> Option<String> {
    let found: Vec<_> = [
        ("CLAUDECODE", "claude-code"),
        ("CODEX_THREAD_ID", "codex"),
        ("OPENCODE", "opencode"),
        ("CURSOR_AGENT", "cursor"),
    ]
    .into_iter()
    .filter(|(key, _)| std::env::var(key).is_ok_and(|value| !value.is_empty()))
    .map(|(_, harness)| harness.to_string())
    .collect();
    match found.len() {
        0 => None,
        1 => found.into_iter().next(),
        _ => Some("unknown".into()),
    }
}

impl ExpRunArgs {
    pub(crate) fn invocation_identity(
        &self,
    ) -> crate::error::Result<Option<crate::store::InvocationIdentity>> {
        let context = self
            .invocation_context
            .clone()
            .or_else(|| (!self.forwarded).then(|| std::env::var("ORX_INVOCATION_CONTEXT").ok())?);
        let identity: Option<crate::store::InvocationIdentity> = match context {
            Some(json) => Some(serde_json::from_str(&json)?),
            // Codex exports only its thread id to shells; that thread's running turn invoked us.
            None if !self.forwarded && self.launching_chat_session().is_some() => {
                std::env::var("CODEX_THREAD_ID")
                    .ok()
                    .and_then(|thread| crate::local::harness::codex::running_turn_identity(&thread))
            }
            None => None,
        };
        if let Some(identity) = &identity {
            identity.validate()?;
        }
        Ok(identity)
    }

    pub fn launching_chat_session(&self) -> Option<String> {
        self.chat_session_id.clone().or_else(|| {
            (!self.forwarded)
                .then(crate::local::chat::launching_chat_session)
                .flatten()
        })
    }
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Port to bind on 127.0.0.1 (default 4790 — what the api proxies to).
    #[arg(long)]
    pub port: Option<u16>,
}

#[derive(Args, Debug)]
pub struct UpArgs {
    /// Port to listen on. With `--remote`, the local presentation port.
    #[arg(long, default_value_t = 4791)]
    pub port: u16,
    /// Address to listen on: `127.0.0.1`, or `0.0.0.0` to also serve devices on
    /// your private network. The dashboard has no login, so anyone who can
    /// reach it controls your agents and terminals; only do this on a network
    /// you trust. Public peers, public addresses and hostnames stay refused.
    #[arg(
        long,
        default_value = "127.0.0.1",
        value_name = "ADDR",
        conflicts_with_all = ["remote", "remote_host"]
    )]
    pub host: std::net::IpAddr,
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
    /// Keep the engine running without opening a window.
    #[arg(long, hide = true)]
    pub headless: bool,
    /// Don't spawn the opencode agent on startup (for tests).
    #[arg(long)]
    pub no_agent: bool,
    /// opencode model override, e.g. `anthropic/claude-sonnet-4-5`.
    #[arg(long)]
    pub model: Option<String>,
    /// Internal persistent dashboard/agent-host mode.
    #[arg(long, hide = true)]
    pub remote_host: bool,
    /// Serving the desktop app's window, which only restarts when asked.
    #[arg(skip)]
    pub desktop_app: bool,
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
pub struct LibraryArgs {
    #[command(subcommand)]
    pub command: LibraryCommand,
}

#[derive(Subcommand, Debug)]
pub enum LibraryCommand {
    /// Save a file or ZIP across projects; replaces an existing entry of the same name.
    Add { path: std::path::PathBuf },
}

#[derive(Args, Debug)]
pub struct InstallSkillsArgs {
    /// Which agent(s) to install into: `claude`, `codex`, `opencode`, `cursor`,
    /// `antigravity`, or `all`. Defaults to every agent already set up on this machine.
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

#[derive(Args, Debug)]
pub struct FeedbackArgs {
    #[arg(long, value_enum)]
    pub kind: FeedbackKind,
    /// One line, at most 200 characters.
    #[arg(long)]
    pub summary: String,
    /// For bugs, include as much detail as possible: non-sensitive input,
    /// command and flags, error, expected and actual behavior, environment,
    /// and workaround; at most 4000 characters. Omit sensitive information.
    #[arg(long)]
    pub details: String,
    /// The user's own words, rephrased to strip sensitive details; at most 1000
    /// characters.
    #[arg(long)]
    pub quote: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[value(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    Bug,
    FeatureRequest,
    Frustration,
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
    /// PubMed biomedical literature (NCBI E-utilities).
    Pubmed,
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
            LitSource::Pubmed => "pubmed",
        }
    }

    /// Human-facing name for error/UI text.
    pub fn display_name(&self) -> &'static str {
        match self {
            LitSource::Alphaxiv => "alphaXiv",
            LitSource::Openalex => "OpenAlex",
            LitSource::Biorxiv => "bioRxiv",
            LitSource::Pubmed => "PubMed",
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
    /// OpenAlex scholarly-graph search across disciplines. Set OPENALEX_API_KEY (free:
    /// help.openalex.org/api/authentication) for a daily budget of its own.
    Openalex(DiscoverySearchArgs),
    /// bioRxiv preprint search through OpenAlex's bioRxiv source index.
    Biorxiv(DiscoverySearchArgs),
    /// PubMed biomedical literature search through NCBI E-utilities.
    Pubmed(DiscoverySearchArgs),
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
    /// Print the desktop build identity.
    #[arg(long, hide = true, conflicts_with_all = ["check", "json", "build_channel", "dashboard_protocol"])]
    pub build_id: bool,
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
    /// other), an OpenAlex `W…` id, or a PubMed PMID / URL. The source is auto-detected.
    pub id: String,
    /// Force the source instead of auto-detecting it from the id.
    #[arg(long, value_enum)]
    pub source: Option<LitSource>,
    /// Fetch the full extracted paper text instead of the report (alphaXiv only;
    /// OpenAlex/bioRxiv/PubMed have no extracted full text and point you at a full-text link).
    #[arg(long)]
    pub full: bool,
}

fn main() {
    if let Err(error) = commands::remote_host::prepare_readiness_pipe() {
        eprintln!("Could not prepare server readiness: {error}");
        std::process::exit(1);
    }
    run_cli();
}

// Desktop app mode blocks the main thread while the dashboard runs on worker threads.
#[tokio::main]
async fn run_cli() {
    #[cfg(all(target_os = "macos", debug_assertions))]
    if std::env::args().nth(1).as_deref() == Some("--dev-desktop") {
        let port = std::env::args()
            .nth(2)
            .and_then(|arg| arg.parse::<u16>().ok());
        if let Some(port) = port.filter(|port| *port != 0) {
            commands::app::run_dev(port);
        } else {
            eprintln!("--dev-desktop requires a valid UI port");
            std::process::exit(1);
        }
        return;
    }
    if std::env::var_os("ORX_ENGINE_ASKPASS").is_some() {
        if engine::transport::askpass::relay().await.is_err() {
            std::process::exit(1);
        }
        return;
    }
    if std::env::var_os("ORX_SSH_ASKPASS_ENDPOINT").is_some() {
        if jobs::ssh::service::askpass().await.is_err() {
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("ssh-service") {
        let result = if std::env::args().nth(2).as_deref() == Some("stop") {
            jobs::ssh::service::shutdown_idle().await
        } else {
            jobs::ssh::service::run().await
        };
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(windows)]
    {
        install_panic_reporter();
        updates::remove_retired_exes();
    }
    // Double-clicked as the macOS .app? Enter GUI app mode (Dock icon, dashboard
    // server, window) instead of parsing CLI args. Also require an empty argv so
    // the bundled binary stays usable as a CLI (`…/MacOS/OpenResearch up`), since
    // the bundle itself launches it with no arguments. See commands::app.
    #[cfg(target_os = "macos")]
    if commands::app::launched_as_app_bundle() && std::env::args_os().len() == 1 {
        // Shell hydration may change XDG_CONFIG_HOME; settle it before telemetry or the lifecycle lock.
        commands::app::hydrate_shell_env().await;
        telemetry::set_flag(false);
        commands::app::run().await;
        return;
    }
    // Started by the Windows app's launcher, OpenResearch.exe, or the Linux
    // AppImage's AppRun. See commands::app.
    #[cfg(all(desktop_app, not(target_os = "macos")))]
    if commands::app::launched_with_app_arg() {
        telemetry::set_flag(std::env::var_os(commands::app::APP_NO_TELEMETRY_ENV).is_some());
        commands::app::run().await;
        return;
    }

    let cli = Cli::parse();
    // Double-clicked from Explorer: open the desktop app instead of a console session.
    #[cfg(windows)]
    if cli.command.is_none() && owns_its_console() {
        relaunch_as_app(cli.no_telemetry);
        return;
    }
    let Some(command) = cli.command else {
        // Bare `orx`: print the command overview to stdout and exit 0.
        use clap::CommandFactory;
        Cli::command().print_help().ok();
        return;
    };
    if ends_with_its_stdout(&command) {
        output::end_when_stdout_closes();
    }
    // Outdated-version warning (skipped for the commands that manage updates
    // themselves). `start` prints the cached warning to stderr *now*,
    // before the command runs, so it shows even for commands that
    // `std::process::exit` on their own (e.g. the "not logged in" path) instead
    // of returning here. Never touches stdout or the exit code. Silence it with
    // ORX_NO_UPDATE_CHECK / NO_UPDATE_NOTIFIER.
    // `plan-gate` is a per-tool-call hook body (fires on every Bash call during
    // plan mode): it must stay fast and touch neither stdout nor the network, so
    // skip the update check and telemetry and run it directly.
    if matches!(command, Command::InvocationGate) {
        // Fail open: a missing model only loses attribution, a deny blocks every Bash call.
        if let Err(error) = commands::invocation_gate::run().await {
            eprintln!("orx invocation-gate: {error}");
        }
        return;
    }
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
    // `orx mcp` is the agent's stdio MCP child for the turn: stdout is the MCP
    // channel (nothing else may write to it) and startup must be instant or
    // Claude times the server out — skip the update check and telemetry.
    if matches!(command, Command::AntigravityGate) {
        if let Err(error) = commands::mcp::run_antigravity().await {
            eprintln!("orx antigravity-gate: {error}");
            std::process::exit(1);
        }
        return;
    }
    if matches!(command, Command::AlphaxivMcp) {
        if let Err(error) = commands::alphaxiv_mcp::run().await {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if matches!(command, Command::Mcp) {
        if let Err(err) = commands::mcp::run().await {
            // stderr only; a failed bridge degrades plan mode, never the CLI.
            eprintln!("orx mcp: {err}");
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
        let publish = || -> error::Result<()> {
            let lock = store::open_lifecycle_lock()?;
            let _guard = lock.read()?;
            local::git::push_branch(&args.repo_path, &args.branch, &args.owner, &args.repo)
        };
        if let Err(err) = publish() {
            eprintln!("orx publish-branch: {err}");
            std::process::exit(1);
        }
        return;
    }

    let warning = (!matches!(
        command,
        Command::Version(_) | Command::Update(_) | Command::Delete(_) | Command::Feedback(_)
    ))
    // `orx up` updates from its own periodic pass, once it holds the backend lock that defers it.
    .then(|| updates::UpdateWarning::start(!matches!(command, Command::Up(_))));

    // Anonymous usage analytics. Record the flag process-globally so command
    // modules can fire events without threading it through, then fire the
    // per-invocation event *before* dispatch so commands that exit on their own
    // (e.g. the "not logged in" path) are still counted. Opt out with
    // --no-telemetry or `orx telemetry off`.
    telemetry::set_flag(cli.no_telemetry);
    let session = telemetry::TelemetrySession::start(
        should_capture_command(&command).then(|| command_name(&command)),
        match &command {
            Command::Up(args) => Some(telemetry::UpLaunchMode::of(args)),
            _ => None,
        },
    );

    let result = dispatch(command).await;
    if let Some(warning) = warning {
        warning.finish().await;
    }
    session.finish(result.is_ok()).await;

    if let Err(err) = result {
        // Match the TS: print only the message, exit 1.
        eprintln!("{}", err);
        show_error_dialog(&err.to_string());
        // sysexits' EX_TEMPFAIL: the app's sign-in dialog says to try again in a moment.
        let code = if err.is::<client::AlphaXivUnavailable>() {
            75
        } else {
            1
        };
        std::process::exit(code);
    }
}

/// Explorer gives a double-clicked exe a console of its own, which closes when it exits.
#[cfg(windows)]
fn owns_its_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;

    let mut attached = [0u32; 2];
    // SAFETY: writes at most `attached.len()` process ids into `attached`.
    let count = unsafe { GetConsoleProcessList(attached.as_mut_ptr(), attached.len() as u32) };
    count == 1
}

/// Restarts as `orx app` in a console no one sees, as OpenResearch.exe does, so
/// the console Explorer opened closes when this process exits.
#[cfg(windows)]
fn relaunch_as_app(no_telemetry: bool) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    let started = std::env::current_exe().and_then(|exe| {
        let mut app = crate::process::command(exe);
        if no_telemetry {
            app.env(commands::app::APP_NO_TELEMETRY_ENV, "1");
        }
        app.arg(commands::app::APP_ARG)
            // Inherited handles would tie the app to Explorer's console.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    });
    match started {
        // Explorer let this process take the foreground; pass that to the app's window.
        // SAFETY: a plain syscall on the child's process id.
        Ok(child) => unsafe {
            AllowSetForegroundWindow(child.id());
        },
        Err(error) => show_error_dialog(&format!("Could not start OpenResearch: {error}")),
    }
}

/// A double-clicked exe's console closes with it, so repeat the error in a dialog.
#[cfg(windows)]
fn show_error_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    // The app's console is hidden, and shared with the agents it runs, so it
    // doesn't own it.
    if !owns_its_console() && !commands::app::launched_with_app_arg() {
        return;
    }
    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (body, title) = (wide(message), wide("OpenResearch stopped"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}

#[cfg(target_os = "macos")]
fn show_error_dialog(message: &str) {
    if !commands::app::launched_as_app_bundle() || std::env::args_os().len() != 1 {
        return;
    }
    let _ = crate::process::command("osascript")
        .args([
            "-e",
            "on run argv\ndisplay alert \"OpenResearch could not start\" message (item 1 of argv) as critical\nend run",
            "--",
            message,
        ])
        .output();
}

/// A launcher-started AppImage has no terminal to read, so show the error with
/// whichever desktop's dialog tool is installed, as the folder picker does.
#[cfg(all(desktop_app, target_os = "linux"))]
fn show_error_dialog(message: &str) {
    if !commands::app::launched_with_app_arg() {
        return;
    }
    eprintln!("OpenResearch: {message}");
    for (program, args) in [
        (
            "zenity",
            &["--error", "--no-markup", "--title=OpenResearch", "--text"][..],
        ),
        ("kdialog", &["--title", "OpenResearch", "--error"][..]),
    ] {
        let mut dialog = crate::process::command(program);
        dialog.args(args).arg(message);
        local::shell_env::restore_host_gui_env(&mut dialog);
        if dialog.status().is_ok() {
            return;
        }
    }
}

#[cfg(not(any(windows, target_os = "macos", all(desktop_app, target_os = "linux"))))]
fn show_error_dialog(_message: &str) {}

/// Main thread only: a worker's panic has a running dashboard to report through.
#[cfg(windows)]
fn install_panic_reporter() {
    let inner = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        inner(info);
        if std::thread::current().name() == Some("main") {
            show_error_dialog(&format!("{info}"));
        }
    }));
}

/// Servers, protocol children and per-call hooks outlive a closed stdout; every other
/// command ends with it, as `orx runs | head` expects.
fn ends_with_its_stdout(command: &Command) -> bool {
    !matches!(
        command,
        Command::Up(_)
            | Command::Serve(_)
            | Command::Mcp
            | Command::AlphaxivMcp
            | Command::AntigravityGate
            | Command::RemoteHost(_)
            | Command::PlanGate
            | Command::InvocationGate
    )
}

fn should_capture_command(command: &Command) -> bool {
    !matches!(
        command,
        Command::Update(UpdateArgs {
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
        Command::Alphaxiv(_) => "alphaxiv",
        Command::Projects(_) => "projects",
        Command::Orgs(_) => "orgs",
        Command::Project(_) => "project",
        Command::Runs(_) => "runs",
        Command::Logs(_) => "logs",
        Command::Tensorboard(_) => "tensorboard",
        Command::Metrics(_) => "metrics",
        Command::CreateExperiment(_) => "create-experiment",
        Command::Compute(_) => "compute",
        Command::Instance(_) => "instance",
        Command::SshKey(_) => "ssh-key",
        Command::Exp(_) => "exp",
        Command::Skill(_) => "skill",
        Command::Skills(_) => "skills",
        Command::Templates(_) => "templates",
        Command::InstallSkills(_) => "install-skills",
        Command::Discover(_) => "discover",
        Command::Paper(_) => "paper",
        Command::Version(_) => "version",
        Command::Update(_) => "update",
        Command::InstallCli(_) => "install-cli",
        Command::Delete(_) => "delete",
        Command::Serve(_) => "serve",
        Command::Up(_) => "up",
        Command::Telemetry(_) => "telemetry",
        Command::Feedback(_) => "feedback",
        Command::PlanGate => "plan-gate",
        Command::InvocationGate => "invocation-gate",
        Command::Mcp => "mcp",
        Command::AlphaxivMcp => "alphaxiv-mcp",
        Command::AntigravityGate => "antigravity-gate",
        Command::PublishBranch(_) => "publish-branch",
        Command::RemoteHost(_) => "remote-host",
    }
}

async fn dispatch(command: Command) -> error::Result<()> {
    let uses_lock = command_uses_lifecycle_lock(&command);
    if uses_lock {
        local::storage::prepare().await?;
    }
    let lifecycle_lock = uses_lock.then(store::open_lifecycle_lock).transpose()?;
    let _lifecycle_guard = lifecycle_lock
        .as_ref()
        .map(|lock| lock.read())
        .transpose()?;

    match command {
        Command::Login(args) => commands::login::run(args).await,
        Command::Logout => commands::logout::run().await,
        Command::Alphaxiv(args) => match args.command {
            AlphaXivCommand::Library { json } => alphaxiv::print_library(json).await,
        },
        Command::Projects(args) => commands::projects::run(args).await,
        Command::Orgs(args) => commands::orgs::run(args).await,
        Command::Project(args) => commands::project::run(args).await,
        Command::Runs(args) => commands::runs::run(args).await,
        Command::Logs(args) => commands::logs::run(args).await,
        Command::Tensorboard(args) => commands::tensorboard::run(args).await,
        Command::Metrics(args) => commands::metrics::run(args).await,
        Command::CreateExperiment(args) => commands::create_experiment::run(args).await,
        Command::Compute(args) => commands::compute::run(args).await,
        Command::Instance(args) => commands::instance::run(args).await,
        Command::SshKey(args) => match args.command {
            SshKeyCommand::Add(a) => commands::ssh_key::add(a.path).await,
            SshKeyCommand::List => commands::ssh_key::list().await,
        },
        Command::Exp(args) => commands::exp::run(args).await,
        Command::Skill(args) => commands::skill::run(args).await,
        Command::Skills(args) => commands::library::skills(args),
        Command::Templates(args) => commands::library::templates(args),
        Command::InstallSkills(args) => commands::install_skills::run(args).await,
        Command::Discover(args) => commands::discover::run(args).await,
        Command::Paper(args) => commands::paper::run(args).await,
        Command::Version(args) => commands::version::run(args).await,
        Command::Update(args) => commands::update::run(args).await,
        Command::InstallCli(args) => commands::install_cli::run(args).await,
        Command::Delete(args) => commands::delete::run(args).await,
        Command::Serve(args) => commands::serve::run(args).await,
        Command::Up(mut args) => {
            if args.headless {
                args.no_browser = true;
            }
            match args.remote.clone() {
                Some(host) => commands::up_remote::run(&host, args).await,
                None => commands::up::run(args).await,
            }
        }
        Command::Telemetry(args) => commands::telemetry::run(args).await,
        Command::Feedback(args) => commands::feedback::run(args).await,
        // Handled before dispatch (fast path, no telemetry/update check).
        Command::PlanGate => commands::plan_gate::run().await,
        Command::InvocationGate => commands::invocation_gate::run().await,
        Command::Mcp => commands::mcp::run().await,
        Command::AntigravityGate => commands::mcp::run_antigravity().await,
        Command::AlphaxivMcp => unreachable!("handled before dispatch"),
        Command::PublishBranch(_) => unreachable!("handled before dispatch"),
        Command::RemoteHost(_) => unreachable!("handled before dispatch"),
    }
}

fn command_uses_lifecycle_lock(command: &Command) -> bool {
    !matches!(
        command,
        Command::Login(_)
            | Command::Logout
            | Command::Alphaxiv(_)
            | Command::InstallSkills(_)
            | Command::Discover(_)
            | Command::Paper(_)
            | Command::Version(_)
            | Command::Delete(_)
            | Command::Telemetry(_)
            | Command::Feedback(_)
            | Command::PlanGate
            | Command::InvocationGate
            | Command::AlphaxivMcp
            | Command::Mcp
            | Command::AntigravityGate
            | Command::PublishBranch(_)
            | Command::RemoteHost(_)
    )
}

#[cfg(test)]
mod cli_tests {
    use super::*;

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
    fn discover_parses_openalex_biorxiv_and_pubmed_primitives() {
        for (source, expected) in [
            ("openalex", LitSource::Openalex),
            ("biorxiv", LitSource::Biorxiv),
            ("pubmed", LitSource::Pubmed),
        ] {
            let cli = Cli::try_parse_from(["orx", "discover", source, "protein folding"])
                .expect("source discovery should parse");
            let Some(Command::Discover(DiscoverArgs { command })) = cli.command else {
                panic!("expected discover command");
            };
            let actual = match command {
                DiscoverCommand::Openalex(_) => LitSource::Openalex,
                DiscoverCommand::Biorxiv(_) => LitSource::Biorxiv,
                DiscoverCommand::Pubmed(_) => LitSource::Pubmed,
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
