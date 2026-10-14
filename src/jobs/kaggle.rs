//! Kaggle backend — a local controller runs each experiment as a private
//! Kaggle kernel through the official `kaggle` CLI, on one of several Kaggle
//! accounts.
//!
//! The run is a local job (`kaggle_job`, same run-dir layout as `localbox`):
//! its `run.sh` starts `kaggle-controller.sh`, which
//!   1. uploads the immutable source snapshot as a private dataset
//!      (`kaggle datasets create`) and waits until Kaggle has processed it,
//!   2. pushes a script kernel (`kaggle kernels push`) that mounts the
//!      dataset, extracts the snapshot, runs the fixed command, and prints a
//!      sentinel with the exit code,
//!   3. follows the kernel's log (`kaggle kernels logs --follow`) and waits for
//!      it to finish, then downloads its output (`kaggle kernels output`),
//!   4. deletes the source dataset on every exit path.
//!
//! Accounts: every account lives in its own folder under the orx data dir,
//! which the CLI sees as its home (`HOME` and `KAGGLE_CONFIG_DIR`), so
//! credentials of one account never leak into a run on another and the CLI's
//! own `~/.kaggle` is left alone. The folder holds exactly what the CLI would
//! keep in `~/.kaggle` (`access_token`, `kaggle.json`, or the OAuth
//! `credentials.json`), readable by the owner only. orx never prints or logs
//! a credential; runs get the folder's path, not its contents.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::error::{anyhow, Result};

/// One Kaggle accelerator, as kernel metadata names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Accelerator {
    /// orx flavor id (lowercase).
    pub id: &'static str,
    /// `gpu`, `tpu`, or `cpu`.
    pub kind: &'static str,
    /// The kernel's `machine_shape`; empty for CPU.
    pub machine_shape: &'static str,
    pub label: &'static str,
    pub count: u32,
    /// Memory per accelerator in GB.
    pub memory_gb: u32,
}

pub const ACCELERATORS: &[Accelerator] = &[
    Accelerator {
        id: "cpu",
        kind: "cpu",
        machine_shape: "",
        label: "CPU only",
        count: 0,
        memory_gb: 0,
    },
    Accelerator {
        id: "p100",
        kind: "gpu",
        machine_shape: "NvidiaTeslaP100",
        label: "NVIDIA P100",
        count: 1,
        memory_gb: 16,
    },
    Accelerator {
        id: "t4",
        kind: "gpu",
        machine_shape: "NvidiaTeslaT4",
        label: "2× NVIDIA T4",
        count: 2,
        memory_gb: 15,
    },
    Accelerator {
        id: "tpu",
        kind: "tpu",
        machine_shape: "Tpu1VmV38",
        label: "TPU VM v3-8",
        count: 8,
        memory_gb: 16,
    },
];

/// Two T4s beat one P100 for most runs, and they share the GPU quota.
pub const DEFAULT_FLAVOR: &str = "t4";

/// Kaggle stops a session after 12 hours (9 on TPUs).
pub const MAX_TIMEOUT_SECS: u64 = 12 * 3600;
pub const MAX_TPU_TIMEOUT_SECS: u64 = 9 * 3600;
pub const DEFAULT_TIMEOUT_SECS: u64 = 4 * 3600;

/// Which account a run uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountChoice {
    /// The account marked active in settings.
    Active,
    /// The account with the most quota left for the flavor's accelerator.
    Auto,
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flavor {
    pub accelerator: Accelerator,
    pub account: AccountChoice,
}

impl Flavor {
    /// The canonical spelling, e.g. `t4@alice`.
    pub fn id(&self) -> String {
        match &self.account {
            AccountChoice::Active => self.accelerator.id.to_string(),
            AccountChoice::Auto => format!("{}@auto", self.accelerator.id),
            AccountChoice::Named(name) => format!("{}@{name}", self.accelerator.id),
        }
    }

    pub fn label(&self) -> String {
        let a = &self.accelerator;
        if a.memory_gb > 0 {
            format!("{} ({} GB each)", a.label, a.memory_gb)
        } else {
            a.label.to_string()
        }
    }

    pub fn timeout_cap(&self) -> u64 {
        if self.accelerator.kind == "tpu" {
            MAX_TPU_TIMEOUT_SECS
        } else {
            MAX_TIMEOUT_SECS
        }
    }
}

/// Parse `--flavor` for `--backend kaggle`.
///
/// - `t4` (default), `p100`, `tpu`, or `cpu`;
/// - `<id>@<account>` to run on one saved account, or `<id>@auto` for the
///   account with the most weekly quota left. Without `@`, the active account.
pub fn parse_flavor(flavor: Option<&str>) -> Result<Flavor> {
    let raw = flavor
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .unwrap_or(DEFAULT_FLAVOR);
    let (name, account) = match raw.split_once('@') {
        Some((name, account)) => (name.trim(), Some(account.trim())),
        None => (raw, None),
    };
    let name = if name.is_empty() {
        DEFAULT_FLAVOR.to_string()
    } else {
        name.to_ascii_lowercase()
    };
    let accelerator = *ACCELERATORS
        .iter()
        .find(|a| a.id == name)
        .ok_or_else(|| anyhow!("Unknown Kaggle flavor '{raw}'. {}", flavor_help()))?;
    let account = match account {
        None => AccountChoice::Active,
        Some(a) if a.eq_ignore_ascii_case("auto") => AccountChoice::Auto,
        Some(a) if valid_username(a) => AccountChoice::Named(a.to_string()),
        Some(_) => return Err(anyhow!("Unknown Kaggle flavor '{raw}'. {}", flavor_help())),
    };
    Ok(Flavor {
        accelerator,
        account,
    })
}

fn flavor_help() -> String {
    let ids: Vec<_> = ACCELERATORS.iter().map(|a| a.id).collect();
    format!(
        "Use one of {}, optionally with `@<account>` or `@auto`.",
        ids.join(", ")
    )
}

/// Kaggle usernames are letters, digits, `-` and `_`; anything else could
/// escape the account folder.
pub fn valid_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// --- CLI ---------------------------------------------------------------------

/// The `kaggle` executable: PATH first, then pip/uv/pipx's `~/.local/bin`.
pub fn find_cli() -> Option<PathBuf> {
    crate::local::shell_env::find_on_path("kaggle").or_else(|| {
        dirs::home_dir().and_then(|home| {
            crate::local::shell_env::find_in_dir(&home.join(".local/bin"), "kaggle")
        })
    })
}

pub const INSTALL_HINT: &str = "The Kaggle CLI is not installed. Install it with `uv tool install kaggle` (or `pipx install kaggle`, or `pip install --user kaggle`), then add an account.";
pub const ADD_ACCOUNT_HINT: &str = "Add a Kaggle account: create an API token at https://www.kaggle.com/settings/api, then run `orx compute connect kaggle` or paste it in Settings → Compute → Kaggle.";
/// The CLI that follows kernel logs live and reads `machine_shape`.
const MIN_MAJOR_VERSION: u32 = 2;

/// Install commands the settings card offers, most recommended first.
pub const INSTALL_COMMANDS: &[&str] = &[
    "uv tool install kaggle",
    "pipx install kaggle",
    "pip install --user kaggle",
];

/// Credentials the CLI would read from its own `~/.kaggle`, which an account
/// can be imported from.
pub const CREDENTIAL_FILES: &[(&str, &str)] = &[
    ("access_token", "token"),
    ("access_token.txt", "token"),
    ("kaggle.json", "key"),
    ("credentials.json", "oauth"),
];

async fn cli_version(path: &Path) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(path)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// `Kaggle CLI 2.2.4` / `Kaggle API 1.6.17` → its major version.
fn major_version(version: &str) -> Option<u32> {
    version
        .split_whitespace()
        .last()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

// --- accounts ------------------------------------------------------------------

fn root() -> PathBuf {
    crate::store::data_dir().join("kaggle")
}

fn accounts_file() -> PathBuf {
    root().join("accounts.json")
}

/// The folder the CLI uses as its home for one account.
pub fn account_home(username: &str) -> PathBuf {
    root().join("accounts").join(username)
}

fn config_dir(home: &Path) -> PathBuf {
    home.join(".kaggle")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsFile {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub accounts: Vec<StoredAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAccount {
    pub username: String,
    /// `token` (API token), `key` (legacy kaggle.json), or `oauth`.
    pub method: String,
    pub added_at: i64,
}

pub fn load_accounts() -> AccountsFile {
    let mut file: AccountsFile = std::fs::read(accounts_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    file.accounts
        .retain(|a| valid_username(&a.username) && config_dir(&account_home(&a.username)).is_dir());
    if file
        .active
        .as_ref()
        .is_none_or(|active| !file.accounts.iter().any(|a| &a.username == active))
    {
        file.active = file.accounts.first().map(|a| a.username.clone());
    }
    file
}

fn save_accounts(file: &AccountsFile) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(file)?;
    std::fs::create_dir_all(root())?;
    crate::local::git::atomic_write_with_mode(&accounts_file(), &bytes, Some(0o600))?;
    Ok(())
}

/// The account a run on `choice` uses.
pub async fn resolve_account(choice: &AccountChoice, kind: &str) -> Result<String> {
    let file = load_accounts();
    if file.accounts.is_empty() {
        return Err(anyhow!("{ADD_ACCOUNT_HINT}"));
    }
    match choice {
        AccountChoice::Active => file.active.ok_or_else(|| anyhow!("{ADD_ACCOUNT_HINT}")),
        AccountChoice::Named(name) => file
            .accounts
            .iter()
            .find(|a| a.username.eq_ignore_ascii_case(name))
            .map(|a| a.username.clone())
            .ok_or_else(|| {
                anyhow!(
                    "No saved Kaggle account '{name}'. Saved accounts: {}.",
                    file.accounts
                        .iter()
                        .map(|a| a.username.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
        AccountChoice::Auto => {
            let statuses = accounts(false).await;
            pick_auto(&statuses, kind)
                .or(file.active)
                .ok_or_else(|| anyhow!("{ADD_ACCOUNT_HINT}"))
        }
    }
}

/// The account with the most hours left for `kind` (`gpu`/`tpu`); any
/// account for CPU runs, which use no quota.
fn pick_auto(statuses: &[AccountStatus], kind: &str) -> Option<String> {
    let remaining = |status: &AccountStatus| -> f64 {
        let quota = status.quota.as_ref();
        match kind {
            "gpu" => quota.and_then(|q| q.gpu.as_ref()),
            "tpu" => quota.and_then(|q| q.tpu.as_ref()),
            _ => return f64::INFINITY,
        }
        .map_or(-1.0, |line| line.remaining_hours)
    };
    statuses
        .iter()
        .filter(|s| s.error.is_none())
        .max_by(|a, b| remaining(a).total_cmp(&remaining(b)))
        .map(|s| s.username.clone())
}

/// A `kaggle` command that sees only `username`'s credentials.
pub fn command(cli: &Path, home: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(cli);
    for (key, value) in account_env(home) {
        command.env(key, value);
    }
    for key in SCRUBBED_ENV {
        command.env_remove(key);
    }
    command
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    command
}

/// Environment the CLI reads credentials from; removed so the account folder
/// is the only source.
const SCRUBBED_ENV: &[&str] = &["KAGGLE_API_TOKEN", "KAGGLE_USERNAME", "KAGGLE_KEY"];

/// `HOME`/`KAGGLE_CONFIG_DIR` point at the account folder. A `pip install
/// --user` CLI finds its packages through `HOME`, so the real user base stays.
fn account_env(home: &Path) -> Vec<(&'static str, String)> {
    let home_str = home.to_string_lossy().into_owned();
    let mut env = vec![
        ("HOME", home_str.clone()),
        ("USERPROFILE", home_str),
        (
            "KAGGLE_CONFIG_DIR",
            config_dir(home).to_string_lossy().into_owned(),
        ),
    ];
    if std::env::var_os("PYTHONUSERBASE").is_none() {
        if let Some(real) = dirs::home_dir() {
            env.push((
                "PYTHONUSERBASE",
                real.join(".local").to_string_lossy().into_owned(),
            ));
        }
    }
    env
}

const CLI_TIMEOUT: Duration = Duration::from_secs(45);

async fn run_cli(cli: &Path, home: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = tokio::time::timeout(CLI_TIMEOUT, command(cli, home).args(args).output())
        .await
        .map_err(|_| "The Kaggle CLI did not answer in time.".to_string())?
        .map_err(|e| format!("Could not run the Kaggle CLI: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(cli_error(&stderr, &stdout))
}

/// The CLI's own last error line, or a sign-in hint for an auth failure.
fn cli_error(stderr: &str, stdout: &str) -> String {
    let text = format!("{stderr}\n{stdout}");
    let lower = text.to_ascii_lowercase();
    if lower.contains("authentication required")
        || lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("invalid credentials")
    {
        return "Kaggle did not accept these credentials. Check the token (or create a new one at https://www.kaggle.com/settings/api) and add the account again.".into();
    }
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect())
        .unwrap_or_else(|| "The Kaggle CLI failed without a message.".into())
}

/// A new account's credentials, as the user pasted them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    /// Required with a legacy key; read from the CLI for an API token.
    #[serde(default)]
    pub username: Option<String>,
    /// An API token from kaggle.com/settings/api.
    #[serde(default)]
    pub token: Option<String>,
    /// A legacy API key (the `key` in kaggle.json).
    #[serde(default)]
    pub key: Option<String>,
    /// A whole kaggle.json, `{"username": ..., "key": ...}`.
    #[serde(default)]
    pub kaggle_json: Option<String>,
}

fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    crate::local::git::atomic_write_with_mode(path, contents, Some(0o600))?;
    Ok(())
}

fn create_private_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;
    Ok(())
}

fn pending_home() -> PathBuf {
    let nonce: String = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    root().join("accounts").join(format!(".pending-{nonce}"))
}

/// Validate pasted credentials with the CLI and save them as an account.
pub async fn add_account(new: NewAccount) -> Result<AccountStatus> {
    let cli = find_cli().ok_or_else(|| anyhow!("{INSTALL_HINT}"))?;
    let home = pending_home();
    let dir = config_dir(&home);
    let username = new
        .username
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty());
    if let Some(name) = username {
        if !valid_username(name) {
            return Err(anyhow!("'{name}' is not a Kaggle username."));
        }
    }
    let (method, username) = if let Some(json_text) =
        new.kaggle_json.as_deref().filter(|t| !t.trim().is_empty())
    {
        let parsed: Value = serde_json::from_str(json_text.trim()).map_err(|_| {
            anyhow!("That is not a kaggle.json file: it needs \"username\" and \"key\".")
        })?;
        let (Some(user), Some(key)) = (parsed["username"].as_str(), parsed["key"].as_str()) else {
            return Err(anyhow!(
                "That is not a kaggle.json file: it needs \"username\" and \"key\"."
            ));
        };
        if !valid_username(user) {
            return Err(anyhow!("'{user}' is not a Kaggle username."));
        }
        write_private(
            &dir.join("kaggle.json"),
            &serde_json::to_vec(&json!({"username": user, "key": key.trim()}))?,
        )?;
        ("key", Some(user.to_string()))
    } else if let Some(token) = new
        .token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        write_private(&dir.join("access_token"), token.as_bytes())?;
        ("token", username.map(str::to_string))
    } else if let Some(key) = new.key.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let user =
            username.ok_or_else(|| anyhow!("A legacy API key needs the Kaggle username too."))?;
        write_private(
            &dir.join("kaggle.json"),
            &serde_json::to_vec(&json!({"username": user, "key": key}))?,
        )?;
        ("key", Some(user.to_string()))
    } else {
        return Err(anyhow!(
            "Paste an API token, a kaggle.json, or a username and legacy key."
        ));
    };
    let result = finish_pending(&cli, &home, method, username).await;
    let _ = std::fs::remove_dir_all(&home);
    result
}

/// Copy the credentials the CLI already uses from `~/.kaggle` into an account.
pub async fn import_cli_account() -> Result<AccountStatus> {
    let cli = find_cli().ok_or_else(|| anyhow!("{INSTALL_HINT}"))?;
    let (source, method) = cli_credentials()
        .ok_or_else(|| anyhow!("The Kaggle CLI has no saved sign-in in ~/.kaggle to import."))?;
    let home = pending_home();
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().replace(".txt", ""))
        .unwrap_or_default();
    let bytes = std::fs::read(&source)?;
    write_private(&config_dir(&home).join(name), &bytes)?;
    let result = finish_pending(&cli, &home, method, None).await;
    let _ = std::fs::remove_dir_all(&home);
    result
}

/// The CLI's own saved sign-in, if any (never its contents).
pub fn cli_credentials() -> Option<(PathBuf, &'static str)> {
    let real = dirs::home_dir()?;
    let mut dirs = vec![real.join(".kaggle")];
    if let Some(dir) = std::env::var_os("KAGGLE_CONFIG_DIR") {
        dirs.insert(0, PathBuf::from(dir));
    }
    dirs.push(real.join(".config/kaggle"));
    dirs.iter().find_map(|dir| {
        CREDENTIAL_FILES.iter().find_map(|(name, method)| {
            let path = dir.join(name);
            path.is_file().then_some((path, *method))
        })
    })
}

/// Ask the CLI who the credentials belong to, then move the pending folder
/// into place under that username.
async fn finish_pending(
    cli: &Path,
    home: &Path,
    method: &str,
    username: Option<String>,
) -> Result<AccountStatus> {
    let config = run_cli(cli, home, &["config", "view"])
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let reported = config_username(&config);
    let username = match (reported, username) {
        (Some(reported), Some(given)) if !reported.eq_ignore_ascii_case(&given) => {
            return Err(anyhow!(
                "These credentials belong to Kaggle user '{reported}', not '{given}'."
            ));
        }
        (Some(reported), _) => reported,
        (None, Some(given)) => given,
        (None, None) => {
            return Err(anyhow!(
                "The Kaggle CLI could not tell whose credentials these are. Enter the Kaggle username too."
            ))
        }
    };
    if !valid_username(&username) {
        return Err(anyhow!("'{username}' is not a Kaggle username."));
    }
    // Credentials are only accepted once Kaggle has answered with them.
    let quota = read_quota(cli, home).await.map_err(|e| anyhow!("{e}"))?;

    let target = account_home(&username);
    let _ = std::fs::remove_dir_all(&target);
    if let Some(parent) = target.parent() {
        create_private_dir(parent)?;
    }
    std::fs::rename(home, &target)
        .map_err(|e| anyhow!("Could not save the Kaggle account: {e}"))?;

    let mut file = load_accounts();
    file.accounts.retain(|a| a.username != username);
    file.accounts.push(StoredAccount {
        username: username.clone(),
        method: method.to_string(),
        added_at: crate::store::now_ms(),
    });
    if file.active.is_none() {
        file.active = Some(username.clone());
    }
    save_accounts(&file)?;
    invalidate().await;
    Ok(AccountStatus {
        active: file.active.as_deref() == Some(username.as_str()),
        username,
        method: method.to_string(),
        quota,
        error: None,
    })
}

/// `- username: alice` from `kaggle config view`.
fn config_username(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("- username:")?.trim();
        (valid_username(value) && value != "None").then(|| value.to_string())
    })
}

pub async fn remove_account(username: &str) -> Result<()> {
    let mut file = load_accounts();
    if !file.accounts.iter().any(|a| a.username == username) {
        return Err(anyhow!("No saved Kaggle account '{username}'."));
    }
    file.accounts.retain(|a| a.username != username);
    if file.active.as_deref() == Some(username) {
        file.active = file.accounts.first().map(|a| a.username.clone());
    }
    save_accounts(&file)?;
    let _ = std::fs::remove_dir_all(account_home(username));
    invalidate().await;
    Ok(())
}

pub async fn set_active(username: &str) -> Result<()> {
    let mut file = load_accounts();
    let found = file
        .accounts
        .iter()
        .find(|a| a.username.eq_ignore_ascii_case(username))
        .map(|a| a.username.clone())
        .ok_or_else(|| anyhow!("No saved Kaggle account '{username}'."))?;
    file.active = Some(found);
    save_accounts(&file)?;
    invalidate().await;
    Ok(())
}

/// Sign in through the CLI's browser flow into a fresh account folder.
/// Interactive: the CLI prints a URL and reads the code from the terminal.
pub async fn login_interactive() -> Result<AccountStatus> {
    let cli = find_cli().ok_or_else(|| anyhow!("{INSTALL_HINT}"))?;
    let home = pending_home();
    create_private_dir(&config_dir(&home))?;
    let mut login = command(&cli, &home);
    login
        .args(["auth", "login", "--no-launch-browser"])
        .stdin(std::process::Stdio::inherit());
    let status = login.status().await?;
    let result = if status.success() && config_dir(&home).join("credentials.json").is_file() {
        finish_pending(&cli, &home, "oauth", None).await
    } else {
        Err(anyhow!("The Kaggle sign-in did not finish ({status})."))
    };
    let _ = std::fs::remove_dir_all(&home);
    result
}

// --- quota and status ------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub gpu: Option<QuotaLine>,
    pub tpu: Option<QuotaLine>,
    /// When the weekly quota resets, as Kaggle reports it.
    pub refresh_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaLine {
    pub used_hours: f64,
    pub remaining_hours: f64,
    pub total_hours: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub username: String,
    pub method: String,
    pub active: bool,
    /// `None` when the CLI is too old for `kaggle quota` or Kaggle did not say.
    pub quota: Option<Quota>,
    /// Why the account could not be checked.
    pub error: Option<String>,
}

async fn read_quota(cli: &Path, home: &Path) -> std::result::Result<Option<Quota>, String> {
    match run_cli(cli, home, &["quota", "--format", "json"]).await {
        Ok(text) => Ok(parse_quota(&text)),
        // An older CLI has no `quota`; prove the credentials another way.
        Err(error) if error.contains("invalid choice") => run_cli(
            cli,
            home,
            &["kernels", "list", "--mine", "--page-size", "1"],
        )
        .await
        .map(|_| None),
        Err(error) => Err(error),
    }
}

fn hours(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_end_matches('h').trim().parse().ok(),
        _ => None,
    }
}

fn parse_quota(text: &str) -> Option<Quota> {
    let start = text.find('[')?;
    let rows: Vec<Value> = serde_json::from_str(text[start..].trim()).ok()?;
    let mut quota = Quota {
        gpu: None,
        tpu: None,
        refresh_at: None,
    };
    for row in rows {
        let line = (|| {
            let used = hours(&row["used"])?;
            let total = hours(&row["total"])?;
            let remaining = hours(&row["remaining"]).unwrap_or((total - used).max(0.0));
            Some(QuotaLine {
                used_hours: used,
                remaining_hours: remaining,
                total_hours: total,
            })
        })();
        match row["resource"]
            .as_str()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("gpu") => quota.gpu = line,
            Some("tpu") => quota.tpu = line,
            _ => {}
        }
        if quota.refresh_at.is_none() {
            quota.refresh_at = row["refreshAt"]
                .as_str()
                .or(row["refresh_at"].as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
    }
    (quota.gpu.is_some() || quota.tpu.is_some()).then_some(quota)
}

/// The settings page re-reads accounts on every open; quota moves slowly.
const ACCOUNTS_TTL: Duration = Duration::from_secs(60);

type AccountsCache = Mutex<Option<(Instant, Vec<AccountStatus>)>>;

fn cache() -> &'static AccountsCache {
    static CACHE: OnceLock<AccountsCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

pub async fn invalidate() {
    *cache().lock().await = None;
}

/// Every saved account with its weekly quota, checked in parallel.
pub async fn accounts(force: bool) -> Vec<AccountStatus> {
    let file = load_accounts();
    let Some(cli) = find_cli() else {
        return file
            .accounts
            .iter()
            .map(|a| AccountStatus {
                username: a.username.clone(),
                method: a.method.clone(),
                active: file.active.as_deref() == Some(a.username.as_str()),
                quota: None,
                error: None,
            })
            .collect();
    };
    let mut cached = cache().lock().await;
    if !force {
        if let Some((at, statuses)) = cached.as_ref() {
            if at.elapsed() < ACCOUNTS_TTL {
                return statuses
                    .iter()
                    .cloned()
                    .map(|mut s| {
                        s.active = file.active.as_deref() == Some(s.username.as_str());
                        s
                    })
                    .collect();
            }
        }
    }
    let checks = file.accounts.iter().map(|account| {
        let cli = cli.clone();
        let active = file.active.as_deref() == Some(account.username.as_str());
        async move {
            let home = account_home(&account.username);
            let (quota, error) = match read_quota(&cli, &home).await {
                Ok(quota) => (quota, None),
                Err(error) => (None, Some(error)),
            };
            AccountStatus {
                username: account.username.clone(),
                method: account.method.clone(),
                active,
                quota,
                error,
            }
        }
    });
    let statuses = futures::future::join_all(checks).await;
    *cached = Some((Instant::now(), statuses.clone()));
    statuses
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub supported_os: bool,
    pub cli_path: Option<String>,
    pub cli_version: Option<String>,
    /// The CLI predates live logs and accelerator choice; runs still work but
    /// stream their log only at the end.
    pub cli_outdated: bool,
    pub install_commands: &'static [&'static str],
    pub active: Option<String>,
    pub account_count: usize,
    /// The Kaggle CLI has its own sign-in in `~/.kaggle` that can be imported.
    pub importable: bool,
    pub ready: bool,
    pub error: Option<String>,
    pub default_flavor: &'static str,
    pub accelerators: &'static [Accelerator],
}

/// Readiness without touching the network.
pub async fn status() -> Status {
    let supported_os = cfg!(unix);
    let cli = find_cli();
    let cli_version = match &cli {
        Some(path) => cli_version(path).await,
        None => None,
    };
    let cli_outdated = cli_version
        .as_deref()
        .and_then(major_version)
        .is_some_and(|major| major < MIN_MAJOR_VERSION);
    let file = load_accounts();
    let importable = cli_credentials().is_some();
    let error = if !supported_os {
        Some(
            "The Kaggle backend runs on Linux and macOS; on Windows, run orx inside WSL."
                .to_string(),
        )
    } else if cli.is_none() {
        Some(INSTALL_HINT.to_string())
    } else if file.accounts.is_empty() {
        Some(ADD_ACCOUNT_HINT.to_string())
    } else {
        None
    };
    Status {
        supported_os,
        cli_path: cli.map(|p| p.to_string_lossy().into_owned()),
        cli_version,
        cli_outdated,
        install_commands: INSTALL_COMMANDS,
        active: file.active.clone(),
        account_count: file.accounts.len(),
        importable,
        ready: error.is_none(),
        error,
        default_flavor: DEFAULT_FLAVOR,
        accelerators: ACCELERATORS,
    }
}

/// Configured: the CLI is installed and at least one account is saved.
pub fn configured() -> bool {
    find_cli().is_some() && !load_accounts().accounts.is_empty()
}

/// The executable when Kaggle is usable, else the reason it isn't.
pub async fn preflight() -> Result<PathBuf> {
    let status = status().await;
    match (status.error, status.cli_path) {
        (None, Some(path)) => Ok(PathBuf::from(path)),
        (Some(error), _) => Err(anyhow!("{error}")),
        (None, None) => Err(anyhow!("{INSTALL_HINT}")),
    }
}

// --- run controller ------------------------------------------------------------------

/// Kernel and dataset slugs for a run; short, lowercase, and unique per run.
pub fn run_slug(run_id: &str) -> String {
    let id: String = run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("orx-run-{id}")
}

fn dataset_slug(run_id: &str) -> String {
    run_slug(run_id).replacen("orx-run-", "orx-src-", 1)
}

pub const CONTROLLER_FILE: &str = "kaggle-controller.sh";
pub const KERNEL_DIR: &str = "kaggle-kernel";
pub const DATASET_DIR: &str = "kaggle-dataset";
pub const OUTPUT_DIR: &str = "kaggle-output";
const DRIVER_FILE: &str = "orx_driver.py";
const ARCHIVE_FILE: &str = "source.orxsrc";
const EXIT_SENTINEL: &str = "__ORX_EXIT_CODE__=";

pub struct RunSpec<'a> {
    pub cli: &'a Path,
    pub username: &'a str,
    pub run_id: &'a str,
    pub flavor: &'a Flavor,
    pub archive: &'a Path,
    pub run_command: &'a str,
    pub timeout_secs: u64,
    /// Synced secrets the run does not get: a kernel's source is stored on
    /// Kaggle, so orx never writes secrets into it. Names only.
    pub skipped_env: Vec<String>,
}

/// Write the controller, kernel folder, and dataset folder into `dir`.
pub fn write_run_files(dir: &Path, spec: &RunSpec<'_>) -> Result<()> {
    let kernel_dir = dir.join(KERNEL_DIR);
    let dataset_dir = dir.join(DATASET_DIR);
    std::fs::create_dir_all(&kernel_dir)?;
    std::fs::create_dir_all(&dataset_dir)?;
    let dataset = dataset_slug(spec.run_id);
    std::fs::write(
        dataset_dir.join("dataset-metadata.json"),
        serde_json::to_vec_pretty(&json!({
            "title": dataset,
            "id": format!("{}/{dataset}", spec.username),
            "licenses": [{"name": "CC0-1.0"}],
        }))?,
    )?;
    // Unknown extension: Kaggle unpacks .zip/.tar uploads, which would lose
    // the snapshot's own file modes.
    let archive_copy = dataset_dir.join(ARCHIVE_FILE);
    if std::fs::hard_link(spec.archive, &archive_copy).is_err() {
        std::fs::copy(spec.archive, &archive_copy)
            .map_err(|e| anyhow!("Could not stage the source snapshot: {e}"))?;
    }
    std::fs::write(
        kernel_dir.join("kernel-metadata.json"),
        serde_json::to_vec_pretty(&kernel_metadata(spec))?,
    )?;
    std::fs::write(
        kernel_dir.join(DRIVER_FILE),
        driver_source(spec.run_command, spec.timeout_secs),
    )?;
    std::fs::write(dir.join(CONTROLLER_FILE), controller_script(spec))?;
    Ok(())
}

fn kernel_metadata(spec: &RunSpec<'_>) -> Value {
    let slug = run_slug(spec.run_id);
    let a = &spec.flavor.accelerator;
    let mut meta = json!({
        "id": format!("{}/{slug}", spec.username),
        "title": slug,
        "code_file": DRIVER_FILE,
        "language": "python",
        "kernel_type": "script",
        "is_private": true,
        "enable_gpu": a.kind == "gpu",
        "enable_tpu": a.kind == "tpu",
        "enable_internet": true,
        "dataset_sources": [format!("{}/{}", spec.username, dataset_slug(spec.run_id))],
        "competition_sources": [],
        "kernel_sources": [],
        "model_sources": [],
    });
    if !a.machine_shape.is_empty() {
        meta["machine_shape"] = json!(a.machine_shape);
    }
    meta
}

/// The bash controller that drives one run (see the module docs).
pub fn controller_script(spec: &RunSpec<'_>) -> String {
    use crate::jobs::ssh::sh_quote;
    let home = account_home(spec.username);
    let env = account_env(&home)
        .into_iter()
        .map(|(key, value)| format!("export {key}={}", sh_quote(&value)))
        .collect::<Vec<_>>()
        .join("\n");
    let slug = run_slug(spec.run_id);
    format!(
        r#"#!/usr/bin/env bash
# Generated by orx: runs one experiment as a private Kaggle kernel.
set -u
shopt -s lastpipe
kaggle={cli}
{env}
unset {scrubbed}
kernel={owner}/{slug}
dataset={owner}/{dataset}
url="https://www.kaggle.com/code/$kernel"
pushed=0
finished=0
cleanup() {{
  if [ "$pushed" = 1 ] && [ "$finished" = 0 ]; then
    echo "[orx] Stopped following the Kaggle kernel. It keeps running until it ends or reaches its {timeout}s limit; stop it sooner at $url"
  fi
  "$kaggle" datasets delete "$dataset" -y </dev/null >/dev/null 2>&1 \
    || echo "[orx] Could not delete the source dataset; remove it at https://www.kaggle.com/datasets/$dataset"
}}
trap cleanup EXIT
trap 'exit 143' TERM INT HUP

# "user/slug has status "KernelWorkerStatus.RUNNING"" -> running
kernel_state() {{
  "$kaggle" kernels status "$kernel" </dev/null 2>/dev/null \
    | sed -n 's/.*has status "\([^"]*\)".*/\1/p' | head -n 1 \
    | sed 's/.*\.//' | tr '[:upper:]' '[:lower:]'
}}

echo "[orx] Kaggle account: {owner}"
{skipped}echo "[orx] Uploading the source snapshot as a private dataset"
if ! "$kaggle" datasets create -p {dataset_dir} -q </dev/null; then
  echo "[orx] Kaggle refused the source dataset. Check the account's credentials with: orx compute test kaggle"
  exit 1
fi
ready=0
for _ in $(seq 1 180); do
  state=$("$kaggle" datasets status "$dataset" </dev/null 2>/dev/null | tr '[:upper:]' '[:lower:]')
  case "$state" in
    *ready*) ready=1; break ;;
    *error*) break ;;
  esac
  sleep 5
done
if [ "$ready" != 1 ]; then
  echo "[orx] Kaggle did not finish processing the source dataset."
  exit 1
fi

echo "[orx] Starting a Kaggle kernel: {label}"
if ! "$kaggle" kernels push -p {kernel_dir} -t {timeout} </dev/null; then
  echo "[orx] Kaggle refused the kernel. GPUs and internet access need a phone-verified Kaggle account, and the weekly GPU/TPU quota may be used up (orx compute show kaggle)."
  exit 1
fi
pushed=1
echo "[orx] Kernel: $url"

state=""
misses=0
while :; do
  current=$(kernel_state)
  case "$current" in
    running|complete|error|cancel_requested|cancel_acknowledged) state="$current"; break ;;
    queued|new_script)
      [ "$state" = queued ] || echo "[orx] Waiting in Kaggle's queue"
      state=queued; misses=0 ;;
    *)
      misses=$((misses + 1))
      if [ "$misses" -ge 60 ]; then
        echo "[orx] Lost contact with Kaggle while waiting for the kernel to start."
        exit 1
      fi ;;
  esac
  sleep 10
done

code=""
if [ "$state" = running ]; then
  "$kaggle" kernels logs "$kernel" --follow </dev/null 2>/dev/null | while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      *{sentinel}*) code="${{line##*{sentinel}}}"; code="${{code%%[!0-9]*}}" ;;
      *) printf '%s\n' "$line" ;;
    esac
  done
fi

misses=0
while :; do
  state=$(kernel_state)
  case "$state" in
    complete|error|cancel_acknowledged) break ;;
    "")
      misses=$((misses + 1))
      if [ "$misses" -ge 60 ]; then
        echo "[orx] Lost contact with Kaggle while the kernel was running."
        exit 1
      fi ;;
    *) misses=0 ;;
  esac
  sleep 15
done

echo "[orx] Downloading the kernel's output"
mkdir -p {output_dir}
"$kaggle" kernels output "$kernel" -p {output_dir} -q </dev/null >/dev/null 2>&1 \
  || echo "[orx] Could not download the kernel's output; it stays at $url"
finished=1
if [ -z "$code" ]; then
  # No live log (an older CLI, or a dropped stream): read the saved one.
  code=$(cat {output_dir}/*.log 2>/dev/null | grep -o '{sentinel}[0-9]*' | tail -n 1 | sed 's/.*=//')
  if [ -n "$code" ]; then
    echo "[orx] The full kernel log is in {output_dir}/{slug}.log"
  fi
fi
if [ -z "$code" ]; then
  echo "[orx] The Kaggle kernel ended ($state) without reporting the run's exit status. See $url"
  exit 1
fi
exit "$code"
"#,
        cli = sh_quote(&spec.cli.to_string_lossy()),
        env = env,
        scrubbed = SCRUBBED_ENV.join(" "),
        owner = spec.username,
        slug = slug,
        dataset = dataset_slug(spec.run_id),
        timeout = spec.timeout_secs,
        label = spec.flavor.label().replace(['"', '$', '`', '\\'], ""),
        dataset_dir = sh_quote(DATASET_DIR),
        kernel_dir = sh_quote(KERNEL_DIR),
        output_dir = OUTPUT_DIR,
        sentinel = EXIT_SENTINEL,
        skipped = skipped_note(&spec.skipped_env),
    )
}

fn skipped_note(keys: &[String]) -> String {
    let keys: Vec<&str> = keys
        .iter()
        .map(String::as_str)
        .filter(|key| key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    if keys.is_empty() {
        return String::new();
    }
    format!(
        "echo \"[orx] Not sent to Kaggle (kernel code is stored there): {}. Runs that need these secrets belong on another backend.\"\n",
        keys.join(", ")
    )
}

/// The kernel's script: finds the mounted snapshot, runs the fixed command,
/// and prints the exit sentinel. The kernel itself always succeeds so its
/// output is kept; the run's status rides on the sentinel.
pub fn driver_source(run_command: &str, timeout_secs: u64) -> String {
    use base64::Engine as _;
    let script = crate::compute::staged_script(run_command);
    let script_b64 = base64::engine::general_purpose::STANDARD.encode(script);
    DRIVER_TEMPLATE
        .replace("__ORX_SCRIPT_B64__", &script_b64)
        .replace("__ORX_SENTINEL__", EXIT_SENTINEL)
        .replace("__ORX_ARCHIVE__", ARCHIVE_FILE)
        .replace("__ORX_TIMEOUT_SECS__", &timeout_secs.to_string())
}

const DRIVER_TEMPLATE: &str = r#"# Generated by orx: runs one experiment snapshot on Kaggle.
import base64 as _orx_b64
import os as _orx_os
import subprocess as _orx_sp
import sys as _orx_sys

ORX_TIMEOUT_SECS = __ORX_TIMEOUT_SECS__


def _orx_find_archive():
    for base, _dirs, files in _orx_os.walk("/kaggle/input"):
        if "__ORX_ARCHIVE__" in files:
            return _orx_os.path.join(base, "__ORX_ARCHIVE__")
    return None


def _orx_main():
    archive = _orx_find_archive()
    if archive is None:
        print("[orx] The source snapshot dataset is not mounted in this kernel.", flush=True)
        return 97
    try:
        gpus = _orx_sp.run(
            ["nvidia-smi", "--query-gpu=name,memory.total", "--format=csv,noheader"],
            capture_output=True, text=True, timeout=30,
        ).stdout.strip()
        for gpu in gpus.splitlines():
            print("[orx] GPU: " + gpu, flush=True)
    except Exception:
        pass
    workdir = "/tmp/orx-run"
    _orx_os.makedirs(_orx_os.path.join(workdir, "repo"), exist_ok=True)
    _orx_sp.run(["tar", "-xf", archive, "-C", _orx_os.path.join(workdir, "repo")], check=True)
    env = dict(_orx_os.environ)
    env.setdefault("PYTHONUNBUFFERED", "1")
    env.setdefault("PYTHONIOENCODING", "utf-8")
    # Files written here are downloaded into the run's kaggle-output folder.
    env["ORX_OUTPUT_DIR"] = "/kaggle/working"
    script = _orx_b64.b64decode("__ORX_SCRIPT_B64__").decode("utf-8")
    proc = _orx_sp.Popen(
        ["timeout", "--signal=TERM", "--kill-after=60", str(ORX_TIMEOUT_SECS), "bash", "-c", script],
        cwd=workdir, env=env, stdout=_orx_sp.PIPE, stderr=_orx_sp.STDOUT,
        text=True, bufsize=1, errors="replace",
    )
    for line in proc.stdout:
        _orx_sys.stdout.write(line)
        _orx_sys.stdout.flush()
    code = proc.wait()
    if code == 124:
        print("[orx] The run hit its timeout of %d seconds." % ORX_TIMEOUT_SECS, flush=True)
    return code


try:
    _orx_code = _orx_main()
except Exception as _orx_error:
    print("[orx] The Kaggle driver failed: %r" % (_orx_error,), flush=True)
    _orx_code = 1
print("\n__ORX_SENTINEL__%d" % _orx_code, flush=True)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavors_parse_with_default_and_accounts() {
        let default = parse_flavor(None).unwrap();
        assert_eq!(default.id(), "t4");
        assert_eq!(default.account, AccountChoice::Active);
        assert_eq!(parse_flavor(Some("P100")).unwrap().id(), "p100");
        let named = parse_flavor(Some("tpu@alice_2")).unwrap();
        assert_eq!(named.account, AccountChoice::Named("alice_2".into()));
        assert_eq!(named.id(), "tpu@alice_2");
        assert_eq!(parse_flavor(Some("t4@AUTO")).unwrap().id(), "t4@auto");
        assert_eq!(parse_flavor(Some("@bob")).unwrap().id(), "t4@bob");
    }

    #[test]
    fn unknown_flavors_and_unsafe_accounts_are_rejected() {
        assert!(parse_flavor(Some("a100")).is_err());
        assert!(parse_flavor(Some("t4@../etc")).is_err());
        assert!(parse_flavor(Some("t4@")).is_err());
        assert!(parse_flavor(Some("t4@a b")).is_err());
    }

    #[test]
    fn quota_parses_from_cli_json() {
        let text = r#"[
  {"resource": "GPU", "used": "4.50h", "remaining": "25.50h", "total": "30.00h", "refreshAt": "2026-10-18T00:00:00"},
  {"resource": "TPU", "used": "0.00h", "remaining": "20.00h", "total": "20.00h", "refreshAt": "2026-10-18T00:00:00"}
]"#;
        let quota = parse_quota(text).unwrap();
        let gpu = quota.gpu.unwrap();
        assert_eq!(
            (gpu.used_hours, gpu.remaining_hours, gpu.total_hours),
            (4.5, 25.5, 30.0)
        );
        assert_eq!(quota.tpu.unwrap().remaining_hours, 20.0);
        assert_eq!(quota.refresh_at.as_deref(), Some("2026-10-18T00:00:00"));
        assert!(parse_quota("No quota information available").is_none());
    }

    fn status(name: &str, gpu_left: Option<f64>, error: Option<&str>) -> AccountStatus {
        AccountStatus {
            username: name.into(),
            method: "token".into(),
            active: false,
            quota: gpu_left.map(|left| Quota {
                gpu: Some(QuotaLine {
                    used_hours: 30.0 - left,
                    remaining_hours: left,
                    total_hours: 30.0,
                }),
                tpu: None,
                refresh_at: None,
            }),
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn auto_picks_the_account_with_the_most_quota_left() {
        let statuses = [
            status("a", Some(3.0), None),
            status("b", Some(21.5), None),
            status("c", Some(29.0), Some("bad token")),
            status("d", None, None),
        ];
        assert_eq!(pick_auto(&statuses, "gpu").as_deref(), Some("b"));
        assert!(pick_auto(&[], "gpu").is_none());
    }

    #[test]
    fn usernames_come_from_config_view() {
        let text = "Configuration values from /x/.kaggle\n- username: alice\n- auth_method: ACCESS_TOKEN\n";
        assert_eq!(config_username(text).as_deref(), Some("alice"));
        assert!(config_username("- username: None\n").is_none());
    }

    #[test]
    fn run_slugs_are_short_lowercase_and_distinct() {
        assert_eq!(run_slug("3F2A-91bc-77de-4410"), "orx-run-3f2a91bc77de");
        assert_eq!(dataset_slug("3F2A-91bc-77de-4410"), "orx-src-3f2a91bc77de");
    }

    fn sample_spec<'a>(flavor: &'a Flavor) -> RunSpec<'a> {
        RunSpec {
            cli: Path::new("/home/me/.local/bin/kaggle"),
            username: "alice",
            run_id: "0123456789abcdef",
            flavor,
            archive: Path::new("/data/source-snapshots/0123.tar"),
            run_command: "python train.py",
            timeout_secs: 3600,
            skipped_env: vec!["HF_TOKEN".into(), "BAD;KEY".into()],
        }
    }

    #[test]
    fn controller_uploads_pushes_follows_and_always_cleans_up() {
        let flavor = parse_flavor(Some("t4")).unwrap();
        let script = controller_script(&sample_spec(&flavor));
        let create = script.find("datasets create").unwrap();
        let push = script.find("kernels push").unwrap();
        let follow = script.find("kernels logs \"$kernel\" --follow").unwrap();
        let output = script.find("kernels output").unwrap();
        assert!(
            create < push && push < follow && follow < output,
            "{script}"
        );
        assert!(script.contains("trap cleanup EXIT"), "{script}");
        assert!(
            script.contains("datasets delete \"$dataset\" -y"),
            "{script}"
        );
        assert!(
            script.contains("kernel=alice/orx-run-0123456789ab"),
            "{script}"
        );
        assert!(script.contains("-t 3600"), "{script}");
        assert!(script.contains("unset KAGGLE_API_TOKEN KAGGLE_USERNAME KAGGLE_KEY"));
        assert!(script.contains("export KAGGLE_CONFIG_DIR="), "{script}");
        assert!(script.contains("Not sent to Kaggle (kernel code is stored there): HF_TOKEN."));
        assert!(!script.contains("BAD;KEY"), "{script}");
    }

    #[test]
    fn kernel_metadata_requests_the_accelerator_and_mounts_the_snapshot() {
        let flavor = parse_flavor(Some("p100")).unwrap();
        let meta = kernel_metadata(&sample_spec(&flavor));
        assert_eq!(meta["id"], "alice/orx-run-0123456789ab");
        assert_eq!(meta["title"], "orx-run-0123456789ab");
        assert_eq!(meta["machine_shape"], "NvidiaTeslaP100");
        assert_eq!(meta["enable_gpu"], true);
        assert_eq!(meta["is_private"], true);
        assert_eq!(meta["dataset_sources"][0], "alice/orx-src-0123456789ab");
        let cpu = parse_flavor(Some("cpu")).unwrap();
        let meta = kernel_metadata(&sample_spec(&cpu));
        assert!(meta.get("machine_shape").is_none());
        assert_eq!(meta["enable_gpu"], false);
    }

    #[test]
    fn driver_embeds_the_staged_command_and_reports_its_exit() {
        use base64::Engine as _;
        let driver = driver_source("python train.py --lr 3e-4", 7200);
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(crate::compute::staged_script("python train.py --lr 3e-4"));
        assert!(driver.contains(&encoded));
        assert!(driver.contains("ORX_TIMEOUT_SECS = 7200"));
        assert!(
            driver.contains("print(\"\\n__ORX_EXIT_CODE__=%d\""),
            "{driver}"
        );
        assert!(!driver.contains("__ORX_ARCHIVE__"));
    }

    #[cfg(unix)]
    #[test]
    fn controller_runs_end_to_end_against_a_stand_in_cli() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("orx-kaggle-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let cli = root.join("kaggle");
        let calls = root.join("calls.log");
        std::fs::write(
            &cli,
            r#"#!/usr/bin/env bash
echo "$*" >> "$FAKE_CALLS"
case "$1 $2" in
  "datasets status") echo ready ;;
  "kernels status")
    n=$(cat "$FAKE_CALLS.n" 2>/dev/null || echo 0); echo $((n + 1)) > "$FAKE_CALLS.n"
    if [ "$n" = 0 ]; then s=RUNNING; else s=COMPLETE; fi
    echo "alice/orx-run-0123456789ab has status \"KernelWorkerStatus.$s\"" ;;
  "kernels logs") echo "hello from kaggle"; printf '\n__ORX_EXIT_CODE__=3\n' ;;
  "kernels output") mkdir -p "$5" ;;
esac
exit 0
"#,
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let archive = root.join("snapshot.tar");
        std::fs::write(&archive, b"tar").unwrap();
        let run_dir = root.join("run");
        let flavor = parse_flavor(Some("t4")).unwrap();
        let mut spec = sample_spec(&flavor);
        spec.cli = &cli;
        spec.archive = &archive;
        write_run_files(&run_dir, &spec).unwrap();
        assert!(run_dir.join(DATASET_DIR).join(ARCHIVE_FILE).is_file());
        let output = std::process::Command::new("bash")
            .arg(CONTROLLER_FILE)
            .current_dir(&run_dir)
            .env("FAKE_CALLS", &calls)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(3), "{stdout}");
        assert!(stdout.contains("hello from kaggle"), "{stdout}");
        assert!(!stdout.contains("__ORX_EXIT_CODE__"), "{stdout}");
        let calls = std::fs::read_to_string(&calls).unwrap();
        let order = [
            "datasets create",
            "kernels push",
            "kernels logs",
            "kernels output",
            "datasets delete alice/orx-src-0123456789ab -y",
        ];
        for call in order {
            assert!(calls.contains(call), "{call} missing from:\n{calls}");
        }
        let positions: Vec<usize> = order.iter().map(|c| calls.find(c).unwrap()).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{calls}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn auth_failures_read_as_rejected_credentials() {
        let help = "Authentication required to call the Kaggle API.\n...\n    Save the token to ~/.kaggle/access_token\n";
        assert!(cli_error("", help).starts_with("Kaggle did not accept these credentials"));
        assert_eq!(
            cli_error("Traceback\nValueError: boom\n", ""),
            "ValueError: boom"
        );
    }

    #[test]
    fn versions_parse() {
        assert_eq!(major_version("Kaggle CLI 2.2.4"), Some(2));
        assert_eq!(major_version("Kaggle API 1.6.17"), Some(1));
        assert_eq!(major_version("garbage"), None);
    }
}
