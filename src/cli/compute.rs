use clap::{Args, Subcommand};

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
