use clap::{Args, Subcommand};

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
