//! Configure compute and inspect execution readiness without a dashboard server.

use super::up::compute_settings as settings;
use crate::error::anyhow;
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{json, Map, Value};

use crate::client::{list_catalog, list_cpu_catalog, Disk};
use crate::error::{require_credentials, Result};
use crate::output::print_table;

pub async fn run(args: crate::ComputeArgs) -> Result<()> {
    if let Some(command) = args.command {
        if args.catalog.cpu
            || args.catalog.gpu.is_some()
            || args.catalog.count.is_some()
            || args.catalog.provider.is_some()
        {
            return Err(anyhow!(
                "Catalog filters must follow compute catalog, or be used without a subcommand."
            ));
        }
        return run_command(command, args.json).await;
    }
    catalog(args.catalog, args.json).await
}

/// Lists the compute catalog. With `--cpu`, lists CPU-only offers; otherwise the
/// GPU catalog, optionally filtered by gpu id and/or count.
async fn catalog(args: CatalogArgs, json: bool) -> Result<()> {
    let CatalogArgs {
        cpu,
        gpu,
        count,
        provider,
    } = args;
    let creds = require_credentials().await;

    if cpu {
        return run_cpu(&creds, json).await;
    }

    let offers = list_catalog(&creds).await?.offers;

    // The API already returns offers sorted by price ascending; keep that order.
    let filtered: Vec<_> = offers
        .into_iter()
        .filter(|o| gpu.as_ref().is_none_or(|g| o.gpu.eq_ignore_ascii_case(g)))
        .filter(|o| count.is_none_or(|c| o.gpu_count == c))
        .filter(|o| {
            provider
                .as_ref()
                .is_none_or(|p| o.provider.eq_ignore_ascii_case(p))
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string(&filtered)?);
        return Ok(());
    }
    if filtered.is_empty() {
        println!("No matching compute offers.");
        if gpu.is_some() {
            println!("GPU ids look like RTX_3090 or H100_SXM; run `orx compute` to list them.");
        }
        return Ok(());
    }

    let rows: Vec<Vec<String>> = filtered
        .iter()
        .map(|o| {
            vec![
                o.gpu.clone(),
                o.gpu_count.to_string(),
                format!("${:.2}", o.price_per_hour),
                fmt_disk(&o.disk),
                format!("{:.0}", o.vcpus),
                format!("{:.0}", o.ram_gb),
                o.region.clone().unwrap_or_else(|| "—".to_string()),
                o.provider.clone(),
            ]
        })
        .collect();

    print_table(
        &[
            "GPU", "COUNT", "$/HR", "DISK", "VCPUS", "RAM(GB)", "REGION", "PROVIDER",
        ],
        &rows,
    );

    Ok(())
}

/// Formats an offer's disk pricing for the `DISK` column: sizable offers show a
/// per-GB/hour rate, fixed offers show the bundled capacity. Falls back to `—`
/// if the expected payload is missing for the given `sizable` flag.
fn fmt_disk(disk: &Disk) -> String {
    let value = if disk.sizable {
        disk.per_gb_hour.map(|r| format!("${:.4}/GB·hr", r))
    } else {
        disk.included_gb.map(|gb| format!("{:.0}GB incl", gb))
    };
    value.unwrap_or_else(|| "—".to_string())
}

/// Lists the CPU-only offer catalog as a price-sorted table. From a local
/// (`orx up`) project, launch one with `orx exp run --backend openresearch
/// --flavor <flavor>:<vcpus>`.
async fn run_cpu(creds: &crate::config::Credentials, json: bool) -> Result<()> {
    let offers = list_cpu_catalog(creds).await?.offers;

    if json {
        println!("{}", serde_json::to_string(&offers)?);
        return Ok(());
    }
    if offers.is_empty() {
        println!("No CPU compute offers available.");
        return Ok(());
    }

    let rows: Vec<Vec<String>> = offers
        .iter()
        .map(|o| {
            vec![
                o.cpu_flavor.clone(),
                format!("{:.0}", o.vcpus),
                format!("${:.2}", o.price_per_hour),
                fmt_disk(&o.disk),
                format!("{:.0}", o.ram_gb),
                o.region.clone().unwrap_or_else(|| "—".to_string()),
                o.provider.clone(),
            ]
        })
        .collect();

    print_table(
        &[
            "FLAVOR", "VCPUS", "$/HR", "DISK", "RAM(GB)", "REGION", "PROVIDER",
        ],
        &rows,
    );

    Ok(())
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Backend {
    Local,
    Ssh,
    Slurm,
    Ray,
    Hf,
    Modal,
    Tinker,
    K8s,
    Vercel,
    Openresearch,
}
impl Backend {
    fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ssh => "ssh",
            Self::Slurm => "slurm",
            Self::Ray => "ray",
            Self::Hf => "hf",
            Self::Modal => "modal",
            Self::Tinker => "tinker",
            Self::K8s => "k8s",
            Self::Vercel => "vercel",
            Self::Openresearch => "openresearch",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ComputeCommand {
    /// Browse cloud offers (also the behavior without a subcommand).
    Catalog(CatalogArgs),
    /// List backends and the machine-wide default.
    Status,
    /// Inspect backend configuration and hardware or credential sources.
    Show { backend: Backend },
    /// Change the default for future runs only.
    Default {
        #[command(subcommand)]
        command: DefaultCommand,
    },
    /// Save supplied settings; never implicitly select a default backend.
    Configure(Box<ConfigureArgs>),
    /// Test execution readiness. Failure exits nonzero.
    Test(CheckArgs),
    /// Authenticate interactively, then test readiness.
    Connect(ConnectArgs),
    /// Read or safely replace ~/.ssh/config.
    SshConfig {
        #[command(subcommand)]
        command: SshConfigCommand,
    },
    /// Machine-wide, user-owned compute guidance.
    Instructions {
        #[command(subcommand)]
        command: InstructionsCommand,
    },
}

#[derive(Debug, Args)]
pub struct CatalogArgs {
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
#[derive(Debug, Subcommand)]
pub enum DefaultCommand {
    Set {
        backend: Backend,
        #[arg(long)]
        flavor: Option<String>,
    },
    Clear,
}
#[derive(Debug, Args)]
pub struct ConfigureArgs {
    backend: Backend,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    container: Option<String>,
    /// Absolute remote dir for runs and the source cache (SSH per host, or Slurm); default ~/.orx.
    #[arg(long)]
    remote_root: Option<String>,
    /// true keeps every finished run's remote dir; false (the default) keeps each project's 3 newest
    /// SSH/Slurm runs and removes older ones except tensorboard/.
    #[arg(long)]
    keep_remote_runs: Option<bool>,
    #[arg(long)]
    default_host: Option<String>,
    #[arg(long)]
    partition: Option<String>,
    #[arg(long)]
    account: Option<String>,
    /// Default `#SBATCH --qos` for Slurm runs.
    #[arg(long)]
    qos: Option<String>,
    #[arg(long)]
    time_limit: Option<String>,
    /// Default `#SBATCH --cpus-per-task` for Slurm runs.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    cpus_per_task: Option<u32>,
    /// Default `#SBATCH --mem` for Slurm runs (64G, 4000M, or megabytes).
    #[arg(long)]
    mem: Option<String>,
    /// An extra `#SBATCH` option for every Slurm run, such as --sbatch-arg='--rsc p=1:g=1';
    /// repeat for several. Replaces the saved list.
    #[arg(long = "sbatch-arg", value_name = "ARG", allow_hyphen_values = true)]
    sbatch_args: Vec<String>,
    /// true (the default) writes `#SBATCH --ntasks=1` into every Slurm job; false leaves it out,
    /// for clusters that refuse it.
    #[arg(long)]
    request_one_task: Option<bool>,
    #[arg(long)]
    context: Option<String>,
    #[arg(long)]
    namespace: Option<String>,
    #[arg(long)]
    address: Option<String>,
    /// Clear a saved field; repeat for multiple fields (e.g. --clear time-limit, --clear mem).
    #[arg(long)]
    clear: Vec<String>,
    /// JSON credentials: {"token":...}, {"key":...}, or {"tokenId":...,"tokenSecret":...}. Use - for stdin.
    #[arg(long)]
    credentials_file: Option<String>,
}
#[derive(Debug, Args)]
pub struct ConnectArgs {
    #[command(flatten)]
    check: CheckArgs,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    backend: Backend,
    #[arg(long)]
    host: Option<String>,
    #[arg(long, conflicts_with = "no_container")]
    container: Option<String>,
    #[arg(long)]
    no_container: bool,
    #[arg(long)]
    address: Option<String>,
}
#[derive(Debug, Subcommand)]
pub enum SshConfigCommand {
    Show,
    Set {
        /// Replacement content; - reads stdin.
        #[arg(long)]
        file: String,
        /// File containing the exact previously read content.
        #[arg(long)]
        previous_file: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum InstructionsCommand {
    Show,
    /// Initialize the file if absent and print its absolute path.
    Path,
    Set {
        #[arg(long)]
        file: String,
        /// SHA-256 revision returned by show --json.
        #[arg(long)]
        expected_revision: String,
    },
}

fn read_input(path: &str) -> Result<String> {
    if path == "-" {
        Ok(std::io::read_to_string(std::io::stdin())?)
    } else {
        Ok(std::fs::read_to_string(path)?)
    }
}

impl ConfigureArgs {
    fn body(self) -> Result<Value> {
        let backend = self.backend.name();
        let allowed: &[&str] = match backend {
            "ssh" => &[
                "host",
                "container",
                "remoteRoot",
                "keepRemoteRuns",
                "defaultHost",
            ],
            "slurm" => &[
                "host",
                "partition",
                "account",
                "qos",
                "timeLimit",
                "cpusPerTask",
                "mem",
                "remoteRoot",
                "keepRemoteRuns",
                "sbatchArgs",
                "requestOneTask",
            ],
            "k8s" => &["context", "namespace"],
            "ray" => &["address"],
            "hf" => &["token"],
            "tinker" => &["key"],
            "modal" => &["tokenId", "tokenSecret"],
            _ => return Err(anyhow!("{backend} has no editable settings.")),
        };
        let mut body = Map::new();
        for (key, value) in [
            ("host", self.host),
            ("container", self.container),
            ("remoteRoot", self.remote_root),
            ("defaultHost", self.default_host),
            ("partition", self.partition),
            ("account", self.account),
            ("qos", self.qos),
            ("timeLimit", self.time_limit),
            ("mem", self.mem),
            ("context", self.context),
            ("namespace", self.namespace),
            ("address", self.address),
        ] {
            if let Some(value) = value {
                body.insert(key.into(), json!(value));
            }
        }
        if let Some(keep) = self.keep_remote_runs {
            body.insert("keepRemoteRuns".into(), json!(keep));
        }
        if let Some(one_task) = self.request_one_task {
            body.insert("requestOneTask".into(), json!(one_task));
        }
        if let Some(cpus) = self.cpus_per_task {
            body.insert("cpusPerTask".into(), json!(cpus));
        }
        if !self.sbatch_args.is_empty() {
            body.insert("sbatchArgs".into(), json!(self.sbatch_args));
        }
        if let Some(path) = self.credentials_file {
            let credentials: Map<String, Value> = serde_json::from_str(&read_input(&path)?)
                .map_err(|_| {
                    anyhow!(
                        "Credentials must be a JSON object with the documented credential fields."
                    )
                })?;
            for (key, value) in credentials {
                if !["token", "key", "tokenId", "tokenSecret"].contains(&key.as_str())
                    || !value.is_string()
                {
                    return Err(anyhow!("Unsupported credential field or value."));
                }
                body.insert(key, value);
            }
        }
        if self.clear.iter().any(|field| field == "credentials") {
            if !matches!(backend, "hf" | "modal" | "tinker")
                || self.clear.len() != 1
                || !body.is_empty()
            {
                return Err(anyhow!(
                    "--clear credentials must be the only change and applies to hf/modal/tinker."
                ));
            }
            return Ok(json!({"clearCredentials": true}));
        }
        for field in self.clear {
            let key = match field.as_str() {
                "time-limit" => "timeLimit",
                "cpus-per-task" => "cpusPerTask",
                "default-host" => "defaultHost",
                "remote-root" => "remoteRoot",
                "keep-remote-runs" => "keepRemoteRuns",
                "sbatch-arg" => "sbatchArgs",
                "request-one-task" => "requestOneTask",
                other => other,
            };
            if !allowed.contains(&key)
                || matches!(key, "token" | "key" | "tokenId" | "tokenSecret")
                || (backend == "ssh" && key == "host")
            {
                return Err(anyhow!("Cannot clear {field} for {backend}."));
            }
            if body.contains_key(key) {
                return Err(anyhow!("Cannot set and clear {field} together."));
            }
            body.insert(
                key.into(),
                if matches!(key, "container" | "defaultHost") {
                    Value::Null
                } else if key == "cpusPerTask" {
                    json!(0)
                } else if key == "keepRemoteRuns" {
                    json!(false)
                } else if key == "requestOneTask" {
                    json!(true)
                } else if key == "sbatchArgs" {
                    json!([])
                } else {
                    json!("")
                },
            );
        }
        for key in body.keys() {
            if !allowed.contains(&key.as_str()) {
                return Err(anyhow!("{key} is not a setting for {backend}."));
            }
        }
        if body.is_empty() {
            return Err(anyhow!(
                "Supply settings to change; use compute show {backend} to inspect."
            ));
        }
        let host_setting = ["container", "remoteRoot", "keepRemoteRuns"]
            .iter()
            .any(|key| body.contains_key(*key));
        if backend == "ssh" && body.contains_key("host") && !host_setting {
            return Err(anyhow!("--host needs --container, --remote-root, --keep-remote-runs, or a matching --clear; use --default-host to pick a default."));
        }
        if backend == "ssh" && host_setting && !body.contains_key("host") {
            return Err(anyhow!(
                "A container, remote root, or keep-remote-runs change requires --host."
            ));
        }
        Ok(Value::Object(body))
    }
}

async fn run_command(command: ComputeCommand, json_output: bool) -> Result<()> {
    if let ComputeCommand::Test(ref args) = command {
        validate_check(args)?;
    }
    if let ComputeCommand::Connect(ref args) = command {
        validate_check(&args.check)?;
        if json_output {
            return Err(anyhow!(
                "connect is interactive; use test --json for machine-readable checks."
            ));
        }
        if !matches!(args.check.backend, Backend::Ssh | Backend::Slurm) {
            connect(&args.check).await?;
        }
    }
    let is_check = matches!(
        command,
        ComputeCommand::Test(_) | ComputeCommand::Connect(_)
    );
    let value = match command {
        ComputeCommand::Test(args) => check(&args, "test").await?,
        ComputeCommand::Connect(ConnectArgs { check: args }) => check(&args, "connect").await?,
        ComputeCommand::Catalog(a) => return catalog(a, json_output).await,
        ComputeCommand::Status => settings::status().await?,
        ComputeCommand::Show { backend } => settings::show(backend.name()).await?,
        ComputeCommand::Default { command } => match command {
            DefaultCommand::Set { backend, flavor } => {
                settings::default(Some(backend.name().into()), flavor).await?
            }
            DefaultCommand::Clear => settings::default(None, None).await?,
        },
        ComputeCommand::Configure(args) => {
            let backend = args.backend.name();
            settings::configure(backend, (*args).body()?).await?
        }
        ComputeCommand::SshConfig { command } => match command {
            SshConfigCommand::Show => settings::read_ssh_config().await?,
            SshConfigCommand::Set {
                file,
                previous_file,
            } => {
                if file == "-" && previous_file == "-" {
                    return Err(anyhow!("Only one input can use stdin."));
                }
                settings::write_ssh_config(read_input(&file)?, read_input(&previous_file)?).await?
            }
        },
        ComputeCommand::Instructions { command } => {
            let path = instructions_path();
            match command {
                InstructionsCommand::Show => instructions_read(&path)?,
                InstructionsCommand::Path => {
                    instructions_init(&path)?;
                    if !json_output {
                        println!("{}", path.display());
                        return Ok(());
                    }
                    json!({"path": path})
                }
                InstructionsCommand::Set {
                    file,
                    expected_revision,
                } => instructions_write(&path, &read_input(&file)?, &expected_revision)?,
            }
        }
    };
    if json_output {
        println!("{}", serde_json::to_string(&value)?);
    } else if let Some(content) = value.get("content").and_then(Value::as_str) {
        print!("{content}");
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    if is_check && value["ready"] != true {
        return Err(anyhow!(
            "Compute is not ready; see the check results above."
        ));
    }
    Ok(())
}

/// Checks run in the server, which owns SSH connections and relays their login prompts.
async fn check(args: &CheckArgs, action: &str) -> Result<Value> {
    let body = json!({"backend":args.backend.name(), "host":args.host, "container":args.container, "noContainer":args.no_container, "address":args.address, "callerPythonOverrides":crate::jobs::caller_python_overrides()});
    let client = crate::plane::local_plane::ApiClient::discover().await?;
    if action == "connect" && matches!(args.backend, Backend::Ssh | Backend::Slurm) {
        use std::io::IsTerminal as _;
        if !std::io::stdin().is_terminal() {
            return Err(anyhow!("connect requires a terminal for login/MFA. Use configure with --credentials-file - for unattended credential setup."));
        }
        let prompt: crate::engine::transport::Prompt = std::sync::Arc::new(|question| {
            Box::pin(async move { Ok(Some(secret_prompt(&question).await?)) })
        });
        return client.connect(body, prompt).await;
    }
    client
        .json(
            reqwest::Method::POST,
            &format!("compute/{action}"),
            Some(&body),
        )
        .await
}

fn validate_check(args: &CheckArgs) -> Result<()> {
    let name = args.backend.name();
    if args.host.is_some() && !matches!(name, "ssh" | "slurm") {
        return Err(anyhow!("--host only applies to ssh/slurm."));
    }
    if (args.container.is_some() || args.no_container) && name != "ssh" {
        return Err(anyhow!("Container options only apply to ssh."));
    }
    if args.address.is_some() && name != "ray" {
        return Err(anyhow!("--address only applies to ray."));
    }
    Ok(())
}

fn instructions_path() -> std::path::PathBuf {
    crate::config::config_dir().join("compute/CUSTOM.md")
}
fn revision(content: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(content.as_bytes()))
}
fn instructions_init(path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("Missing compute directory"))?,
    )?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn instructions_read(path: &std::path::Path) -> Result<Value> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    Ok(json!({"path": path, "revision": revision(&content), "content": content}))
}
fn instructions_write(path: &std::path::Path, content: &str, expected: &str) -> Result<Value> {
    instructions_init(path)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write()?;
    if instructions_read(path)?["revision"] != expected {
        return Err(anyhow!(
            "Custom instructions changed. Read them again before saving."
        ));
    }
    crate::local::git::atomic_write_with_mode(path, content.as_bytes(), Some(0o600))?;
    instructions_read(path)
}

async fn connect(args: &CheckArgs) -> Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err(anyhow!("connect requires a terminal for login/MFA. Use configure with --credentials-file - for unattended credential setup."));
    }
    match args.backend {
        Backend::Hf => {
            interactive_command("hf", &["auth".into(), "login".into()]).await?;
            use crate::jobs::huggingface::{resolve_token_with_source, TokenSource};
            if let Ok((_, TokenSource::Env | TokenSource::OpenresearchEnv)) =
                resolve_token_with_source()
            {
                eprintln!("HF_TOKEN overrides the login cache. Unset the environment variable or run orx compute configure hf --clear credentials to use the new login.");
            }
        }
        Backend::Modal => {
            settings::configure("modal", json!({"tokenId": secret_prompt("Modal token ID").await?, "tokenSecret": secret_prompt("Modal token secret").await?})).await?;
        }
        Backend::Tinker => {
            settings::configure(
                "tinker",
                json!({"key": secret_prompt("Tinker API key").await?}),
            )
            .await?;
        }
        Backend::Openresearch => {
            super::login::run(crate::LoginArgs {
                alphaxiv: false,
                api_url: None,
                non_interactive: false,
                device: false,
            })
            .await?;
            super::ssh_key::add(None).await?;
        }
        Backend::Vercel => {
            eprintln!(
                "Vercel Sandbox reads VERCEL_TOKEN, VERCEL_TEAM_ID and VERCEL_PROJECT_ID (or a \
                 VERCEL_OIDC_TOKEN from `vercel env pull`) from the environment or Settings → \
                 Environment. Check them with `orx compute test vercel`."
            );
        }
        Backend::Local | Backend::Ray | Backend::K8s | Backend::Ssh | Backend::Slurm => {}
    }
    Ok(())
}

async fn interactive_command(program: &str, args: &[String]) -> Result<()> {
    let path = crate::local::shell_env::find(program).ok_or_else(|| {
        anyhow!("{program} is not installed. Install it, then retry compute connect.")
    })?;
    let mut command = crate::process::tokio_command(path);
    command.args(args);
    if program == "ssh" {
        command.env("ORX_SSH_PROBE", "1");
    }
    let status = command.status().await?;
    if !status.success() {
        return Err(anyhow!("{program} exited with {status}."));
    }
    Ok(())
}

fn credential_line(value: &str) -> &str {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
}

#[cfg(unix)]
async fn secret_prompt(label: &str) -> Result<String> {
    use std::process::Stdio;
    struct RestoreEcho(String);
    impl Drop for RestoreEcho {
        fn drop(&mut self) {
            let _ = crate::process::command("stty")
                .arg(&self.0)
                .stdin(Stdio::inherit())
                .status();
            eprintln!();
        }
    }
    let original = crate::process::command("stty")
        .arg("-g")
        .stdin(Stdio::inherit())
        .output()?;
    if !original.status.success() {
        return Err(anyhow!(
            "Cannot read terminal settings; use --credentials-file instead."
        ));
    }
    let original = String::from_utf8(original.stdout)?.trim().to_owned();
    eprint!("{label}: ");
    if !crate::process::command("stty")
        .arg("-echo")
        .stdin(Stdio::inherit())
        .status()?
        .success()
    {
        return Err(anyhow!(
            "Cannot disable terminal echo; use --credentials-file instead."
        ));
    }
    let _restore = RestoreEcho(original);
    tokio::select! {
        value = tokio::task::spawn_blocking(|| -> Result<String> {
            let mut value = String::new();
            std::io::stdin().read_line(&mut value)?;
            Ok(credential_line(&value).to_owned())
        }) => value?,
        _ = tokio::signal::ctrl_c() => Err(anyhow!("Credential entry cancelled.")),
    }
}
#[cfg(windows)]
async fn secret_prompt(label: &str) -> Result<String> {
    eprintln!("{label}");
    let output = crate::process::command("powershell.exe").args(["-NoProfile", "-Command", "$s = Read-Host 'Credential' -AsSecureString; [System.Net.NetworkCredential]::new('', $s).Password"]).stdin(std::process::Stdio::inherit()).output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "Credential prompt failed; use --credentials-file instead."
        ));
    }
    Ok(credential_line(&String::from_utf8(output.stdout)?).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn credential_entry_preserves_spaces_and_removes_only_the_submitted_newline() {
        for newline in ["\n", "\r\n", ""] {
            assert_eq!(
                credential_line(&format!(" \tsecret \t{newline}")),
                " \tsecret \t"
            );
        }
        assert_eq!(credential_line("\n"), "");
        assert_eq!(credential_line("secret\r"), "secret\r");
    }

    #[test]
    fn configuration_rejects_unsupported_and_conflicting_fields() {
        let body = |flags: &[&str]| {
            let cli = crate::Cli::try_parse_from(
                ["orx", "compute", "configure"]
                    .into_iter()
                    .chain(flags.iter().copied()),
            )
            .unwrap();
            let Some(crate::Command::Compute(crate::ComputeArgs {
                command: Some(ComputeCommand::Configure(args)),
                ..
            })) = cli.command
            else {
                panic!()
            };
            (*args).body()
        };
        assert!(body(&["ray", "--host", "lab"]).is_err());
        assert!(body(&["slurm", "--time-limit", "24h", "--clear", "time-limit"]).is_err());
        assert_eq!(
            body(&["slurm", "--clear", "time-limit"]).unwrap(),
            json!({"timeLimit":""})
        );
        assert_eq!(
            body(&["slurm", "--cpus-per-task", "8", "--mem", "64G"]).unwrap(),
            json!({"cpusPerTask":8, "mem":"64G"})
        );
        assert_eq!(
            body(&["slurm", "--clear", "cpus-per-task", "--clear", "mem"]).unwrap(),
            json!({"cpusPerTask":0, "mem":""})
        );
        assert!(body(&["ssh", "--host", "lab", "--mem", "64G"]).is_err());
        assert!(crate::Cli::try_parse_from([
            "orx",
            "compute",
            "configure",
            "slurm",
            "--cpus-per-task",
            "0"
        ])
        .is_err());
        assert_eq!(
            body(&["ssh", "--host", "lab", "--clear", "container"]).unwrap(),
            json!({"host":"lab", "container":null})
        );
        assert_eq!(
            body(&["ssh", "--host", "lab", "--remote-root", "/scratch/me/orx"]).unwrap(),
            json!({"host":"lab", "remoteRoot":"/scratch/me/orx"})
        );
        assert_eq!(
            body(&["slurm", "--clear", "remote-root"]).unwrap(),
            json!({"remoteRoot":""})
        );
        assert_eq!(
            body(&["ssh", "--host", "lab", "--keep-remote-runs", "true"]).unwrap(),
            json!({"host":"lab", "keepRemoteRuns":true})
        );
        assert!(body(&["ssh", "--remote-root", "/scratch/me/orx"]).is_err());
        assert_eq!(
            body(&[
                "slurm",
                "--qos",
                "acc_ehpc",
                "--sbatch-arg",
                "--rsc p=1:g=1",
                "--sbatch-arg=-G 1",
                "--request-one-task",
                "false",
            ])
            .unwrap(),
            json!({"qos":"acc_ehpc", "sbatchArgs":["--rsc p=1:g=1", "-G 1"], "requestOneTask":false})
        );
        assert_eq!(
            body(&[
                "slurm",
                "--clear",
                "qos",
                "--clear",
                "sbatch-arg",
                "--clear",
                "request-one-task"
            ])
            .unwrap(),
            json!({"qos":"", "sbatchArgs":[], "requestOneTask":true})
        );
        assert!(body(&["ssh", "--host", "lab", "--qos", "normal"]).is_err());
        assert!(crate::Cli::try_parse_from(["orx", "compute", "status", "--json"]).is_ok());
    }
}
