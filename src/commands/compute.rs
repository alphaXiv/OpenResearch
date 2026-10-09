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

/// Remaining credit and GPU prices across providers, so a run can go where it
/// is cheapest and will not run out of credit midway.
async fn prices(args: PricesArgs, json: bool) -> Result<()> {
    use crate::jobs::provider_prices::overview;
    let mut overview = overview(args.fresh).await;
    overview
        .providers
        .retain(|provider| args.all || provider.configured);
    if let Some(gpu) = &args.gpu {
        let gpu = gpu.to_ascii_lowercase();
        for provider in &mut overview.providers {
            provider.offers.retain(|offer| {
                offer
                    .gpu
                    .as_ref()
                    .is_some_and(|model| model.to_ascii_lowercase().starts_with(&gpu))
            });
        }
    }
    if json {
        println!("{}", serde_json::to_string(&overview)?);
        return Ok(());
    }
    let money = |value: f64| format!("${value:.2}");
    let mut balances = Vec::new();
    let mut rows = Vec::new();
    for provider in &overview.providers {
        let credit = match (&provider.balance, &provider.spend) {
            (Some(balance), _) => {
                let mut text = format!("{:.1} {}", balance.amount, balance.unit);
                if let Some(usd) = balance.usd {
                    text.push_str(&format!(" (≈{})", money(usd)));
                }
                if let Some(burn) = balance.burning_per_hour.filter(|burn| *burn > 0.0) {
                    text.push_str(&format!(", spending {burn:.2}/h now"));
                }
                text
            }
            (None, Some(spend)) => format!(
                "{} used this month, {} billed{}",
                spend.metered_usd.map_or("?".into(), money),
                spend.billed_usd.map_or("?".into(), money),
                spend.credits_usd.filter(|credits| *credits > 0.0).map_or(
                    String::new(),
                    |credits| format!(", {} covered by credits", money(credits))
                ),
            ),
            (None, None) => match provider.billing {
                "own" => "own hardware".into(),
                "tokens" => "billed per token".into(),
                "credits" => "prepaid, balance unknown".into(),
                _ => "pay as you go".into(),
            },
        };
        let status = if provider.configured {
            ""
        } else {
            " (not connected)"
        };
        balances.push(vec![
            format!("{}{status}", provider.id),
            credit,
            provider.error.clone().unwrap_or_default(),
        ]);
        for offer in provider.offers.iter().filter(|offer| offer.gpu.is_some()) {
            let mut price = offer.usd_per_hour.map_or("—".into(), money);
            if let Some(units) = offer.units_per_hour {
                price = format!("{price} ({units:.2} CU)");
            }
            if offer.estimated {
                price = format!("≈{price}");
            }
            rows.push(vec![
                provider.id.to_string(),
                offer.flavor.clone(),
                format!(
                    "{}{}",
                    offer.gpu.clone().unwrap_or_default(),
                    if offer.gpu_count > 1 {
                        format!(" ×{}", offer.gpu_count)
                    } else {
                        String::new()
                    }
                ),
                offer
                    .vram_gb
                    .map_or("—".into(), |vram| format!("{vram:.0}")),
                price,
                offer
                    .runway_hours
                    .map_or("—".into(), |hours| format!("{hours:.1} h")),
                match offer.available {
                    Some(false) => "not on plan".into(),
                    _ => offer.host.clone().unwrap_or_default(),
                },
            ]);
        }
    }
    print_table(&["PROVIDER", "CREDIT", "NOTE"], &balances);
    println!();
    rows.sort_by(|a, b| price_key(&a[4]).total_cmp(&price_key(&b[4])));
    if rows.is_empty() {
        println!("No GPU prices to show.");
    } else {
        print_table(
            &[
                "PROVIDER", "FLAVOR", "GPU", "VRAM(GB)", "$/HR", "RUNWAY", "NOTE",
            ],
            &rows,
        );
        println!("\n≈ marks a published or estimated price rather than the account's own rate.");
    }
    Ok(())
}

fn price_key(cell: &str) -> f64 {
    cell.trim_start_matches(['≈', '$'])
        .split(' ')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(f64::INFINITY)
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
    Colab,
    K8s,
    Openresearch,
    Gcp,
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
            Self::Colab => "colab",
            Self::K8s => "k8s",
            Self::Openresearch => "openresearch",
            Self::Gcp => "gcp",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ComputeCommand {
    /// Browse cloud offers (also the behavior without a subcommand).
    Catalog(CatalogArgs),
    /// Remaining credit and GPU prices on every provider, cheapest first.
    Prices(PricesArgs),
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
pub struct PricesArgs {
    /// Re-read balances and prices instead of using the five-minute cache.
    #[arg(long)]
    pub fresh: bool,
    /// Only offers with this GPU model (e.g. `a100`). Case-insensitive.
    #[arg(long)]
    pub gpu: Option<String>,
    /// Include providers that are not connected.
    #[arg(long)]
    pub all: bool,
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
    #[arg(long)]
    default_host: Option<String>,
    #[arg(long)]
    partition: Option<String>,
    #[arg(long)]
    account: Option<String>,
    #[arg(long)]
    time_limit: Option<String>,
    #[arg(long)]
    context: Option<String>,
    #[arg(long)]
    namespace: Option<String>,
    #[arg(long)]
    address: Option<String>,
    /// Google Cloud project that owns and pays for the VMs (gcp).
    #[arg(long)]
    project: Option<String>,
    /// Compute Engine zone for new VMs, e.g. us-central1-a (gcp).
    #[arg(long)]
    zone: Option<String>,
    /// Use cheaper Spot VMs that Google can reclaim mid-run (gcp).
    #[arg(long)]
    spot: Option<bool>,
    /// VM image family and its project (gcp; defaults to a Deep Learning VM image).
    #[arg(long)]
    image_family: Option<String>,
    #[arg(long)]
    image_project: Option<String>,
    /// Boot disk size in GB (gcp).
    #[arg(long)]
    disk_gb: Option<u64>,
    /// Clear a saved field; repeat for multiple fields (e.g. --clear time-limit).
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
    /// Idle lifetime for a new SSH master, in seconds (default: 86400; 0 means indefinite).
    #[arg(long)]
    persist: Option<u64>,
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
            "ssh" => &["host", "container", "defaultHost"],
            "slurm" => &["host", "partition", "account", "timeLimit"],
            "k8s" => &["context", "namespace"],
            "ray" => &["address"],
            "hf" => &["token"],
            "tinker" => &["key"],
            "modal" => &["tokenId", "tokenSecret"],
            "gcp" => &[
                "project",
                "zone",
                "spot",
                "imageFamily",
                "imageProject",
                "diskGb",
            ],
            _ => return Err(anyhow!("{backend} has no editable settings.")),
        };
        let mut body = Map::new();
        for (key, value) in [
            ("host", self.host),
            ("container", self.container),
            ("defaultHost", self.default_host),
            ("partition", self.partition),
            ("account", self.account),
            ("timeLimit", self.time_limit),
            ("context", self.context),
            ("namespace", self.namespace),
            ("address", self.address),
            ("project", self.project),
            ("zone", self.zone),
            ("imageFamily", self.image_family),
            ("imageProject", self.image_project),
        ] {
            if let Some(value) = value {
                body.insert(key.into(), json!(value));
            }
        }
        if let Some(spot) = self.spot {
            body.insert("spot".into(), json!(spot));
        }
        if let Some(disk) = self.disk_gb {
            body.insert("diskGb".into(), json!(disk));
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
                "default-host" => "defaultHost",
                "image-family" => "imageFamily",
                "image-project" => "imageProject",
                other => other,
            };
            if !allowed.contains(&key)
                || matches!(key, "token" | "key" | "tokenId" | "tokenSecret")
                || (backend == "ssh" && key == "host")
                || matches!(key, "spot" | "diskGb")
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
        if backend == "ssh" && body.contains_key("host") && !body.contains_key("container") {
            return Err(anyhow!("--host selects a container setting; supply --container or --clear container. Use --default-host to select a default."));
        }
        if backend == "ssh" && body.contains_key("container") && !body.contains_key("host") {
            return Err(anyhow!("A container change requires --host."));
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
        if args.persist.is_some()
            && (!cfg!(unix) || !matches!(args.check.backend, Backend::Ssh | Backend::Slurm))
        {
            return Err(anyhow!("--persist requires SSH or Slurm on Unix."));
        }
        if json_output {
            return Err(anyhow!(
                "connect is interactive; use test --json for machine-readable checks."
            ));
        }
        connect(&args.check, args.persist).await?;
    }
    let is_check = matches!(
        command,
        ComputeCommand::Test(_) | ComputeCommand::Connect(_)
    );
    let value = match command {
        ComputeCommand::Catalog(a) => return catalog(a, json_output).await,
        ComputeCommand::Prices(a) => return prices(a, json_output).await,
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
        ComputeCommand::Test(args) | ComputeCommand::Connect(ConnectArgs { check: args, .. }) => {
            settings::check(
                args.backend.name(),
                args.host,
                args.container,
                args.no_container,
                args.address,
            )
            .await?
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

async fn connect(args: &CheckArgs, persist: Option<u64>) -> Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err(anyhow!("connect requires a terminal for login/MFA. Use configure with --credentials-file - for unattended credential setup."));
    }
    match args.backend {
        Backend::Ssh | Backend::Slurm => {
            let host = match &args.host {
                Some(host) => Some(host.clone()),
                None => match args.backend {
                    Backend::Ssh => crate::config::ssh_settings()?.default_host,
                    _ => {
                        crate::jobs::slurm::load_settings()?
                            .unwrap_or_default()
                            .host
                    }
                },
            }
            .ok_or_else(|| anyhow!("Pass --host or configure a default host."))?;
            let target = crate::jobs::ssh::SshTarget::alias(&host);
            let connection = crate::jobs::ssh::interactive_args(&target, persist).await?;
            interactive_command("ssh", &connection.args).await?;
            if let Some(warning) = crate::jobs::ssh::setup_connection_sharing(&target).await {
                eprintln!("orx: warning: {warning}");
            }
        }
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
            super::login::run(crate::LoginArgs { api_url: None }).await?;
            super::ssh_key::add(None).await?;
        }
        Backend::Colab => {
            // Any authenticated command starts the CLI's browser sign-in once.
            let cli = crate::jobs::colab::find_cli()
                .ok_or_else(|| anyhow!("{}", crate::jobs::colab::INSTALL_HINT))?;
            let status = tokio::process::Command::new(cli)
                .arg("sessions")
                .status()
                .await?;
            if !status.success() {
                return Err(anyhow!("colab sessions exited with {status}."));
            }
        }
        Backend::Gcp => {
            if crate::jobs::gcp::account().await.is_none() {
                interactive_command("gcloud", &["auth".into(), "login".into()]).await?;
            }
        }
        Backend::Local | Backend::Ray | Backend::K8s => {}
    }
    Ok(())
}

async fn interactive_command(program: &str, args: &[String]) -> Result<()> {
    let path = crate::local::shell_env::find_on_path(program).ok_or_else(|| {
        anyhow!("{program} is not installed. Install it, then retry compute connect.")
    })?;
    let mut command = tokio::process::Command::new(path);
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

#[cfg(unix)]
async fn secret_prompt(label: &str) -> Result<String> {
    use std::process::{Command, Stdio};
    struct RestoreEcho(String);
    impl Drop for RestoreEcho {
        fn drop(&mut self) {
            let _ = Command::new("stty")
                .arg(&self.0)
                .stdin(Stdio::inherit())
                .status();
            eprintln!();
        }
    }
    let original = Command::new("stty")
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
    if !Command::new("stty")
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
            Ok(value.trim().to_owned())
        }) => value?,
        _ = tokio::signal::ctrl_c() => Err(anyhow!("Credential entry cancelled.")),
    }
}
#[cfg(windows)]
async fn secret_prompt(label: &str) -> Result<String> {
    eprintln!("{label}");
    let output = std::process::Command::new("powershell.exe").args(["-NoProfile", "-Command", "$s = Read-Host 'Credential' -AsSecureString; [System.Net.NetworkCredential]::new('', $s).Password"]).stdin(std::process::Stdio::inherit()).output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "Credential prompt failed; use --credentials-file instead."
        ));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
            body(&["ssh", "--host", "lab", "--clear", "container"]).unwrap(),
            json!({"host":"lab", "container":null})
        );
        assert!(crate::Cli::try_parse_from(["orx", "compute", "status", "--json"]).is_ok());
    }
}
