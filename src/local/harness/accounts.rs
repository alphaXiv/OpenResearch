//! Several accounts per harness, and the quota each one has left.
//!
//! Claude Code, Codex, and Antigravity can each run on more than one account.
//! The account the harness is already signed in to is `default`; every other
//! account lives in its own folder under the orx data dir, which the harness
//! sees as its config home (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, or `$HOME` for
//! `agy`), so one account's credentials never reach a turn on another.
//!
//! Quota is tracked per account and per pool. A pool is the quota a group of
//! models shares: one per harness, except Antigravity, whose Gemini models and
//! its Claude/GPT models draw on separate quotas. What an account has left
//! comes from three sources, the tightest of which wins:
//! - the provider's own report, where one can be read (Codex app-server's
//!   rate limits; Claude's and Antigravity's usage endpoints, best effort);
//! - a limit the user sets (turns or tokens per window), against the usage
//!   orx recorded for that account;
//! - a usage-limit failure, which blocks the pool until the reset time the
//!   message names, or for the window when it names none.
//!
//! Before each turn the chat picks the account with headroom for its model's
//! pool, staying on its last account while that one has room, and a turn that
//! stops on a limit moves to another account before failover tries another
//! harness.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{anyhow, Result};
use crate::store::harness_accounts::{QuotaSnapshot, TrackedUsage};
use crate::store::Store;

/// The account a harness is signed in to outside orx.
pub const DEFAULT: &str = "default";

/// Harnesses that can run on more than one account.
pub const ACCOUNT_HARNESSES: &[&str] = &["claude-code", "codex", "antigravity"];

/// Below this share of quota left, an account is passed over when another
/// has more room.
const LOW: f64 = 0.15;

/// A provider report older than this no longer says anything about now.
const SNAPSHOT_TTL_MS: i64 = 6 * 3600 * 1000;

/// How long a limit failure blocks a pool when neither the message nor the
/// user's settings say when it resets.
const DEFAULT_BLOCK_MS: i64 = 3600 * 1000;

pub fn supports_accounts(harness: &str) -> bool {
    ACCOUNT_HARNESSES.contains(&harness)
}

/// A quota models share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Pool {
    pub id: &'static str,
    pub label: &'static str,
}

const CLAUDE_POOLS: &[Pool] = &[Pool {
    id: "claude",
    label: "Claude models",
}];
const CODEX_POOLS: &[Pool] = &[Pool {
    id: "openai",
    label: "OpenAI models",
}];
/// Antigravity meters its Gemini models and the Claude/GPT models it also
/// offers against different quotas.
const AGY_POOLS: &[Pool] = &[
    Pool {
        id: "gemini",
        label: "Gemini models",
    },
    Pool {
        id: "partner",
        label: "Claude & GPT models",
    },
];

pub fn pools(harness: &str) -> &'static [Pool] {
    match harness {
        "claude-code" => CLAUDE_POOLS,
        "codex" => CODEX_POOLS,
        "antigravity" => AGY_POOLS,
        _ => &[],
    }
}

/// The pool `model` draws on. A chat with no model picked runs the harness
/// default, which for Antigravity is a Gemini model.
pub fn pool_for(harness: &str, model: Option<&str>) -> &'static str {
    match harness {
        "antigravity" => {
            let model = model.unwrap_or_default().to_ascii_lowercase();
            if ["claude", "opus", "sonnet", "haiku", "gpt"]
                .iter()
                .any(|marker| model.contains(marker))
            {
                "partner"
            } else {
                "gemini"
            }
        }
        _ => pools(harness).first().map_or("default", |pool| pool.id),
    }
}

fn pool_label(harness: &str, pool: &str) -> &'static str {
    pools(harness)
        .iter()
        .find(|candidate| candidate.id == pool)
        .map_or("Models", |pool| pool.label)
}

// --- stored accounts -------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitUnit {
    Turns,
    Tokens,
}

/// A limit the user set for one pool: `limit` turns or tokens every
/// `window_hours`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolLimit {
    pub pool: String,
    pub limit: f64,
    pub unit: LimitUnit,
    pub window_hours: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAccount {
    pub harness: String,
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub added_at: i64,
    /// Left out when chats pick an account.
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub limits: Vec<PoolLimit>,
}

impl StoredAccount {
    fn implicit_default(harness: &str) -> Self {
        Self {
            harness: harness.to_string(),
            id: DEFAULT.to_string(),
            label: None,
            added_at: 0,
            paused: false,
            limits: Vec::new(),
        }
    }

    pub fn is_default(&self) -> bool {
        self.id == DEFAULT
    }

    fn limit(&self, pool: &str) -> Option<&PoolLimit> {
        self.limits
            .iter()
            .find(|limit| limit.pool == pool && limit.limit > 0.0 && limit.window_hours > 0.0)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountsFile {
    #[serde(default)]
    accounts: Vec<StoredAccount>,
}

fn root() -> PathBuf {
    crate::store::data_dir().join("agents").join("accounts")
}

fn accounts_file() -> PathBuf {
    crate::store::data_dir()
        .join("agents")
        .join("accounts.json")
}

fn load() -> AccountsFile {
    std::fs::read(accounts_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(file: &AccountsFile) -> Result<()> {
    let path = accounts_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::local::git::atomic_write_with_mode(
        &path,
        &serde_json::to_vec_pretty(file)?,
        Some(0o600),
    )?;
    Ok(())
}

pub fn valid_id(id: &str) -> bool {
    id != DEFAULT
        && (1..=32).contains(&id.len())
        && id
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The folder an extra account's harness uses as its config home; `None` for
/// the default account, which keeps the harness's own.
pub fn account_home(harness: &str, id: &str) -> Option<PathBuf> {
    (id != DEFAULT && valid_id(id) && supports_accounts(harness))
        .then(|| root().join(harness).join(id))
}

/// Every account of `harness`, the default first.
pub fn configured(harness: &str) -> Vec<StoredAccount> {
    if !supports_accounts(harness) {
        return Vec::new();
    }
    let file = load();
    let mut accounts: Vec<StoredAccount> = file
        .accounts
        .into_iter()
        .filter(|account| account.harness == harness)
        .collect();
    let default = accounts
        .iter()
        .position(StoredAccount::is_default)
        .map(|index| accounts.remove(index))
        .unwrap_or_else(|| StoredAccount::implicit_default(harness));
    accounts.retain(|account| account_home(harness, &account.id).is_some_and(|home| home.is_dir()));
    accounts.sort_by_key(|account| account.added_at);
    accounts.insert(0, default);
    accounts
}

pub fn find(harness: &str, id: &str) -> Option<StoredAccount> {
    configured(harness)
        .into_iter()
        .find(|account| account.id == id)
}

/// Save a new account. It still has to be signed in (see [`login_env`]).
pub fn add(harness: &str, id: &str, label: Option<String>) -> Result<StoredAccount> {
    if !supports_accounts(harness) {
        return Err(anyhow!("{harness} does not support more than one account."));
    }
    let id = id.trim().to_ascii_lowercase();
    if !valid_id(&id) {
        return Err(anyhow!(
            "Use 1–32 lowercase letters, digits, `-` or `_` for the account name (not `default`)."
        ));
    }
    if harness == "antigravity" && cfg!(windows) {
        return Err(anyhow!(
            "More than one Antigravity account needs macOS or Linux."
        ));
    }
    let mut file = load();
    if file
        .accounts
        .iter()
        .any(|account| account.harness == harness && account.id == id)
        || account_home(harness, &id).is_some_and(|home| home.is_dir())
    {
        return Err(anyhow!(
            "There is already a {harness} account named `{id}`."
        ));
    }
    let home = account_home(harness, &id).expect("validated id");
    create_private_dir(&home)?;
    let account = StoredAccount {
        harness: harness.to_string(),
        id,
        label: label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty()),
        added_at: crate::store::now_ms(),
        paused: false,
        limits: Vec::new(),
    };
    file.accounts.push(account.clone());
    save(&file)?;
    Ok(account)
}

/// Delete an extra account, its folder (and so its credentials), and what
/// orx recorded about it.
pub fn remove(harness: &str, id: &str) -> Result<()> {
    let Some(home) = account_home(harness, id) else {
        return Err(anyhow!("The default account cannot be removed here."));
    };
    let mut file = load();
    file.accounts
        .retain(|account| !(account.harness == harness && account.id == id));
    save(&file)?;
    if home.exists() {
        std::fs::remove_dir_all(&home)?;
    }
    Store::open()?.forget_account(harness, id)?;
    Ok(())
}

/// Changes to an account's settings; absent fields stay as they are.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPatch {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub paused: Option<bool>,
    /// Replaces every limit when present.
    #[serde(default)]
    pub limits: Option<Vec<PoolLimit>>,
    /// Forget a limit failure, e.g. after the user bought more usage.
    #[serde(default)]
    pub clear_block: bool,
}

pub fn update(harness: &str, id: &str, patch: AccountPatch) -> Result<StoredAccount> {
    let mut account =
        find(harness, id).ok_or_else(|| anyhow!("There is no {harness} account named `{id}`."))?;
    if let Some(label) = patch.label {
        let label = label.trim().to_string();
        account.label = (!label.is_empty()).then_some(label);
    }
    if let Some(paused) = patch.paused {
        account.paused = paused;
    }
    if let Some(limits) = patch.limits {
        let mut kept: Vec<PoolLimit> = Vec::new();
        for limit in limits {
            if !pools(harness).iter().any(|pool| pool.id == limit.pool) {
                return Err(anyhow!("{harness} has no `{}` quota pool.", limit.pool));
            }
            if !(limit.limit.is_finite() && limit.window_hours.is_finite())
                || limit.limit < 0.0
                || limit.window_hours <= 0.0
                || limit.window_hours > 24.0 * 31.0
            {
                return Err(anyhow!(
                    "A limit needs a positive amount and a window of up to 31 days."
                ));
            }
            kept.retain(|existing| existing.pool != limit.pool);
            if limit.limit > 0.0 {
                kept.push(limit);
            }
        }
        account.limits = kept;
    }
    if patch.clear_block {
        Store::open()?.clear_account_limits(harness, id)?;
    }
    let mut file = load();
    file.accounts
        .retain(|stored| !(stored.harness == harness && stored.id == id));
    file.accounts.push(account.clone());
    save(&file)?;
    Ok(account)
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

// --- launching on an account -------------------------------------------------------

/// Environment that points `harness` at an extra account's config home,
/// preparing the home first. Empty for the default account.
pub fn launch_env(harness: &str, id: &str) -> Result<Vec<(&'static str, OsString)>> {
    let Some(home) = account_home(harness, id) else {
        return Ok(Vec::new());
    };
    if !home.is_dir() {
        return Err(anyhow!(
            "The {harness} account `{id}` was removed. Pick another account in Settings → Harnesses."
        ));
    }
    use crate::local::native_store;
    Ok(match harness {
        "claude-code" => {
            let home = native_store::prepare_claude_account(&home)?;
            vec![
                ("CLAUDE_CONFIG_DIR", home.clone().into_os_string()),
                // Keeps each account's Keychain entry apart on macOS.
                ("CLAUDE_SECURESTORAGE_CONFIG_DIR", home.into_os_string()),
            ]
        }
        "codex" => vec![(
            "CODEX_HOME",
            native_store::prepare_codex_account(&home)?.into_os_string(),
        )],
        "antigravity" => {
            let home = native_store::prepare_agy_account(&home.join("home"))?;
            let mut env = vec![
                ("HOME", home.clone().into_os_string()),
                ("USERPROFILE", home.into_os_string()),
            ];
            // On Linux `agy` keeps its sign-in in the Secret Service keyring,
            // which every account would share whatever HOME says. Without a
            // session bus it falls back to `.gemini/antigravity-cli/
            // antigravity-oauth-token` inside the account's own home.
            if cfg!(all(unix, not(target_os = "macos"))) {
                env.push((
                    "DBUS_SESSION_BUS_ADDRESS",
                    OsString::from("unix:path=/nonexistent/orx-account-bus"),
                ));
            }
            env
        }
        _ => Vec::new(),
    })
}

/// The extra account a turn runs on, if any. Chats whose native session
/// lives in the user's own (legacy) store stay on the default account, whose
/// home that store is.
pub fn extra_account(
    account: Option<&str>,
    native_store: crate::local::native_store::NativeStore,
) -> Option<&str> {
    account.filter(|id| {
        *id != DEFAULT && native_store == crate::local::native_store::NativeStore::Isolated
    })
}

/// [`launch_env`] for signing an account in; the same homes, so a login lands
/// where turns will look for it.
pub fn login_env(harness: &str, id: &str) -> Result<Vec<(&'static str, OsString)>> {
    if account_home(harness, id).is_none() {
        return Err(anyhow!(
            "Sign the default account in from the harness card instead."
        ));
    }
    launch_env(harness, id)
}

/// The shell form of signing an account in, for users who prefer a terminal.
pub fn login_command_line(harness: &str, id: &str) -> Option<String> {
    let home = account_home(harness, id)?;
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    Some(match harness {
        "claude-code" => format!(
            "CLAUDE_CONFIG_DIR={0} CLAUDE_SECURESTORAGE_CONFIG_DIR={0} claude auth login",
            quote(&home)
        ),
        "codex" => format!("CODEX_HOME={} codex login", quote(&home)),
        "antigravity" if cfg!(all(unix, not(target_os = "macos"))) => format!(
            "HOME={} DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/orx-account-bus agy",
            quote(&home.join("home"))
        ),
        "antigravity" => format!("HOME={} agy", quote(&home.join("home"))),
        _ => return None,
    })
}

/// Files that exist once an account is signed in.
fn credential_files(harness: &str, id: &str) -> Vec<PathBuf> {
    use crate::local::native_store::{claude_home, codex_home, NativeStore};
    match (harness, account_home(harness, id)) {
        ("claude-code", Some(home)) => {
            vec![home.join(".credentials.json"), home.join(".claude.json")]
        }
        ("claude-code", None) => {
            let legacy = claude_home(NativeStore::Legacy);
            let mut files = vec![
                legacy.join(".credentials.json"),
                legacy.join(".claude.json"),
            ];
            files.extend(dirs::home_dir().map(|home| home.join(".claude.json")));
            files
        }
        ("codex", Some(home)) => vec![home.join("auth.json")],
        ("codex", None) => vec![codex_home(NativeStore::Legacy).join("auth.json")],
        ("antigravity", Some(home)) => super::antigravity::agy_account_files_in(&home.join("home")),
        ("antigravity", None) => dirs::home_dir()
            .map(|home| super::antigravity::agy_account_files_in(&home))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Whether an extra account has credentials saved. The default account is
/// taken as signed in: its harness card reports when it is not.
pub fn signed_in(harness: &str, id: &str) -> bool {
    if id == DEFAULT {
        return true;
    }
    match harness {
        "claude-code" => {
            let Some(home) = account_home(harness, id) else {
                return false;
            };
            home.join(".credentials.json").is_file()
                || read_json(&home.join(".claude.json"))
                    .is_some_and(|value| value.get("oauthAccount").is_some_and(Value::is_object))
        }
        _ => credential_files(harness, id)
            .iter()
            .any(|path| path.is_file()),
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Who an account is signed in as, read from its local files only.
pub fn email(harness: &str, id: &str) -> Option<String> {
    for path in credential_files(harness, id) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = text.trim();
        let Some(value) = serde_json::from_str::<Value>(text)
            .ok()
            .or_else(|| super::detect::jwt_payload(text))
        else {
            continue;
        };
        let found = match harness {
            "claude-code" => value
                .pointer("/oauthAccount/emailAddress")
                .and_then(Value::as_str)
                .map(str::to_string),
            "codex" => value
                .pointer("/tokens/id_token")
                .and_then(Value::as_str)
                .and_then(super::detect::jwt_payload)
                .and_then(|claims| claims["email"].as_str().map(str::to_string))
                .or_else(|| {
                    value
                        .get("OPENAI_API_KEY")
                        .and_then(Value::as_str)
                        .filter(|key| !key.is_empty())
                        .map(|_| "API key".to_string())
                }),
            _ => super::antigravity::find_email(&value),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

// --- quota status -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStatus {
    pub window: String,
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    pub read_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QuotaState {
    Ok,
    Low,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolStatus {
    pub pool: &'static str,
    pub label: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<PoolLimit>,
    /// Usage against `limit`, in its unit and window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    /// What orx recorded on this account and pool in the last 24 hours.
    pub tracked_turns: u64,
    pub tracked_tokens: u64,
    /// The provider's own report, freshest first.
    pub windows: Vec<WindowStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_until: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// Share of quota left, 0–1, from the tightest source; `None` when no
    /// source says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    pub state: QuotaState,
}

impl PoolStatus {
    fn blocked(&self) -> bool {
        self.state == QuotaState::Blocked
    }

    /// How attractive this pool is for the next turn; `None` when blocked.
    /// An unknown quota ranks below any known room but above a nearly
    /// exhausted one.
    fn score(&self) -> Option<f64> {
        (!self.blocked()).then(|| self.remaining.unwrap_or(LOW + 0.05))
    }
}

/// The quota of `pool` on `account` at `now`, without asking the provider.
pub fn pool_status(store: &Store, account: &StoredAccount, pool: &str, now: i64) -> PoolStatus {
    let harness = account.harness.as_str();
    let pool_static = pools(harness)
        .iter()
        .find(|candidate| candidate.id == pool)
        .map_or("default", |pool| pool.id);
    let day = store
        .account_usage_since(harness, &account.id, pool, now - 24 * 3600 * 1000)
        .unwrap_or_default();
    let limit = account.limit(pool).cloned();
    let used = limit.as_ref().map(|limit| {
        let since = now - (limit.window_hours * 3600.0 * 1000.0) as i64;
        let usage: TrackedUsage = store
            .account_usage_since(harness, &account.id, pool, since)
            .unwrap_or_default();
        match limit.unit {
            LimitUnit::Turns => usage.turns as f64,
            LimitUnit::Tokens => usage.tokens as f64,
        }
    });
    let windows: Vec<WindowStatus> = store
        .quota_snapshots(harness, &account.id)
        .unwrap_or_default()
        .into_iter()
        .filter(|snapshot| snapshot.pool == pool)
        .map(|snapshot| WindowStatus {
            window: snapshot.window,
            used_percent: snapshot.used_percent,
            resets_at: snapshot.resets_at_ms,
            read_at: snapshot.read_at_ms,
        })
        .collect();
    let block = store
        .account_limit(harness, &account.id, pool)
        .ok()
        .flatten()
        .filter(|(until, _)| *until > now);
    let mut remaining: Option<f64> = None;
    let mut tighten = |value: f64| {
        let value = value.clamp(0.0, 1.0);
        remaining = Some(remaining.map_or(value, |current: f64| current.min(value)));
    };
    if let (Some(limit), Some(used)) = (&limit, used) {
        tighten(1.0 - used / limit.limit);
    }
    for window in &windows {
        if now - window.read_at > SNAPSHOT_TTL_MS {
            continue;
        }
        if window.resets_at.is_some_and(|reset| reset <= now) {
            tighten(1.0);
        } else {
            tighten(1.0 - window.used_percent / 100.0);
        }
    }
    let state = if block.is_some() || remaining.is_some_and(|left| left <= 0.0) {
        QuotaState::Blocked
    } else {
        match remaining {
            Some(left) if left < LOW => QuotaState::Low,
            Some(_) => QuotaState::Ok,
            None => QuotaState::Unknown,
        }
    };
    PoolStatus {
        pool: pool_static,
        label: pool_label(harness, pool),
        limit,
        used,
        tracked_turns: day.turns,
        tracked_tokens: day.tokens,
        windows,
        blocked_until: block.as_ref().map(|(until, _)| *until),
        blocked_reason: block.and_then(|(_, reason)| reason),
        remaining,
        state,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub harness: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub is_default: bool,
    pub signed_in: bool,
    pub paused: bool,
    pub limits: Vec<PoolLimit>,
    pub pools: Vec<PoolStatus>,
    /// The shell command that signs this account in; `None` for the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_command: Option<String>,
}

pub fn account_status(store: &Store, account: &StoredAccount, now: i64) -> AccountStatus {
    let harness = account.harness.as_str();
    AccountStatus {
        harness: account.harness.clone(),
        id: account.id.clone(),
        label: account.label.clone(),
        email: email(harness, &account.id),
        is_default: account.is_default(),
        signed_in: signed_in(harness, &account.id),
        paused: account.paused,
        limits: account.limits.clone(),
        pools: pools(harness)
            .iter()
            .map(|pool| pool_status(store, account, pool.id, now))
            .collect(),
        login_command: login_command_line(harness, &account.id),
    }
}

/// Every account of every harness that supports them, or of `only`.
pub fn statuses(only: Option<&str>) -> Result<Vec<AccountStatus>> {
    let store = Store::open()?;
    let now = crate::store::now_ms();
    Ok(ACCOUNT_HARNESSES
        .iter()
        .filter(|harness| only.is_none_or(|only| only == **harness))
        .flat_map(|harness| configured(harness))
        .map(|account| account_status(&store, &account, now))
        .collect())
}

/// What orx can read about each harness's quota by itself.
pub fn quota_source(harness: &str) -> &'static str {
    match harness {
        "codex" => "Codex reports its 5-hour and weekly limits for ChatGPT sign-ins; API-key accounts report none.",
        "claude-code" => "Claude's usage endpoint reports 5-hour and weekly use for claude.ai sign-ins when its token is readable (Linux, or a fresh token); API keys and Keychain-only sign-ins report none.",
        "antigravity" => "Antigravity's model list reports the share of quota left per model, best effort; when it cannot be read, set limits by hand.",
        _ => "",
    }
}

// --- choosing an account ---------------------------------------------------------

/// The accounts a turn on `harness` may use, with their quota for `pool`.
fn candidates(harness: &str, pool: &str) -> Vec<(String, PoolStatus)> {
    let Ok(store) = Store::open() else {
        return Vec::new();
    };
    let now = crate::store::now_ms();
    configured(harness)
        .into_iter()
        .filter(|account| !account.paused && signed_in(harness, &account.id))
        .map(|account| {
            let status = pool_status(&store, &account, pool, now);
            (account.id, status)
        })
        .collect()
}

/// Pick from `candidates`: the sticky account while it has room, otherwise
/// the one with the most quota left. `None` when every candidate is blocked.
fn pick(
    candidates: &[(String, PoolStatus)],
    sticky: Option<&str>,
    exclude: &[String],
) -> Option<String> {
    let usable = candidates
        .iter()
        .filter(|(id, _)| !exclude.contains(id))
        .filter_map(|(id, status)| Some((id, status.score()?)));
    if let Some(sticky) = sticky {
        if usable
            .clone()
            .any(|(id, score)| id == sticky && score >= LOW)
        {
            return Some(sticky.to_string());
        }
    }
    usable
        .fold(None::<(&String, f64)>, |best, (id, score)| match best {
            Some((_, top)) if top >= score => best,
            _ => Some((id, score)),
        })
        .map(|(id, _)| id.clone())
}

/// The account the next turn on `harness` and `model` should run on.
/// `None` when the harness has only its default account (nothing to route),
/// or every account is out of quota for that model's pool.
pub fn choose(harness: &str, model: Option<&str>, sticky: Option<&str>) -> Option<String> {
    if !supports_accounts(harness) {
        return None;
    }
    let candidates = candidates(harness, pool_for(harness, model));
    if candidates.len() < 2 && candidates.iter().all(|(id, _)| id == DEFAULT) {
        return None;
    }
    pick(&candidates, sticky, &[])
}

/// Another account with room for `model` after `tried` ran out.
pub fn next_account(harness: &str, model: Option<&str>, tried: &[String]) -> Option<String> {
    if !supports_accounts(harness) {
        return None;
    }
    pick(&candidates(harness, pool_for(harness, model)), None, tried)
}

/// For a harness with several pools: a model in another pool that some
/// account still has room for, and that account. `current` is preferred.
pub fn other_pool_model(
    harness: &str,
    model: Option<&str>,
    current: &str,
    models: &[super::ModelInfo],
    recommended: Option<&str>,
) -> Option<(String, String)> {
    let from = pool_for(harness, model);
    let mut options: Vec<&str> = recommended
        .into_iter()
        .chain(models.iter().map(|model| model.id.as_str()))
        .filter(|id| pool_for(harness, Some(id)) != from)
        .collect();
    options.dedup();
    let target = *options.first()?;
    let account = pick(
        &candidates(harness, pool_for(harness, Some(target))),
        Some(current),
        &[],
    )?;
    Some((target.to_string(), account))
}

// --- recording ----------------------------------------------------------------------

/// Record one turn attempt against the account it ran on.
pub fn record_turn(
    harness: &str,
    account: &str,
    model: Option<&str>,
    execution_id: &str,
    outcome: &str,
) {
    if !supports_accounts(harness) {
        return;
    }
    let Ok(store) = Store::open() else {
        return;
    };
    let tokens = store.usage_execution_tokens(execution_id).unwrap_or(0);
    let _ = store.record_account_usage(
        harness,
        account,
        pool_for(harness, model),
        model,
        crate::store::now_ms(),
        tokens,
        outcome,
    );
}

/// Block `account`'s pool for `model` after a usage-limit failure, until the
/// reset time `message` names, else the user's window, else an hour.
pub fn record_limit(harness: &str, account: &str, model: Option<&str>, message: &str) {
    if !supports_accounts(harness) {
        return;
    }
    let now = crate::store::now_ms();
    let pool = pool_for(harness, model);
    let window = find(harness, account)
        .and_then(|account| account.limit(pool).map(|limit| limit.window_hours))
        .map(|hours| (hours * 3600.0 * 1000.0) as i64);
    let until = parse_reset(message, now)
        .filter(|until| *until > now)
        .unwrap_or_else(|| now + window.unwrap_or(DEFAULT_BLOCK_MS));
    let reason: String = message.trim().chars().take(300).collect();
    if let Ok(store) = Store::open() {
        let _ = store.set_account_limit(harness, account, pool, until, Some(&reason));
    }
}

/// When a limit message says usage comes back: an epoch after `|` (Claude's
/// `usage limit reached|1760000000`), `try again in 2 days 3 hours`, `resets
/// in 45m`, or `retry after 120 seconds`.
pub fn parse_reset(message: &str, now: i64) -> Option<i64> {
    if let Some((_, tail)) = message.rsplit_once('|') {
        if let Ok(epoch) = tail.trim().parse::<i64>() {
            return Some(if epoch > 10_000_000_000 {
                epoch
            } else {
                epoch * 1000
            });
        }
    }
    let lower = message.to_ascii_lowercase();
    for marker in [
        "try again in",
        "resets in",
        "reset in",
        "retry after",
        "retry in",
        "available in",
        "wait",
    ] {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        if let Some(duration) = parse_duration_ms(&lower[start + marker.len()..]) {
            return Some(now + duration);
        }
    }
    None
}

/// The leading duration of `text`: `2 days 3 hours`, `3h 20m`, `45 minutes`,
/// `120 seconds`. Stops at the first word that is neither.
fn parse_duration_ms(text: &str) -> Option<i64> {
    let mut total = 0f64;
    let mut found = false;
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let mut number = String::new();
        while chars
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || *c == '.')
        {
            number.push(chars.next().unwrap());
        }
        if number.is_empty() {
            // "and" between parts, or the end.
            let rest: String = chars.clone().take(4).collect();
            if rest.starts_with("and ") {
                for _ in 0..4 {
                    chars.next();
                }
                continue;
            }
            break;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut unit = String::new();
        while chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            unit.push(chars.next().unwrap());
        }
        let Ok(value) = number.parse::<f64>() else {
            break;
        };
        let seconds = match unit.as_str() {
            "d" | "day" | "days" => 86_400.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" | "" => 1.0,
            _ => break,
        };
        total += value * seconds;
        found = true;
    }
    found.then_some((total * 1000.0) as i64)
}

// --- reading quota from providers --------------------------------------------------

const PROVIDER_TIMEOUT: Duration = Duration::from_secs(20);

/// Ask the provider what `account` has left and store the answer. `Ok` with
/// no windows means the provider answered but reports no quota for this
/// sign-in; `Err` says why it could not be read.
pub async fn refresh(harness: &str, account: &str) -> std::result::Result<usize, String> {
    let read = match harness {
        "codex" => read_codex(account).await,
        "claude-code" => read_claude(account).await,
        "antigravity" => read_agy(account).await,
        _ => return Err("This harness has no quota to read.".into()),
    }?;
    let store = Store::open().map_err(|error| error.to_string())?;
    store
        .replace_quota_snapshots(harness, account, &read)
        .map_err(|error| error.to_string())?;
    Ok(read.len())
}

fn snapshot(
    harness: &str,
    account: &str,
    pool: &str,
    window: String,
    used_percent: f64,
    resets_at_ms: Option<i64>,
) -> QuotaSnapshot {
    QuotaSnapshot {
        harness: harness.to_string(),
        account: account.to_string(),
        pool: pool.to_string(),
        window,
        used_percent: used_percent.clamp(0.0, 100.0),
        resets_at_ms,
        read_at_ms: crate::store::now_ms(),
    }
}

fn window_name(minutes: i64) -> String {
    match minutes {
        300 => "5h".into(),
        10_080 => "weekly".into(),
        1_440 => "daily".into(),
        m if m % 1_440 == 0 => format!("{}d", m / 1_440),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

/// Codex's app-server answers `account/rateLimits/read` for the account in
/// its `CODEX_HOME`.
async fn read_codex(account: &str) -> std::result::Result<Vec<QuotaSnapshot>, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let bin = super::codex::find_codex().ok_or("Codex is not installed.")?;
    let account_id = account.to_string();
    let env = tokio::task::spawn_blocking(move || -> Result<Vec<(&'static str, OsString)>> {
        if account_id == DEFAULT {
            let home = crate::local::native_store::prepare_codex(
                crate::local::native_store::NativeStore::Isolated,
            )?;
            Ok(vec![("CODEX_HOME", home.into_os_string())])
        } else {
            launch_env("codex", &account_id)
        }
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    let mut command = tokio::process::Command::new(bin);
    command
        .arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::local::chat::prepare_env(&mut command);
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start Codex: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("Codex has no input.")?;
    let stdout = child.stdout.take().ok_or("Codex has no output.")?;
    let exchange = async {
        let lines = [
            json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "orx", "title": "OpenResearch", "version": env!("CARGO_PKG_VERSION")}}}),
            json!({"method": "initialized"}),
            json!({"id": 2, "method": "account/rateLimits/read"}),
        ];
        for line in lines {
            stdin
                .write_all(format!("{line}\n").as_bytes())
                .await
                .map_err(|error| error.to_string())?;
        }
        stdin.flush().await.map_err(|error| error.to_string())?;
        let mut reader = BufReader::new(stdout).lines();
        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|error| error.to_string())?
        {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value["id"] != 2 {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(error["message"]
                    .as_str()
                    .unwrap_or("Codex could not read its rate limits.")
                    .to_string());
            }
            return Ok(value["result"].clone());
        }
        Err("Codex exited before reporting its rate limits.".to_string())
    };
    let result = tokio::time::timeout(PROVIDER_TIMEOUT, exchange)
        .await
        .map_err(|_| "Codex did not report its rate limits in time.".to_string())?;
    let _ = child.kill().await;
    Ok(parse_codex_limits(&result?, account))
}

fn parse_codex_limits(result: &Value, account: &str) -> Vec<QuotaSnapshot> {
    let mut snapshots = Vec::new();
    let limits = &result["rateLimits"];
    for key in ["primary", "secondary"] {
        let window = &limits[key];
        let Some(used) = window["usedPercent"].as_f64() else {
            continue;
        };
        let name = window["windowDurationMins"]
            .as_i64()
            .map(window_name)
            .unwrap_or_else(|| key.to_string());
        let resets = window["resetsAt"].as_i64().map(|seconds| seconds * 1000);
        snapshots.push(snapshot("codex", account, "openai", name, used, resets));
    }
    snapshots
}

/// The OAuth access token Claude Code saved for an account, if it is still
/// valid. macOS keeps it in the Keychain, where it cannot be read.
fn claude_access_token(account: &str) -> Option<String> {
    let path = match account_home("claude-code", account) {
        Some(home) => home.join(".credentials.json"),
        None => {
            crate::local::native_store::claude_home(crate::local::native_store::NativeStore::Legacy)
                .join(".credentials.json")
        }
    };
    let value = read_json(&path)?;
    let oauth = value.get("claudeAiOauth")?;
    if oauth["expiresAt"]
        .as_i64()
        .is_some_and(|expires| expires <= crate::store::now_ms())
    {
        return None;
    }
    oauth["accessToken"].as_str().map(str::to_string)
}

async fn read_claude(account: &str) -> std::result::Result<Vec<QuotaSnapshot>, String> {
    let token = claude_access_token(account).ok_or(
        "No current claude.ai sign-in token could be read for this account (API keys and Keychain sign-ins report no quota). Run a chat on it to refresh the token, or set a limit by hand.",
    )?;
    let client = crate::net::remote_client()
        .timeout(PROVIDER_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .map_err(|error| format!("Claude's usage endpoint did not answer: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Claude's usage endpoint answered {}.",
            response.status()
        ));
    }
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    Ok(parse_claude_usage(&body, account))
}

fn parse_claude_usage(body: &Value, account: &str) -> Vec<QuotaSnapshot> {
    [("five_hour", "5h"), ("seven_day", "weekly")]
        .into_iter()
        .filter_map(|(key, name)| {
            let window = body.get(key)?;
            let used = window["utilization"].as_f64()?;
            let resets = window["resets_at"].as_str().and_then(parse_time_ms);
            Some(snapshot(
                "claude-code",
                account,
                "claude",
                name.into(),
                used,
                resets,
            ))
        })
        .collect()
}

async fn read_agy(account: &str) -> std::result::Result<Vec<QuotaSnapshot>, String> {
    let files = match account_home("antigravity", account) {
        Some(home) => super::antigravity::agy_account_files_in(&home.join("home")),
        None => dirs::home_dir()
            .map(|home| super::antigravity::agy_account_files_in(&home))
            .unwrap_or_default(),
    };
    let token = files
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_json::from_str::<Value>(text.trim()).ok())
        .find_map(|value| super::antigravity::find_string(&value, &["access_token", "accessToken"]))
        .ok_or("No Antigravity sign-in token could be read for this account.")?;
    let client = crate::net::remote_client()
        .timeout(PROVIDER_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post("https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels")
        .bearer_auth(token)
        .json(&json!({}))
        .send()
        .await
        .map_err(|error| format!("Antigravity did not answer: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Antigravity answered {} (its sign-in token may have expired; run a chat on this account to refresh it).",
            response.status()
        ));
    }
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    let snapshots = parse_agy_models(&body, account);
    if snapshots.is_empty() {
        return Err("Antigravity reported no quota for its models.".into());
    }
    Ok(snapshots)
}

/// Each model's `quotaInfo.remainingFraction`, folded to the tightest one
/// per pool: models of a pool share its quota.
fn parse_agy_models(body: &Value, account: &str) -> Vec<QuotaSnapshot> {
    let entries: Vec<(String, &Value)> = match &body["models"] {
        Value::Object(map) => map.iter().map(|(id, model)| (id.clone(), model)).collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(|model| {
                let id = model["id"].as_str().or_else(|| model["name"].as_str())?;
                Some((id.to_string(), model))
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut tightest: Vec<(&'static str, f64, Option<i64>)> = Vec::new();
    for (id, model) in entries {
        let quota = &model["quotaInfo"];
        let Some(left) = quota["remainingFraction"].as_f64() else {
            continue;
        };
        let pool = pool_for("antigravity", Some(&id));
        let resets = quota["resetTime"].as_str().and_then(parse_time_ms);
        match tightest.iter_mut().find(|(existing, ..)| *existing == pool) {
            Some(entry) if left < entry.1 => *entry = (pool, left, resets),
            Some(_) => {}
            None => tightest.push((pool, left, resets)),
        }
    }
    tightest
        .into_iter()
        .map(|(pool, left, resets)| {
            snapshot(
                "antigravity",
                account,
                pool,
                "quota".into(),
                (1.0 - left) * 100.0,
                resets,
            )
        })
        .collect()
}

/// An RFC 3339 time, with `Z` or a numeric offset, in epoch milliseconds.
fn parse_time_ms(stamp: &str) -> Option<i64> {
    let stamp = stamp.trim();
    let (_, time) = stamp.split_once('T')?;
    let offset_at = time.rfind(['+', '-']);
    let (base, offset_ms) = match offset_at {
        Some(at) => {
            let offset = &time[at..];
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let (hours, minutes) = offset[1..].split_once(':').unwrap_or((&offset[1..], "0"));
            let ms = (hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?) * 60_000;
            (&stamp[..stamp.len() - offset.len()], sign * ms)
        }
        None => (stamp, 0),
    };
    Some(crate::local::native_chats::parse_iso_ms(base)? - offset_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(remaining: Option<f64>, state: QuotaState) -> PoolStatus {
        PoolStatus {
            pool: "claude",
            label: "Claude models",
            limit: None,
            used: None,
            tracked_turns: 0,
            tracked_tokens: 0,
            windows: Vec::new(),
            blocked_until: None,
            blocked_reason: None,
            remaining,
            state,
        }
    }

    #[test]
    fn antigravity_splits_gemini_from_partner_models() {
        assert_eq!(
            pool_for("antigravity", Some("gemini-3.1-pro-high")),
            "gemini"
        );
        assert_eq!(pool_for("antigravity", None), "gemini");
        assert_eq!(
            pool_for("antigravity", Some("claude-opus-4-6-thinking")),
            "partner"
        );
        assert_eq!(
            pool_for("antigravity", Some("claude-sonnet-4-6")),
            "partner"
        );
        assert_eq!(
            pool_for("antigravity", Some("gpt-oss-120b-medium")),
            "partner"
        );
        assert_eq!(pool_for("claude-code", Some("opus")), "claude");
        assert_eq!(pool_for("codex", Some("gpt-5")), "openai");
        assert_eq!(pools("antigravity").len(), 2);
    }

    #[test]
    fn picking_stays_on_the_sticky_account_while_it_has_room() {
        let candidates = vec![
            ("default".to_string(), status(Some(0.9), QuotaState::Ok)),
            ("work".to_string(), status(Some(0.5), QuotaState::Ok)),
        ];
        assert_eq!(
            pick(&candidates, Some("work"), &[]).as_deref(),
            Some("work")
        );
        assert_eq!(pick(&candidates, None, &[]).as_deref(), Some("default"));
        // Nearly out: move to the roomiest account.
        let candidates = vec![
            ("default".to_string(), status(Some(0.9), QuotaState::Ok)),
            ("work".to_string(), status(Some(0.05), QuotaState::Low)),
        ];
        assert_eq!(
            pick(&candidates, Some("work"), &[]).as_deref(),
            Some("default")
        );
        // Blocked accounts are never picked; unknown quota beats nearly empty.
        let candidates = vec![
            ("default".to_string(), status(None, QuotaState::Blocked)),
            ("work".to_string(), status(Some(0.05), QuotaState::Low)),
            ("home".to_string(), status(None, QuotaState::Unknown)),
        ];
        assert_eq!(pick(&candidates, None, &[]).as_deref(), Some("home"));
        assert_eq!(
            pick(&candidates, None, &["home".to_string()]).as_deref(),
            Some("work")
        );
        let candidates = vec![("default".to_string(), status(None, QuotaState::Blocked))];
        assert_eq!(pick(&candidates, Some("default"), &[]), None);
    }

    #[test]
    fn reset_times_are_read_from_limit_messages() {
        let now = 1_000_000;
        assert_eq!(
            parse_reset("Claude AI usage limit reached|1760000000", now),
            Some(1_760_000_000_000)
        );
        assert_eq!(
            parse_reset("You've hit your usage limit. Try again in 3 hours.", now),
            Some(now + 3 * 3600 * 1000)
        );
        assert_eq!(
            parse_reset("limit reached, try again in 2 days 1 hour 30 minutes", now),
            Some(now + (2 * 86_400 + 3600 + 1800) * 1000)
        );
        assert_eq!(
            parse_reset("429 Too Many Requests: retry after 120 seconds", now),
            Some(now + 120_000)
        );
        assert_eq!(
            parse_reset("Quota resets in 4h 20m", now),
            Some(now + (4 * 3600 + 1200) * 1000)
        );
        assert_eq!(parse_reset("quota exhausted", now), None);
    }

    #[test]
    fn provider_reports_parse() {
        let codex = json!({"rateLimits": {
            "primary": {"usedPercent": 42, "windowDurationMins": 300, "resetsAt": 1_700_000_000},
            "secondary": {"usedPercent": 10, "windowDurationMins": 10080, "resetsAt": null}
        }});
        let parsed = parse_codex_limits(&codex, "work");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].window, "5h");
        assert_eq!(parsed[0].used_percent, 42.0);
        assert_eq!(parsed[0].resets_at_ms, Some(1_700_000_000_000));
        assert_eq!(parsed[1].window, "weekly");

        let claude = json!({
            "five_hour": {"utilization": 80.0, "resets_at": "2025-10-15T08:00:00.000+00:00"},
            "seven_day": {"utilization": 12.5, "resets_at": "2025-10-20T00:00:00Z"},
            "seven_day_opus": null
        });
        let parsed = parse_claude_usage(&claude, DEFAULT);
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0].resets_at_ms,
            parse_time_ms("2025-10-15T08:00:00Z")
        );
        assert_eq!(parsed[1].used_percent, 12.5);

        let agy = json!({"models": {
            "gemini-3.1-pro-high": {"quotaInfo": {"remainingFraction": 0.6, "resetTime": "2025-10-15T10:00:00Z"}},
            "gemini-3.8-flash": {"quotaInfo": {"remainingFraction": 0.9}},
            "claude-opus-4-6-thinking": {"quotaInfo": {"remainingFraction": 0.0, "resetTime": "2025-10-15T12:00:00Z"}},
            "tab_completion": {}
        }});
        let parsed = parse_agy_models(&agy, DEFAULT);
        let gemini = parsed.iter().find(|s| s.pool == "gemini").unwrap();
        assert!((gemini.used_percent - 40.0).abs() < 1e-9);
        let partner = parsed.iter().find(|s| s.pool == "partner").unwrap();
        assert_eq!(partner.used_percent, 100.0);
    }

    #[test]
    fn offsets_shift_parsed_times() {
        assert_eq!(
            parse_time_ms("2025-01-01T07:00:00+07:00"),
            parse_time_ms("2025-01-01T00:00:00Z")
        );
        assert_eq!(
            parse_time_ms("2025-01-01T00:00:00-01:30"),
            parse_time_ms("2025-01-01T01:30:00Z")
        );
    }

    #[test]
    fn account_ids_are_folder_safe() {
        assert!(valid_id("work"));
        assert!(valid_id("team-2"));
        assert!(!valid_id("default"));
        assert!(!valid_id("../x"));
        assert!(!valid_id("Work"));
        assert!(!valid_id(""));
        assert!(account_home("claude-code", "default").is_none());
        assert!(account_home("opencode", "work").is_none());
    }
}
