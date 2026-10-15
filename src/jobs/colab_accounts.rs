//! Several Google accounts for the Colab backend.
//!
//! Every account lives in its own folder under the orx data dir, which the
//! `colab` CLI sees as its home (`HOME`), so it keeps that account's sign-in
//! (`.config/colab-cli/token.json`) and session list there. Accounts never
//! mix, and the CLI's own sign-in in `~/.config/colab-cli` is left alone; it
//! is imported once as the first account, so runs that worked before keep
//! working. Folders are readable by the owner only, and orx never prints or
//! logs a token.
//!
//! Adding an account drives the CLI's own copy-paste sign-in: the CLI prints a
//! Google URL, the user signs in with the account to add and pastes back the
//! code Google shows. The dashboard does that in two requests
//! ([`login_start`], [`login_finish`]); `orx compute connect colab` does it in
//! the terminal ([`login_interactive`]).
//!
//! A paused account is never picked by `@auto` or for runs without an
//! account; naming it explicitly still works.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use super::colab::{find_cli, Flavor, INSTALL_HINT};
use super::colab_account::Account;
use super::colab_runtimes::Runtime;
use crate::error::{anyhow, Result};

pub const ADD_ACCOUNT_HINT: &str =
    "Add a Google account for Colab in Settings → Compute → Colab, or run `orx compute connect colab`.";

/// Which saved account a run uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountChoice {
    /// The default account (or, when it is paused, the best unpaused one).
    Active,
    /// The unpaused account best placed to run the accelerator now.
    Auto,
    /// One account, by email or by the part before `@` when that is unique.
    Named(String),
}

/// Emails as Google reports them; anything else could escape the account folder.
pub fn valid_account_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 254
        && !id.starts_with('.')
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-'))
}

// --- folders -------------------------------------------------------------------

fn root() -> PathBuf {
    crate::store::data_dir().join("colab")
}

fn accounts_file() -> PathBuf {
    root().join("accounts.json")
}

/// The folder the CLI uses as its home for one account.
pub fn account_home(id: &str) -> PathBuf {
    root().join("accounts").join(id)
}

/// The CLI's sign-in file under a home folder.
pub fn token_file(home: &Path) -> PathBuf {
    home.join(".config/colab-cli/token.json")
}

/// The user's real home, where the CLI keeps its own sign-in.
pub fn cli_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// The CLI's own sign-in, if it has one.
pub fn cli_token() -> Option<PathBuf> {
    cli_home()
        .map(|home| token_file(&home))
        .filter(|p| p.is_file())
}

fn pending_home() -> PathBuf {
    let nonce: String = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    root().join("accounts").join(format!(".pending-{nonce}"))
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

// --- the accounts file ------------------------------------------------------------

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
    /// The Google account's email, lowercase.
    pub id: String,
    pub added_at: i64,
    #[serde(default)]
    pub paused: bool,
}

pub fn load_accounts() -> AccountsFile {
    let mut file: AccountsFile = std::fs::read(accounts_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    file.accounts
        .retain(|a| valid_account_id(&a.id) && token_file(&account_home(&a.id)).is_file());
    if file
        .active
        .as_ref()
        .is_none_or(|active| !file.accounts.iter().any(|a| &a.id == active))
    {
        file.active = file.accounts.first().map(|a| a.id.clone());
    }
    file
}

fn save_accounts(file: &AccountsFile) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(file)?;
    create_private_dir(&root())?;
    crate::local::git::atomic_write_with_mode(&accounts_file(), &bytes, Some(0o600))?;
    Ok(())
}

/// Saved accounts exist, or the CLI's own sign-in is waiting to be imported
/// as the first one.
pub fn configured() -> bool {
    find_cli().is_some() && (!load_accounts().accounts.is_empty() || first_import_pending())
}

/// Before any account was saved, the CLI's own sign-in becomes the first.
fn first_import_pending() -> bool {
    !accounts_file().exists() && cli_token().is_some()
}

/// Import the CLI's own sign-in once, so setups from before accounts keep
/// working. Best-effort: offline, it is retried on the next call.
pub async fn ensure_imported() {
    if first_import_pending() {
        if let Err(error) = import_cli_account().await {
            eprintln!("orx: could not import the Colab CLI's sign-in: {error}");
        }
    }
}

/// `alice@gmail.com` for `alice@gmail.com` or, when unique, `alice`.
fn find_account<'a>(file: &'a AccountsFile, name: &str) -> Option<&'a StoredAccount> {
    let name = name.trim().to_ascii_lowercase();
    if let Some(account) = file.accounts.iter().find(|a| a.id == name) {
        return Some(account);
    }
    let mut matches = file
        .accounts
        .iter()
        .filter(|a| a.id.split('@').next() == Some(name.as_str()));
    match (matches.next(), matches.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

fn unknown_account(file: &AccountsFile, name: &str) -> crate::error::Error {
    if file.accounts.is_empty() {
        return anyhow!("{ADD_ACCOUNT_HINT}");
    }
    anyhow!(
        "No saved Colab account '{name}'. Saved accounts: {}.",
        file.accounts
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The account a run with `flavor` uses.
pub async fn resolve_account(flavor: &Flavor) -> Result<String> {
    ensure_imported().await;
    let file = load_accounts();
    if file.accounts.is_empty() {
        return Err(anyhow!("{ADD_ACCOUNT_HINT}"));
    }
    match &flavor.account {
        AccountChoice::Named(name) => find_account(&file, name)
            .map(|a| a.id.clone())
            .ok_or_else(|| unknown_account(&file, name)),
        AccountChoice::Active => {
            let active = file
                .active
                .as_ref()
                .and_then(|id| file.accounts.iter().find(|a| &a.id == id));
            match active {
                Some(account) if !account.paused => Ok(account.id.clone()),
                _ => auto_account(flavor).await,
            }
        }
        AccountChoice::Auto => auto_account(flavor).await,
    }
}

async fn auto_account(flavor: &Flavor) -> Result<String> {
    let statuses = accounts(false).await;
    pick_auto(&statuses, flavor.accelerator.id).ok_or_else(|| {
        if statuses.iter().all(|s| s.paused) {
            anyhow!("Every saved Colab account is paused. Resume one in Settings → Compute → Colab, or name one with --flavor {}@<account>.", flavor.accelerator.id)
        } else {
            anyhow!(
                "No unpaused Colab account can assign {} now (checked: {}).",
                flavor.label(),
                statuses
                    .iter()
                    .filter(|s| !s.paused)
                    .map(|s| match &s.account.as_ref().and_then(|a| a.error.clone()) {
                        Some(error) => format!("{}: {error}", s.id),
                        None => format!("{}: not on its plan", s.id),
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        }
    })
}

/// Usable now: unpaused, signed in, and the plan can assign `accelerator`
/// (or Colab did not say). Prefer a confirmed plan, then the account with the
/// fewest runtimes up, then the most compute units left.
fn pick_auto(statuses: &[AccountStatus], accelerator: &str) -> Option<String> {
    statuses
        .iter()
        .filter(|s| !s.paused)
        .filter_map(|s| {
            let account = s.account.as_ref().filter(|a| a.error.is_none())?;
            let confirmed = match &account.eligible {
                Some(ids) if ids.iter().any(|id| id == accelerator) => true,
                Some(_) => return None,
                None => false,
            };
            Some((s, account, confirmed))
        })
        .max_by(|(_, a, a_ok), (_, b, b_ok)| {
            a_ok.cmp(b_ok)
                .then_with(|| {
                    b.active_runtimes
                        .unwrap_or(0)
                        .cmp(&a.active_runtimes.unwrap_or(0))
                })
                .then_with(|| {
                    a.balance
                        .unwrap_or(0.0)
                        .total_cmp(&b.balance.unwrap_or(0.0))
                })
        })
        .map(|(s, _, _)| s.id.clone())
}

// --- changing accounts --------------------------------------------------------------

pub async fn set_active(name: &str) -> Result<()> {
    let mut file = load_accounts();
    let id = find_account(&file, name)
        .map(|a| a.id.clone())
        .ok_or_else(|| unknown_account(&file, name))?;
    file.active = Some(id);
    save_accounts(&file)?;
    invalidate().await;
    Ok(())
}

pub async fn set_paused(name: &str, paused: bool) -> Result<()> {
    let mut file = load_accounts();
    let id = find_account(&file, name)
        .map(|a| a.id.clone())
        .ok_or_else(|| unknown_account(&file, name))?;
    for account in &mut file.accounts {
        if account.id == id {
            account.paused = paused;
        }
    }
    save_accounts(&file)?;
    invalidate().await;
    Ok(())
}

/// Forget an account and its sign-in. Refused while an orx run uses it, since
/// that run's controller needs the sign-in to release its runtime.
pub async fn remove_account(name: &str) -> Result<()> {
    let mut file = load_accounts();
    let id = find_account(&file, name)
        .map(|a| a.id.clone())
        .ok_or_else(|| unknown_account(&file, name))?;
    if let Some(run) = live_runs().get(&id) {
        return Err(anyhow!(
            "Run {} is still using {id} on Colab. Wait for it to end or cancel it, then remove the account.",
            run.chars().take(8).collect::<String>()
        ));
    }
    file.accounts.retain(|a| a.id != id);
    if file.active.as_deref() == Some(id.as_str()) {
        file.active = file.accounts.first().map(|a| a.id.clone());
    }
    save_accounts(&file)?;
    let _ = std::fs::remove_dir_all(account_home(&id));
    invalidate().await;
    Ok(())
}

/// Account id → a Colab run on it that has not ended.
fn live_runs() -> HashMap<String, String> {
    let Ok(runs) = crate::store::Store::open().and_then(|store| store.list_runs(1000)) else {
        return HashMap::new();
    };
    runs.into_iter()
        .filter(|run| !crate::store::is_terminal_status(&run.status))
        .filter_map(|run| {
            let backend: serde_json::Value = serde_json::from_str(&run.backend_json).ok()?;
            if backend["kind"] != "colab_job" {
                return None;
            }
            let account = backend["flavor"].as_str()?.split_once('@')?.1.to_string();
            Some((account, run.id))
        })
        .collect()
}

/// Copy the CLI's own sign-in from `~/.config/colab-cli` into an account.
pub async fn import_cli_account() -> Result<AccountStatus> {
    let source = cli_token().ok_or_else(|| {
        anyhow!("The Colab CLI has no saved sign-in in ~/.config/colab-cli to import.")
    })?;
    let home = pending_home();
    create_private_dir(token_file(&home).parent().expect("token folder"))?;
    let bytes = std::fs::read(&source)?;
    crate::local::git::atomic_write_with_mode(&token_file(&home), &bytes, Some(0o600))?;
    let result = finish_pending(&home).await;
    let _ = std::fs::remove_dir_all(&home);
    result
}

/// Ask Google whose sign-in `home` holds, then file it under that email. An
/// account that is already saved gets the new sign-in and keeps its folder,
/// so a run on it keeps its session list.
async fn finish_pending(home: &Path) -> Result<AccountStatus> {
    let id = super::colab_account::email(home)
        .await
        .map_err(|error| anyhow!("Could not check the new Colab sign-in: {error}"))?;
    super::colab_account::refresh_sign_in(home).await;
    if !valid_account_id(&id) {
        return Err(anyhow!("'{id}' is not an account orx can save."));
    }
    let target = account_home(&id);
    if token_file(&target).is_file() {
        std::fs::rename(token_file(home), token_file(&target))
            .map_err(|e| anyhow!("Could not save the Colab sign-in: {e}"))?;
    } else {
        let _ = std::fs::remove_dir_all(&target);
        if let Some(parent) = target.parent() {
            create_private_dir(parent)?;
        }
        std::fs::rename(home, &target)
            .map_err(|e| anyhow!("Could not save the Colab account: {e}"))?;
    }

    let mut file = load_accounts();
    if !file.accounts.iter().any(|a| a.id == id) {
        file.accounts.push(StoredAccount {
            id: id.clone(),
            added_at: crate::store::now_ms(),
            paused: false,
        });
    }
    if file.active.is_none() {
        file.active = Some(id.clone());
    }
    save_accounts(&file)?;
    invalidate().await;
    let paused = file.accounts.iter().any(|a| a.id == id && a.paused);
    Ok(status_of(
        &id,
        file.active.as_deref() == Some(id.as_str()),
        paused,
        true,
    )
    .await)
}

// --- sign-in -----------------------------------------------------------------------

/// The environment that makes the CLI use `home` as its home. A `pip install
/// --user` CLI finds its packages through `HOME`, so the real user base stays.
pub fn account_env(home: &Path) -> Vec<(&'static str, String)> {
    let mut env = vec![("HOME", home.to_string_lossy().into_owned())];
    if std::env::var_os("PYTHONUSERBASE").is_none() {
        if let Some(real) = cli_home() {
            env.push((
                "PYTHONUSERBASE",
                real.join(".local").to_string_lossy().into_owned(),
            ));
        }
    }
    env
}

fn command(cli: &Path, home: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(cli);
    for (key, value) in account_env(home) {
        command.env(key, value);
    }
    command.kill_on_drop(true);
    command
}

/// Any authenticated command makes the CLI sign in first; listing sessions
/// starts nothing.
const SIGN_IN_ARGS: &[&str] = &["sessions"];

/// Google's chooser instead of silently reusing the browser's current account,
/// so a second account can be picked.
fn with_account_chooser(url: &str) -> String {
    url.replacen("prompt=consent", "prompt=select_account%20consent", 1)
}

/// Sign in through the CLI in the terminal, into a fresh account folder.
pub async fn login_interactive() -> Result<AccountStatus> {
    let cli = find_cli().ok_or_else(|| anyhow!("{INSTALL_HINT}"))?;
    let home = pending_home();
    create_private_dir(&home)?;
    eprintln!("Sign in with the Google account to add. If the browser is already signed in to another account, pick or add the right one on Google's account page.");
    let status = command(&cli, &home)
        .args(SIGN_IN_ARGS)
        .stdin(std::process::Stdio::inherit())
        .status()
        .await?;
    let result = if status.success() && token_file(&home).is_file() {
        finish_pending(&home).await
    } else {
        Err(anyhow!("The Colab sign-in did not finish ({status})."))
    };
    let _ = std::fs::remove_dir_all(&home);
    result
}

struct PendingLogin {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    home: PathBuf,
    output: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    started: Instant,
}

/// A sign-in nobody finished is dropped after this long.
const LOGIN_TTL: Duration = Duration::from_secs(15 * 60);
const LOGIN_URL_WAIT: Duration = Duration::from_secs(30);
const LOGIN_FINISH_WAIT: Duration = Duration::from_secs(90);

fn logins() -> &'static Mutex<HashMap<String, PendingLogin>> {
    static LOGINS: OnceLock<Mutex<HashMap<String, PendingLogin>>> = OnceLock::new();
    LOGINS.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn drop_login(mut login: PendingLogin) {
    let _ = login.child.start_kill();
    let _ = login.child.wait().await;
    let _ = std::fs::remove_dir_all(&login.home);
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStart {
    pub id: String,
    /// Google's sign-in page; it ends by showing a code to paste back.
    pub url: String,
}

/// Start the CLI's sign-in in a fresh account folder and return Google's URL.
pub async fn login_start() -> Result<LoginStart> {
    let cli = find_cli().ok_or_else(|| anyhow!("{INSTALL_HINT}"))?;
    {
        let mut pending = logins().lock().await;
        let stale: Vec<String> = pending
            .iter()
            .filter(|(_, login)| login.started.elapsed() > LOGIN_TTL)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(login) = pending.remove(&id) {
                drop_login(login).await;
            }
        }
    }
    let home = pending_home();
    create_private_dir(&home)?;
    let mut child = match command(&cli, &home)
        .args(SIGN_IN_ARGS)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&home);
            return Err(anyhow!("Could not run the Colab CLI: {error}"));
        }
    };
    let stdin = child.stdin.take().expect("piped stdin");
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (url_tx, mut url_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // Keep draining both pipes for the life of the sign-in so the CLI never
    // blocks on a full pipe; the URL is the first Google link it prints.
    for stream in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
    ]
    .into_iter()
    .flatten()
    {
        let output = output.clone();
        let url_tx = url_tx.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(url) = auth_url(&line) {
                    let _ = url_tx.send(url);
                }
                let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
                if output.len() < 200 {
                    output.push(line);
                }
            }
        });
    }
    drop(url_tx);
    let url = match tokio::time::timeout(LOGIN_URL_WAIT, url_rx.recv()).await {
        Ok(Some(url)) => url,
        _ => {
            let message = last_line(&output)
                .unwrap_or_else(|| "The Colab CLI did not offer a sign-in link.".into());
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = std::fs::remove_dir_all(&home);
            return Err(anyhow!("{message}"));
        }
    };
    let id = uuid::Uuid::new_v4().simple().to_string();
    logins().lock().await.insert(
        id.clone(),
        PendingLogin {
            child,
            stdin,
            home,
            output,
            started: Instant::now(),
        },
    );
    Ok(LoginStart {
        id,
        url: with_account_chooser(&url),
    })
}

/// The Google sign-in link in a line the CLI printed.
fn auth_url(line: &str) -> Option<String> {
    let start = line.find("https://accounts.google.com/")?;
    let url = line[start..].split_whitespace().next()?;
    Some(url.to_string())
}

fn last_line(output: &std::sync::Mutex<Vec<String>>) -> Option<String> {
    output
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|line| line.trim())
        .rfind(|line| !line.is_empty() && !line.starts_with("Enter the authorization code"))
        .map(|line| line.chars().take(300).collect())
}

/// Hand Google's code to the waiting CLI and save the account it signed in.
pub async fn login_finish(id: &str, code: &str) -> Result<AccountStatus> {
    let code = code.trim();
    if code.is_empty() || code.contains(char::is_whitespace) {
        return Err(anyhow!("Paste the code Google showed after you signed in."));
    }
    let mut login = logins()
        .lock()
        .await
        .remove(id)
        .ok_or_else(|| anyhow!("This sign-in expired. Start again."))?;
    let written = async {
        login
            .stdin
            .write_all(format!("{code}\n").as_bytes())
            .await?;
        login.stdin.flush().await
    }
    .await;
    if let Err(error) = written {
        let message = last_line(&login.output).unwrap_or_else(|| error.to_string());
        drop_login(login).await;
        return Err(anyhow!("The Colab sign-in stopped: {message}"));
    }
    let status = tokio::time::timeout(LOGIN_FINISH_WAIT, login.child.wait()).await;
    let signed_in = token_file(&login.home).is_file();
    let result = match status {
        Ok(Ok(_)) | Err(_) if signed_in => finish_pending(&login.home).await,
        Ok(Ok(_)) => Err(anyhow!(
            "Google did not accept that code: {}",
            last_line(&login.output).unwrap_or_else(|| "no reason given".into())
        )),
        Ok(Err(error)) => Err(anyhow!("The Colab CLI failed: {error}")),
        Err(_) => Err(anyhow!("The Colab CLI did not finish the sign-in in time.")),
    };
    drop_login(login).await;
    result
}

pub async fn login_cancel(id: &str) {
    if let Some(login) = logins().lock().await.remove(id) {
        drop_login(login).await;
    }
}

// --- status ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    /// The Google account's email.
    pub id: String,
    pub active: bool,
    pub paused: bool,
    /// Plan, compute units, and eligibility; `None` without a sign-in.
    pub account: Option<Account>,
    /// Runtimes assigned to the account; `None` when they could not be read.
    pub runtimes: Option<Vec<Runtime>>,
}

async fn status_of(id: &str, active: bool, paused: bool, force: bool) -> AccountStatus {
    let home = account_home(id);
    let (account, runtimes) = tokio::join!(
        super::colab_account::account(&home, force),
        super::colab_runtimes::runtimes(&home),
    );
    AccountStatus {
        id: id.to_string(),
        active,
        paused,
        account,
        runtimes: runtimes.ok(),
    }
}

/// Every saved account with its plan, balance, and runtimes, read in parallel.
pub async fn accounts(force: bool) -> Vec<AccountStatus> {
    ensure_imported().await;
    let file = load_accounts();
    let checks = file.accounts.iter().map(|account| {
        status_of(
            &account.id,
            file.active.as_deref() == Some(account.id.as_str()),
            account.paused,
            force,
        )
    });
    futures::future::join_all(checks).await
}

pub async fn invalidate() {
    super::colab_account::invalidate_all().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::colab::parse_flavor;

    fn status(id: &str, paused: bool, account: Option<Account>) -> AccountStatus {
        AccountStatus {
            id: id.into(),
            active: false,
            paused,
            account,
            runtimes: None,
        }
    }

    fn account(eligible: Option<&[&str]>, runtimes: u32, balance: f64) -> Account {
        Account {
            eligible: eligible.map(|ids| ids.iter().map(|id| id.to_string()).collect()),
            active_runtimes: Some(runtimes),
            balance: Some(balance),
            ..Account::default()
        }
    }

    #[test]
    fn auto_skips_paused_failing_and_ineligible_accounts() {
        let failing = Account {
            error: Some("HTTP 401".into()),
            ..account(Some(&["t4"]), 0, 500.0)
        };
        let statuses = [
            status(
                "paused@x.com",
                true,
                Some(account(Some(&["a100"]), 0, 900.0)),
            ),
            status("broken@x.com", false, Some(failing)),
            status("free@x.com", false, Some(account(Some(&["t4"]), 0, 0.0))),
            status(
                "pro@x.com",
                false,
                Some(account(Some(&["t4", "a100"]), 0, 80.0)),
            ),
            status("unknown@x.com", false, Some(account(None, 0, 1000.0))),
            status("signed-out@x.com", false, None),
        ];
        assert_eq!(pick_auto(&statuses, "a100").as_deref(), Some("pro@x.com"));
        assert_eq!(
            pick_auto(&statuses, "h100").as_deref(),
            Some("unknown@x.com")
        );
        assert_eq!(pick_auto(&statuses[..2], "a100"), None);
    }

    #[test]
    fn auto_prefers_idle_accounts_then_the_most_units() {
        let statuses = [
            status("busy@x.com", false, Some(account(Some(&["t4"]), 1, 300.0))),
            status("idle@x.com", false, Some(account(Some(&["t4"]), 0, 10.0))),
            status("rich@x.com", false, Some(account(Some(&["t4"]), 0, 90.0))),
        ];
        assert_eq!(pick_auto(&statuses, "t4").as_deref(), Some("rich@x.com"));
    }

    #[test]
    fn accounts_are_found_by_email_or_unique_name() {
        let file = AccountsFile {
            active: None,
            accounts: ["alice@gmail.com", "bob@gmail.com", "bob@lab.org"]
                .iter()
                .map(|id| StoredAccount {
                    id: id.to_string(),
                    added_at: 0,
                    paused: false,
                })
                .collect(),
        };
        assert_eq!(find_account(&file, "Alice").unwrap().id, "alice@gmail.com");
        assert_eq!(
            find_account(&file, "bob@lab.org").unwrap().id,
            "bob@lab.org"
        );
        assert!(find_account(&file, "bob").is_none());
        assert!(find_account(&file, "carol").is_none());
    }

    #[test]
    fn account_ids_cannot_escape_their_folder() {
        assert!(valid_account_id("first.last+lab@gmail.com"));
        for bad in [
            "",
            "../x",
            ".pending-1",
            "a/b@x.com",
            "a b@x.com",
            "a..b@x.com",
        ] {
            assert!(!valid_account_id(bad), "{bad}");
        }
    }

    #[test]
    fn flavors_carry_the_account() {
        let flavor = parse_flavor(Some("a100:highmem@alice@gmail.com")).unwrap();
        assert_eq!(
            flavor.account,
            AccountChoice::Named("alice@gmail.com".into())
        );
        assert_eq!(flavor.id(), "a100:highmem@alice@gmail.com");
        assert_eq!(
            parse_flavor(Some("t4@auto")).unwrap().account,
            AccountChoice::Auto
        );
        assert_eq!(parse_flavor(Some("@bob")).unwrap().id(), "t4@bob");
        assert_eq!(parse_flavor(None).unwrap().account, AccountChoice::Active);
        assert!(parse_flavor(Some("t4@../evil")).is_err());
    }

    #[test]
    fn the_sign_in_link_is_found_and_asks_for_an_account() {
        let line = "  https://accounts.google.com/o/oauth2/auth?response_type=code&client_id=x&prompt=consent&token_usage=remote";
        let url = auth_url(line).unwrap();
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/auth?"));
        assert!(with_account_chooser(&url).contains("prompt=select_account%20consent"));
        assert!(auth_url("Enter the authorization code: ").is_none());
    }
}
