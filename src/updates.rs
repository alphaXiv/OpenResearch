//! Self-update plumbing shared by `orx version`, `orx update`, and the
//! outdated-version warning shown on every command.
//!
//! Latest-version discovery deliberately avoids the GitHub REST API: its
//! unauthenticated limit is 60 requests/hour *per IP*, which is routinely
//! exhausted on the datacenter/NAT addresses agents run from. Instead we fetch
//! the `dist-manifest.json` asset that cargo-dist uploads to every release via
//! the documented `releases/latest/download/<asset>` permalink — a plain CDN
//! redirect with no API rate limit.
//!
//! The cargo-dist shell installer writes an install receipt to
//! `${XDG_CONFIG_HOME:-~/.config}/openresearch-cli/openresearch-cli-receipt.json`.
//! That receipt is the only thing distinguishing an installer-managed binary
//! from a `cargo install` one (both live at `~/.cargo/bin/orx` because
//! dist-workspace.toml sets `install-path = "CARGO_HOME"`), so `orx update`
//! refuses to touch the binary unless the receipt matches it.

use std::io::{IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};

pub mod macos_app;

/// GitHub repo the released binaries come from.
pub const REPO_URL: &str = "https://github.com/alphaXiv/OpenResearch";

/// The cargo-dist app name (the *package* name, not the `orx` bin name) — used
/// in release asset names and the receipt path.
pub const APP_NAME: &str = "openresearch-cli";

/// How long a cached update check stays fresh.
const CHECK_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Sent on requests to GitHub — some CDNs reject the default (empty) UA.
const UA: &str = concat!("openresearch-cli/", env!("CARGO_PKG_VERSION"));

pub fn current_version() -> Version {
    // The crate version is always valid semver; a panic here is a build bug.
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver")
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

// ---------------------------------------------------------------------------
// Latest-version discovery (dist-manifest.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LatestRelease {
    pub version: Version,
    /// The git tag (e.g. `v0.1.15`), used to pin asset downloads to the same
    /// release the manifest described.
    pub tag: String,
}

#[derive(Deserialize)]
struct DistManifest {
    announcement_tag: String,
    #[serde(default)]
    releases: Vec<ManifestRelease>,
}

#[derive(Deserialize)]
struct ManifestRelease {
    app_name: String,
    app_version: String,
}

/// Extracts our app's version (and the release tag) from a dist-manifest body.
fn parse_manifest(body: &str) -> Result<LatestRelease> {
    let manifest: DistManifest = serde_json::from_str(body)?;
    let release = manifest
        .releases
        .iter()
        .find(|r| r.app_name == APP_NAME)
        .ok_or_else(|| anyhow!("Release manifest has no entry for {}", APP_NAME))?;
    let version = Version::parse(&release.app_version)
        .map_err(|e| anyhow!("Could not parse version {:?}: {}", release.app_version, e))?;
    Ok(LatestRelease {
        version,
        tag: manifest.announcement_tag,
    })
}

/// Fetches the latest released version from GitHub (rate-limit-free permalink).
pub async fn fetch_latest(timeout: Duration) -> Result<LatestRelease> {
    let url = format!("{}/releases/latest/download/dist-manifest.json", REPO_URL);
    let res = http()
        .get(&url)
        .header("user-agent", UA)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| {
            anyhow!(
                "Could not fetch the release manifest from {}: {}",
                REPO_URL,
                e
            )
        })?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "Release manifest request failed ({} {})",
            status.as_u16(),
            reason
        ));
    }
    let body = res.text().await?;
    parse_manifest(&body)
}

/// Downloads a release asset pinned to `tag` and returns its bytes.
pub async fn fetch_release_asset(tag: &str, asset: &str, timeout: Duration) -> Result<Vec<u8>> {
    let url = format!("{}/releases/download/{}/{}", REPO_URL, tag, asset);
    let res = http()
        .get(&url)
        .header("user-agent", UA)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| anyhow!("Could not download {}: {}", url, e))?;
    let status = res.status();
    if !status.is_success() {
        let reason = status.canonical_reason().unwrap_or("");
        return Err(anyhow!(
            "Download of {} failed ({} {})",
            url,
            status.as_u16(),
            reason
        ));
    }
    Ok(res.bytes().await?.to_vec())
}

// ---------------------------------------------------------------------------
// Install receipt (written by the cargo-dist shell installer)
// ---------------------------------------------------------------------------

/// The fields of the cargo-dist install receipt that `orx update` relies on.
#[derive(Debug, Deserialize)]
pub struct Receipt {
    pub install_prefix: String,
    pub version: String,
    #[serde(default)]
    pub modify_path: bool,
}

pub fn receipt_path() -> PathBuf {
    // Through `shell_env`, like `config::config_dir()`: in macOS app mode the
    // user's real XDG_CONFIG_HOME lives only in this process.
    let base = crate::local::shell_env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        });
    base.join(APP_NAME)
        .join(format!("{}-receipt.json", APP_NAME))
}

/// Reads the install receipt. `Ok(None)` when it does not exist (i.e. the
/// shell installer never ran on this machine); `Err` when it exists but cannot
/// be parsed, since silently treating a corrupt receipt as "not installed by
/// the installer" would point users at the wrong update path.
pub fn load_receipt() -> Result<Option<Receipt>> {
    let path = receipt_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("Could not read {}: {}", path.display(), e)),
    };
    let receipt: Receipt = serde_json::from_str(&raw)
        .map_err(|e| anyhow!("Install receipt at {} is malformed: {}", path.display(), e))?;
    Ok(Some(receipt))
}

/// Whether the running executable lives under the receipt's install prefix.
/// The receipt records the prefix root (e.g. `~/.cargo`), while the binary sits
/// in its `bin/` subdirectory for the cargo-home layout — so a trailing `bin`
/// component is stripped from the exe's directory before comparing. Both paths
/// should be canonicalized by the caller.
pub fn exe_matches_prefix(exe: &Path, prefix: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    let dir = if dir.file_name() == Some(std::ffi::OsStr::new("bin")) {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    };
    dir == prefix
}

// ---------------------------------------------------------------------------
// Install channel
// ---------------------------------------------------------------------------

/// How this orx got onto the machine — which decides whether it may replace
/// itself, and with what. Only the two channels we ship (the installer script
/// and the macOS bundle) can self-update; everything else belongs to a package
/// manager that would be clobbered by writing over its files.
#[derive(Debug)]
pub enum InstallChannel {
    /// Installed by the cargo-dist shell installer, which left a receipt naming
    /// the prefix it owns.
    Installer {
        receipt: Receipt,
        prefix: PathBuf,
    },
    /// The executable inside `OpenResearch.app`; the path is the bundle root.
    AppBundle(PathBuf),
    /// `cargo install` — lands in the same `~/.cargo/bin/orx` as the installer,
    /// so only the absent receipt tells them apart.
    Cargo,
    Homebrew,
    Nix,
    Unknown,
}

impl InstallChannel {
    /// The stable label reported by `orx version --json` and the dashboard.
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallChannel::Installer { .. } => "installer",
            InstallChannel::AppBundle(_) => "app-bundle",
            InstallChannel::Cargo => "cargo",
            InstallChannel::Homebrew => "homebrew",
            InstallChannel::Nix => "nix",
            InstallChannel::Unknown => "unknown",
        }
    }

    /// Whether this channel can apply its own updates at all (before consulting
    /// the user's opt-out).
    pub fn self_updates(&self) -> bool {
        matches!(
            self,
            InstallChannel::Installer { .. } | InstallChannel::AppBundle(_)
        )
    }
}

/// The `.app` root for a bundle executable at `<root>.app/Contents/MacOS/<exe>`.
/// Split out from [`detect_channel`] so it can be tested off macOS.
fn app_bundle_root(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    if !macos.ends_with("Contents/MacOS") {
        return None;
    }
    // `Contents/MacOS` -> `Contents` -> the bundle root.
    macos.parent()?.parent().map(Path::to_path_buf)
}

/// Classify `exe`. The bundle test comes first: the bundled binary has no
/// receipt, so every later branch would misfile it. A malformed receipt is an
/// error rather than "no receipt" for the reason [`load_receipt`] gives — the
/// wrong answer here sends the user down the wrong update path.
pub fn detect_channel(exe: &Path) -> Result<InstallChannel> {
    if let Some(root) = app_bundle_root(exe) {
        return Ok(InstallChannel::AppBundle(root));
    }
    let exe_str = exe.to_string_lossy();
    if exe_str.starts_with("/nix/store/") {
        return Ok(InstallChannel::Nix);
    }
    if exe_str.starts_with("/opt/homebrew/") || exe_str.contains("/Cellar/") {
        return Ok(InstallChannel::Homebrew);
    }
    if let Some(receipt) = load_receipt()? {
        let prefix = PathBuf::from(&receipt.install_prefix);
        let prefix = prefix.canonicalize().unwrap_or(prefix);
        return Ok(InstallChannel::Installer { receipt, prefix });
    }
    if exe.parent().is_some_and(|dir| dir.ends_with(".cargo/bin")) {
        return Ok(InstallChannel::Cargo);
    }
    Ok(InstallChannel::Unknown)
}

/// The running executable, canonicalized. Canonical because
/// [`exe_matches_prefix`] compares it against a canonicalized prefix, and
/// because the bundle's `orx` symlink must resolve to the real executable
/// before its `Contents/MacOS` parent can be recognized.
fn current_exe() -> Result<PathBuf> {
    std::env::current_exe()?
        .canonicalize()
        .map_err(|e| anyhow!("Could not resolve the running executable: {}", e))
}

/// Classify the running binary. Successes are memoized: classification
/// canonicalizes the executable and may read the receipt, and a running process
/// cannot change how it was installed — `orx up` samples this on a timer, so the
/// repeat cost is real. Failures are *not* cached, because the likeliest cause is
/// a receipt caught mid-rewrite by the installer; caching that would strand a
/// long-lived `orx up` on "unknown, never self-updates" until it restarts.
pub fn current_channel() -> Result<&'static InstallChannel> {
    static CHANNEL: OnceLock<InstallChannel> = OnceLock::new();
    if let Some(channel) = CHANNEL.get() {
        return Ok(channel);
    }
    let channel = detect_channel(&current_exe()?)?;
    Ok(CHANNEL.get_or_init(|| channel))
}

/// Whether an update may be applied **without asking** on this install: a
/// self-updating channel, not opted out, and not a corrupt-receipt install
/// (where [`detect_channel`] errors and the manual command should explain why).
///
/// The `autoUpdate` setting is a narrower switch than [`opted_out`]: it stops
/// the silent apply but leaves the check and the outdated warning in place, so
/// a user who wants to choose their moment still learns there is a release.
pub fn auto_update_eligible() -> bool {
    !opted_out()
        && crate::config::auto_update_enabled()
        && current_channel()
            .map(|channel| channel.self_updates())
            .unwrap_or(false)
}

/// The one-liner that reinstalls orx through the release installer.
const INSTALL_HINT: &str = "curl --proto '=https' --tlsv1.2 -LsSf \
https://github.com/alphaXiv/OpenResearch/releases/latest/download/openresearch-cli-installer.sh | sh";

/// Confirm a directory can be written before an update commits to it — root-owned
/// installs and read-only filesystems fail here rather than after a download.
fn probe_writable(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(format!(".orx-update-probe-{}", uuid::Uuid::new_v4()));
    std::fs::File::create(&probe)?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// What `preflight` resolved this install to. The installer variant is checked
/// here; the bundle's checks live in `macos_app::ensure_replaceable`, which runs
/// after the version comparison so `--dry-run` needs no signed, writable app.
pub enum UpdateTarget {
    /// The receipt names the prefix to reinstall into.
    Installer(Receipt),
    /// The `.app` root to swap. macOS only; see the `macos_app` module.
    AppBundle(PathBuf),
}

/// Classify the install and, for the installer channel, check everything that
/// must hold before orx overwrites itself. `force` waives only the two
/// "something else is managing this binary" checks.
pub fn preflight(force: bool) -> Result<UpdateTarget> {
    let exe = current_exe()?;
    let channel = detect_channel(&exe)?;
    let (receipt, prefix) = match channel {
        InstallChannel::AppBundle(root) => return Ok(UpdateTarget::AppBundle(root)),
        InstallChannel::Nix => {
            return Err(anyhow!(
                "This orx is managed by Nix ({}). Update it through your Nix configuration.",
                exe.display()
            ))
        }
        InstallChannel::Homebrew => {
            return Err(anyhow!(
                "This orx looks Homebrew-managed ({}). Update it with `brew upgrade`.",
                exe.display()
            ))
        }
        InstallChannel::Cargo | InstallChannel::Unknown => {
            return Err(anyhow!(
                "orx was not installed by the installer script (no receipt at {}),\n\
                 so `orx update` won't touch it. Update it the way it was installed:\n\
                 - cargo: cargo install --path . (or your original cargo install invocation)\n\
                 - or reinstall with the installer: {}",
                receipt_path().display(),
                INSTALL_HINT
            ))
        }
        InstallChannel::Installer { receipt, prefix } => (receipt, prefix),
    };

    if !exe_matches_prefix(&exe, &prefix) && !force {
        return Err(anyhow!(
            "The running orx is at {} but the installer's receipt says it installed to {}.\n\
             Are multiple copies of orx installed? Pass --force to update the receipt's copy anyway.",
            exe.display(),
            prefix.display()
        ));
    }
    let current = current_version();
    if receipt.version != current.to_string() && !force {
        return Err(anyhow!(
            "The running orx is {} but the install receipt records {} — something other than\n\
             the installer (likely `cargo install`) overwrote {}. Updating would clobber it.\n\
             Pass --force to proceed anyway.",
            current,
            receipt.version,
            exe.display()
        ));
    }

    // Before the download, not after it.
    if let Some(bin_dir) = exe.parent() {
        probe_writable(bin_dir).map_err(|e| {
            anyhow!(
                "No write permission for {} ({}). If orx was installed with sudo,\n\
                 update it the same way or reinstall per-user.",
                bin_dir.display(),
                e
            )
        })?;
    }

    Ok(UpdateTarget::Installer(receipt))
}

// ---------------------------------------------------------------------------
// Update-check cache (the warning's 24h throttle)
// ---------------------------------------------------------------------------

/// Shortest gap between two background update attempts, doubling per consecutive
/// failure up to [`ATTEMPT_BACKOFF_MAX`]. Without this an unreachable GitHub (or
/// a release whose asset 404s) would respawn an updater on *every* invocation —
/// ruinous when an agent drives `orx` in a loop.
const ATTEMPT_BACKOFF_MIN: Duration = Duration::from_secs(60 * 60);
const ATTEMPT_BACKOFF_MAX: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Default, Serialize, Deserialize)]
struct CheckCache {
    /// Unix seconds of the last completed (or attempted) check.
    checked_at: u64,
    /// Latest version seen at that time.
    latest: String,
    /// Unix seconds of the last background update *attempt* (distinct from the
    /// version check above, which is only a fetch).
    #[serde(default)]
    attempted_at: u64,
    /// Consecutive failed attempts, reset on success. Drives the backoff.
    #[serde(default)]
    failures: u32,
    /// Version the updater last installed. Compared against the *running*
    /// version to tell a long-lived process (`orx up`, the app) that the copy on
    /// disk has moved on and a restart would pick it up. It lives in the cache
    /// rather than in memory because the updater is a separate process.
    #[serde(default)]
    installed_version: String,
}

fn cache_path() -> PathBuf {
    // Per channel. The macOS app and a script-installed CLI share a config dir
    // but track different release trains, so one file would have each install
    // reading the other's `latest` (suppressing a real update) and the other's
    // `installed_version` — which is a restart banner for a bundle that was
    // never touched, and the one symptom here that does not self-heal.
    let channel = current_channel()
        .map(InstallChannel::as_str)
        .unwrap_or("unknown");
    crate::config::config_dir().join(format!("update-check-{channel}.json"))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The single cache this feature shipped with, before channels split it. Read
/// once as a fallback so an existing install keeps its `latest` instead of
/// warning off nothing for a cycle; removed on the first write.
fn legacy_cache_path() -> PathBuf {
    crate::config::config_dir().join("update-check.json")
}

fn read_cache() -> Option<CheckCache> {
    let raw = std::fs::read_to_string(cache_path())
        .or_else(|_| std::fs::read_to_string(legacy_cache_path()))
        .ok()?;
    serde_json::from_str(&raw).ok()
}

/// Record the latest known release.
pub fn write_check_cache(latest: &str) {
    mutate_cache(|cache| {
        cache.checked_at = now_unix();
        cache.latest = latest.to_string();
    });
}

/// Record the outcome of an update attempt: success clears the backoff, failure
/// lengthens it.
pub fn record_attempt(succeeded: bool) {
    mutate_cache(|cache| {
        cache.attempted_at = now_unix();
        cache.failures = if succeeded {
            0
        } else {
            cache.failures.saturating_add(1)
        };
    });
}

/// Note that an updater was already running, damping the spawn storm without
/// touching `failures`. Recording this as a *success* would reset the counter
/// the real updater is about to increment — and losing that race is most likely
/// precisely when the holder is slow because it is failing.
pub fn record_contended() {
    mutate_cache(|cache| cache.attempted_at = now_unix());
}

/// Record a version as installed on disk. Also refreshes the check fields — an
/// install is the freshest possible answer to "what is the latest release".
pub fn record_installed(version: &str) {
    mutate_cache(|cache| {
        cache.checked_at = now_unix();
        cache.latest = version.to_string();
        cache.installed_version = version.to_string();
    });
}

/// How long to wait after `failures` consecutive failures before trying again.
/// The shift is clamped so a runaway counter can't overflow it.
fn attempt_backoff(failures: u32) -> Duration {
    ATTEMPT_BACKOFF_MIN
        .saturating_mul(1 << failures.min(31))
        .min(ATTEMPT_BACKOFF_MAX)
}

/// Whether enough time has passed since the last background update attempt.
/// A cache with no recorded attempt (fresh install, or the first run after this
/// feature shipped) is due immediately.
fn attempt_due(cache: Option<&CheckCache>) -> bool {
    let Some(cache) = cache else {
        return true;
    };
    now_unix().saturating_sub(cache.attempted_at) >= attempt_backoff(cache.failures).as_secs()
}

/// Hand the update to a detached `orx update --background`.
///
/// A separate process, not an in-process task: the update replaces this very
/// binary and must outlive a command that exits in milliseconds.
///
/// Failures here are silent by design: this is a best-effort background nicety,
/// and `orx update` remains the path that explains what went wrong.
fn spawn_background_update() {
    let Ok(mut cmd) = updater_command() else {
        return;
    };
    // Stdio is null because the parent's terminal belongs to the command the
    // user actually ran.
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(mut child) = cmd.spawn() {
        // Reap it if we outlive it (`orx up` does); if the runtime is torn down
        // first the child is simply reparented and keeps going.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

/// The `orx update --background` invocation both spawn sites use.
///
/// The environment is the load-bearing part: in macOS app mode the user's real
/// `XDG_CONFIG_HOME`/`ORX_DATA_DIR` live only in this process (`shell_env::set`
/// deliberately avoids `env::set_var`), so a child left to inherit launchd's
/// environment would take its lock and write its cache under a *different*
/// config dir than the parent reads — losing the restart signal and the backoff,
/// and failing to serialize against a terminal `orx update`.
fn updater_command() -> Result<tokio::process::Command> {
    let mut cmd = tokio::process::Command::new(std::env::current_exe()?);
    cmd.args(["update", "--background"])
        .stdin(std::process::Stdio::null());
    if let Some(path) = crate::local::shell_env::search_path() {
        cmd.env("PATH", path);
    }
    crate::local::shell_env::export_to(|key, value| {
        cmd.env(key, value);
    });
    // Own process group, so a Ctrl-C — or an agent killing the group it spawned
    // `orx` in — can't land between the two renames that swap the app bundle.
    #[cfg(unix)]
    cmd.process_group(0);
    Ok(cmd)
}

/// Read-modify-write the cache, preserving fields the caller didn't touch: the
/// version check and the attempt bookkeeping are written by different code
/// paths, so a whole-object write from either would erase the other's.
///
/// Errors are swallowed — the cache only throttles a best-effort check. The
/// rename is what makes the *file* safe to read concurrently (never torn, always
/// the old or a complete new one); the read-modify-write around it is unlocked,
/// so two processes racing can still lose one side's fields. Both writers
/// re-run often enough that a lost update costs at most one cycle, which is
/// cheaper than holding a lock on every `orx` invocation.
fn mutate_cache(f: impl FnOnce(&mut CheckCache)) {
    let mut cache = read_cache().unwrap_or_default();
    f(&mut cache);
    let path = cache_path();
    let Some(parent) = path.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(parent);
    let Ok(body) = serde_json::to_string(&cache) else {
        return;
    };
    let tmp = parent.join(format!(".update-check.{}.tmp", uuid::Uuid::new_v4()));
    if std::fs::write(&tmp, body).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, &path).is_err() {
        // Rename failed (e.g. cross-device, racing cleanup); don't leak the temp.
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    // Migrated: this channel now has its own file.
    let _ = std::fs::remove_file(legacy_cache_path());
}

// ---------------------------------------------------------------------------
// Status, for long-lived processes (`orx up` and the macOS app)
// ---------------------------------------------------------------------------

/// How often a long-lived process re-checks. Short enough that a day-long
/// session still lands the update, long enough to be invisible.
pub const PERIODIC_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// What the dashboard shows about updates. Derived entirely from the cache and
/// the install channel, so it is the same answer whether the updater ran in this
/// process or in a detached one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current: String,
    /// Latest release this install can actually move to. `None` before the first
    /// check.
    pub latest: Option<String>,
    /// How orx was installed — see [`InstallChannel::as_str`].
    pub channel: &'static str,
    /// Whether this install can update itself at all.
    pub self_updates: bool,
    /// Whether an update would be applied automatically — the setting *and* the
    /// env opt-outs, so this never promises something that will never run.
    pub auto_update: bool,
    /// The environment, not the user's setting, is what forced `auto_update`
    /// off. Kept separate so the UI only blames `ORX_NO_UPDATE_CHECK` when it is
    /// actually the reason.
    pub env_disabled: bool,
    pub update_available: bool,
    /// The version already on disk when it is newer than the running one: only a
    /// restart of *this* process is missing. Named separately from `latest`
    /// because a release can land between the install and the restart.
    pub installed_version: Option<String>,
    pub restart_required: bool,
    /// Whether this platform supports `POST /api/update/restart` (see
    /// [`relaunch`]); pair with `restart_required`. Reported so the dashboard
    /// only offers a button the server will honor.
    pub can_restart: bool,
    /// Identifies this server process, so a client can tell a relaunched server
    /// from the one it was talking to even when both report the same version.
    pub instance: &'static str,
}

pub fn status() -> UpdateStatus {
    let current = current_version();
    let cache = read_cache();
    let channel = current_channel().ok();
    let latest = cache.as_ref().and_then(|c| Version::parse(&c.latest).ok());
    let installed = cache
        .as_ref()
        .and_then(|c| Version::parse(&c.installed_version).ok())
        .filter(|installed| is_outdated(&current, installed));
    UpdateStatus {
        update_available: latest
            .as_ref()
            .is_some_and(|latest| is_outdated(&current, latest)),
        restart_required: installed.is_some(),
        can_restart: cfg!(unix),
        instance: instance_id(),
        installed_version: installed.map(|v| v.to_string()),
        latest: latest.map(|v| v.to_string()),
        channel: channel.map(InstallChannel::as_str).unwrap_or("unknown"),
        self_updates: channel.map(InstallChannel::self_updates).unwrap_or(false),
        auto_update: !opted_out() && crate::config::auto_update_enabled(),
        env_disabled: opted_out(),
        current: current.to_string(),
    }
}

fn instance_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// Environment the app-bundle relaunch hands to the new app: the port the old
/// one served on, so the dashboard tab that asked for the restart reconnects to
/// the same origin instead of timing out against a fresh ephemeral port.
pub const APP_RELAUNCH_PORT_ENV: &str = "ORX_APP_RELAUNCH_PORT";

/// Relaunch this process into the copy on disk. Returns only on failure.
///
/// A terminal `orx up` execs itself: same PID, same terminal, same arguments
/// (plus `--no-browser`), so whatever started it (a shell, a supervisor, an SSH launcher's tunnel)
/// sees an uninterrupted process. The macOS app cannot be exec'd — AppKit and
/// LaunchServices track the launch, not the image — so it exits and leaves a
/// detached shell to `open` the bundle once the old process is gone.
///
/// `port` is what the dashboard is served on; the relaunch keeps it, and skips
/// opening a browser, because the tab that asked is reloading itself.
#[cfg(unix)]
pub fn relaunch(port: u16) -> std::io::Error {
    // Same test as `main`: the bundle exe run from a terminal with arguments is
    // a CLI and execs like one.
    #[cfg(target_os = "macos")]
    if crate::commands::app::launched_as_app_bundle() && std::env::args_os().len() == 1 {
        match current_exe().ok().and_then(|exe| app_bundle_root(&exe)) {
            Some(root) => return relaunch_app_bundle(&root, port),
            None => return std::io::Error::other("could not locate the app bundle to relaunch"),
        }
    }

    // Not the canonical helper: the launch path is what the installer swapped
    // under, and canonicalizing a replaced binary would pin the old inode.
    let Ok(exe) = std::env::current_exe() else {
        return std::io::Error::other("could not resolve the running executable");
    };
    std::process::Command::new(relaunch_target(exe))
        .args(relaunch_args(std::env::args_os().skip(1)))
        .exec()
}

#[cfg(not(unix))]
pub fn relaunch(_port: u16) -> std::io::Error {
    std::io::Error::from(std::io::ErrorKind::Unsupported)
}

/// Linux reports a replaced binary as `<path> (deleted)`; the installer put the
/// new file at `<path>`, which is what to exec.
fn relaunch_target(exe: PathBuf) -> PathBuf {
    exe.to_str()
        .and_then(|exe| exe.strip_suffix(" (deleted)"))
        .map(PathBuf::from)
        .unwrap_or(exe)
}

/// The original arguments plus `--no-browser`: the tab that asked for the
/// restart reloads itself, so a second tab would only be clutter.
fn relaunch_args(args: impl Iterator<Item = std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = args.collect();
    if !args.iter().any(|arg| arg == "--no-browser") {
        args.push("--no-browser".into());
    }
    args
}

#[cfg(target_os = "macos")]
fn relaunch_app_bundle(root: &Path, port: u16) -> std::io::Error {
    let spawned = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.1; done; exec open -n --env "$3" "$2""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(root)
        .arg(format!("{APP_RELAUNCH_PORT_ENV}={port}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Its own group, so it outlives this process and whatever signals it.
        .process_group(0)
        .spawn();
    match spawned {
        Ok(_) => std::process::exit(0),
        Err(err) => err,
    }
}

/// The newest version *this* install can move to.
///
/// The channel decides which manifest answers, and getting this wrong is
/// user-visible: the macOS app installs from `macos-app.json`, which is attached
/// by a separate workflow after the release and legitimately lags (or 404s)
/// behind the CLI's `dist-manifest.json`. Reporting the CLI's version to an app
/// install would advertise — and endlessly re-attempt — a build that does not
/// exist for it. `Ok(None)` means "nothing newer published for this channel".
pub async fn fetch_latest_for_channel(timeout: Duration) -> Result<Option<Version>> {
    if matches!(current_channel(), Ok(InstallChannel::AppBundle(_))) {
        return Ok(macos_app::fetch_manifest(timeout)
            .await?
            .map(|manifest| Version::parse(&manifest.version))
            .transpose()?);
    }
    Ok(Some(fetch_latest(timeout).await?.version))
}

/// Apply an update right now, on request, ignoring the backoff — a person
/// asking is a better signal than the timer. Waits for the updater so the caller
/// can report the outcome, unlike the fire-and-forget background path.
pub async fn apply_now() -> Result<()> {
    if opted_out() {
        return Err(anyhow!(
            "Updates are switched off for this install (ORX_NO_UPDATE_CHECK / \
             OPENRESEARCH_CLI_DISABLE_UPDATE)."
        ));
    }
    let out = tokio::time::timeout(APPLY_NOW_TIMEOUT, updater_command()?.output())
        .await
        .map_err(|_| {
            anyhow!(
                "The update is still running after {} minutes. It will finish in the background; \
                 reopen Settings to see the result.",
                APPLY_NOW_TIMEOUT.as_secs() / 60
            )
        })?
        .map_err(|e| anyhow!("Could not start the updater: {}", e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr.trim();
        return Err(anyhow!(
            "The update failed{}",
            if detail.is_empty() {
                ".".to_string()
            } else {
                format!(": {detail}")
            }
        ));
    }
    Ok(())
}

/// How long a user-initiated update may hold its HTTP request open. Generous
/// enough for a universal app bundle on a slow link, bounded so the dashboard
/// request can't hang indefinitely — the updater keeps running past it either
/// way, and the next status sample reports the result.
const APPLY_NOW_TIMEOUT: Duration = Duration::from_secs(300);

/// One update pass for a process that outlives the invocation-time check in
/// [`UpdateWarning::start`] — `orx up` and the macOS app can run for days, so
/// they poll instead.
///
/// The check is refreshed even when auto-update is off, so the dashboard can
/// still tell the user a release exists. The apply itself goes through the same
/// detached `orx update --background` as the CLI: it takes the updater's file
/// lock, records its own outcome, and — inside the bundle — routes to the app
/// updater. Nothing here needs to know which.
pub async fn periodic_update_pass() {
    if opted_out() {
        return;
    }
    if let Ok(Some(latest)) = fetch_latest_for_channel(Duration::from_secs(10)).await {
        write_check_cache(&latest.to_string());
    }
    if !auto_update_eligible() {
        return;
    }
    let cache = read_cache();
    let outdated = cache
        .as_ref()
        .and_then(|c| Version::parse(&c.latest).ok())
        .is_some_and(|latest| is_outdated(&current_version(), &latest));
    if outdated && attempt_due(cache.as_ref()) {
        spawn_background_update();
    }
}

// ---------------------------------------------------------------------------
// Outdated-version warning
// ---------------------------------------------------------------------------

/// The leading label every warning message starts with. Shared so the builder
/// (`warning_for`) and the styler (`render`, which bolds just this label) can't
/// silently drift apart on a copy edit.
const WARNING_LABEL: &str = "Warning:";

/// Whether the user has explicitly silenced the update check. When true, no
/// warning is shown and no background refresh runs — the one escape hatch for
/// anyone who can't tolerate the extra stderr line (including CI).
fn opted_out() -> bool {
    std::env::var_os("ORX_NO_UPDATE_CHECK").is_some()
        // The generic convention honored by update-notifier and friends.
        || std::env::var_os("NO_UPDATE_NOTIFIER").is_some()
        // cargo-dist's own "don't manage updates for this install" switch; the
        // installer already honors it, so it is the one mental model.
        || std::env::var("OPENRESEARCH_CLI_DISABLE_UPDATE").as_deref() == Ok("1")
}

/// Whether to emit ANSI styling on stderr: only when stderr is a real terminal
/// and the user hasn't set the conventional `NO_COLOR` opt-out. Pipes, files,
/// and CI logs get plain text — never raw escape codes — which matters because
/// this warning now prints on non-interactive runs too.
fn stderr_supports_ansi() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}

/// Wraps `text` in the ANSI bold sequence when `enabled`, else returns it as-is.
/// `\x1b[22m` resets bold/faint specifically (not `\x1b[0m`, which would also
/// clear any surrounding styling the terminal had).
fn bold(text: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[1m{text}\x1b[22m")
    } else {
        text.to_string()
    }
}

/// Renders the warning for printing to stderr: bolds the leading `Warning:`
/// label when `ansi` is set, and sets the whole thing off in its own block with
/// a blank line above and below. Returns the exact bytes to write (the caller
/// uses `write!`, not `writeln!`, since the trailing newlines are included).
fn render(message: &str, ansi: bool) -> String {
    // The message always begins with WARNING_LABEL (see `warning_for`); bold just
    // that label, gh/cargo-style, rather than the whole sentence. If the prefix
    // ever drifts, fall back to the unstyled message — never panic, never emit a
    // stray escape.
    let styled = match message.strip_prefix(WARNING_LABEL) {
        Some(rest) => format!("{}{rest}", bold(WARNING_LABEL, ansi)),
        None => message.to_string(),
    };
    format!("\n{styled}\n\n")
}

/// Version-precedence key: everything that orders two releases *except* build
/// metadata. Per the SemVer spec build metadata is not part of precedence
/// (`1.0.0+a` and `1.0.0+b` are the same release), but `semver::Version`'s `Ord`
/// compares it anyway — so two builds of the same release would otherwise read
/// as "outdated". Comparing this key instead avoids that false positive.
/// `Prerelease`'s own `Ord` already encodes the spec rule that a release sorts
/// above its pre-releases (empty pre-release is the greatest).
fn precedence(v: &Version) -> (u64, u64, u64, &semver::Prerelease) {
    (v.major, v.minor, v.patch, &v.pre)
}

/// Whether `latest` outranks `current` in [`precedence`] — false when `current`
/// is already current, or ahead (a local dev build, or the same release with
/// different build metadata).
pub(crate) fn is_outdated(current: &Version, latest: &Version) -> bool {
    precedence(latest) > precedence(current)
}

/// Builds the outdated-version warning, or `None` when `current` is not behind.
///
/// Because orx talks to a versioned backend, a stale client can hit removed or
/// changed API shapes, so the warning notes the compatibility risk rather than a
/// neutral "new version available".
///
/// When the auto-updater is going to handle it, the message says so instead of
/// asking for a command the user doesn't need to run: the update is already
/// downloading and the *next* invocation will be current. Installs orx doesn't
/// own (cargo/Homebrew/Nix), and anyone who turned auto-update off, keep the
/// manual instruction.
fn warning_for(current: &Version, latest: &Version, orx: &str, automatic: bool) -> Option<String> {
    if !is_outdated(current, latest) {
        return None;
    }
    let remedy = if automatic {
        format!("orx is updating to {latest} in the background; your next run will use it.")
    } else {
        format!("Run `{orx} update` to upgrade.")
    };
    Some(format!(
        "{WARNING_LABEL} orx {current} is outdated (latest {latest}). A newer release is \
         available; upgrade to stay compatible with the API. {remedy}"
    ))
}

/// The outdated-version warning, modeled on the gh CLI / update-notifier
/// pattern: the message shown this run comes from the *cached* previous check,
/// so it is instant and never adds latency to the command. A background refresh
/// updates the cache for the next run at most once per [`CHECK_TTL`].
///
/// Unlike a quiet "new version" nudge, the warning is shown on every command —
/// piped, scripted, or interactive — and is printed in [`start`](Self::start),
/// *before* the command runs, so it survives commands that `std::process::exit`
/// on their own (e.g. the "not logged in" path) instead of returning to `main`.
/// [`opted_out`] is the single switch to silence it.
///
/// Because the message comes from cache, the *first* run after a fresh install
/// has nothing cached yet and shows nothing; the background refresh it kicks off
/// warms the cache so a later run can warn. The refresh is always fire-and-forget
/// — it never blocks the command — so a one-shot environment that throws its
/// cache away each run (e.g. an ephemeral CI container) may take a few runs to
/// warm up, or never. That is the accepted cost of adding zero latency.
pub struct UpdateWarning {
    refresh: Option<tokio::task::JoinHandle<()>>,
}

impl UpdateWarning {
    /// Prints the cached warning (if any) to stderr immediately, hands a pending
    /// update to a detached updater, then kicks off a best-effort background
    /// refresh of the cached "latest" for next time. Printing here — rather than
    /// after the command — is what guarantees the warning shows even when the
    /// command exits the process itself.
    pub fn start() -> UpdateWarning {
        if opted_out() {
            return UpdateWarning { refresh: None };
        }

        let current = current_version();
        let cache = read_cache();
        let cached_latest = cache.as_ref().and_then(|c| Version::parse(&c.latest).ok());
        // Resolved once: it reads settings.json and canonicalizes the exe, and
        // both the message and the spawn decision below depend on it.
        let automatic = cached_latest
            .as_ref()
            .is_some_and(|latest| is_outdated(&current, latest))
            && auto_update_eligible();

        if let Some(message) = cached_latest
            .as_ref()
            .and_then(|latest| warning_for(&current, latest, crate::invocation::orx(), automatic))
        {
            // Infallible: a closed/broken stderr (e.g. `2>&-`, or a reader that
            // already exited) must not panic the process before the command even
            // runs. `eprintln!` would; a swallowed `writeln!` won't.
            let _ = write!(
                std::io::stderr(),
                "{}",
                render(&message, stderr_supports_ansi())
            );
        }

        if automatic && attempt_due(cache.as_ref()) {
            spawn_background_update();
        }

        // Refresh whenever the cache is stale (or absent), regardless of whether
        // stderr is a terminal: scripted/piped runs warn too, so their cache has
        // to stay fresh or they'd warn off a frozen answer indefinitely.
        let stale = cache
            .as_ref()
            .map(|c| now_unix().saturating_sub(c.checked_at) >= CHECK_TTL.as_secs())
            .unwrap_or(true);
        if !stale {
            return UpdateWarning { refresh: None };
        }

        let prev_latest = cache.map(|c| c.latest);
        let handle = tokio::spawn(async move {
            let latest = fetch_latest_for_channel(Duration::from_secs(3))
                .await
                .ok()
                .flatten()
                .map(|v| v.to_string());
            // On fetch failure, refresh checked_at with the old answer so errors
            // don't cause a retry on every invocation. Only fabricate `current`
            // as a last resort (no cache, no previous answer): a one-off failed
            // fetch then suppresses the warning until the TTL lapses, which is
            // the price of not re-fetching on every offline invocation.
            let value = latest
                .or(prev_latest)
                .unwrap_or_else(|| current.to_string());
            write_check_cache(&value);
        });

        UpdateWarning {
            refresh: Some(handle),
        }
    }

    /// Called after the real command finished. On an interactive terminal, grants
    /// the fire-and-forget refresh a brief grace window to land in the cache — it
    /// has often finished during the command's own work — so the cache is warm for
    /// the user's *next* command; the window gives the write a chance to commit
    /// before `#[tokio::main]` tears the runtime down, without which quick commands
    /// would never warm the cache.
    ///
    /// On a non-terminal (pipe, CI, cron) the wait is skipped entirely: an
    /// ephemeral environment throws its cache away between runs, so warming it
    /// buys nothing, and a per-invocation 250ms tail across a CI pipeline that
    /// calls `orx` many times is pure cost. The refresh is still spawned (it may
    /// land if the process outlives it); we just don't pay to wait for it. Never
    /// blocks the command meaningfully, never touches stdout or the exit code.
    pub async fn finish(self) {
        let Some(handle) = self.refresh else {
            return;
        };
        if !std::io::stderr().is_terminal() {
            return;
        }
        // Timed out -> the task keeps running (it may still land); the next stale
        // run tries again regardless.
        let _ = tokio::time::timeout(Duration::from_millis(250), handle).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        app_bundle_root, attempt_backoff, attempt_due, bold, detect_channel, exe_matches_prefix,
        now_unix, parse_manifest, precedence, relaunch_args, relaunch_target, render, warning_for,
        CheckCache, InstallChannel, ATTEMPT_BACKOFF_MAX, ATTEMPT_BACKOFF_MIN,
    };
    use semver::Version;
    use std::ffi::OsString;
    use std::path::Path;
    use std::path::PathBuf;

    #[test]
    fn render_sets_off_the_warning_in_its_own_block() {
        let msg = "Warning: orx 0.1.0 is outdated (latest 0.2.0). foo Run `orx update` to upgrade.";
        // Plain (non-TTY): blank line before and after, no escape codes at all.
        let plain = render(msg, false);
        assert_eq!(plain, format!("\n{msg}\n\n"));
        assert!(
            !plain.contains('\x1b'),
            "plain output must not contain ANSI"
        );

        // Styled (TTY): only the "Warning:" label is bolded; same blank-line block.
        let styled = render(msg, true);
        assert!(styled.starts_with("\n\x1b[1mWarning:\x1b[22m"));
        assert!(styled.ends_with("upgrade.\n\n"));
        // Everything after the label is unstyled — exactly one bold open/close.
        assert_eq!(styled.matches("\x1b[1m").count(), 1);
        assert_eq!(styled.matches("\x1b[22m").count(), 1);

        // Fail-soft: a message without the "Warning:" prefix is passed through
        // unstyled (no panic, no stray escapes), so the label/builder coupling
        // degrades gracefully if the prefix ever changes.
        let no_label = render("orx is outdated.", true);
        assert_eq!(no_label, "\norx is outdated.\n\n");
    }

    #[test]
    fn bold_is_a_noop_when_disabled() {
        assert_eq!(bold("x", false), "x");
        assert_eq!(bold("x", true), "\x1b[1mx\x1b[22m");
    }

    #[test]
    fn precedence_ignores_build_metadata_and_orders_prereleases() {
        let v = |s: &str| Version::parse(s).unwrap();
        // Build metadata is not part of precedence: same release.
        assert_eq!(precedence(&v("1.2.3+a")), precedence(&v("1.2.3+b")));
        assert_eq!(precedence(&v("1.2.3")), precedence(&v("1.2.3+build.99")));
        // A release outranks its pre-releases (empty pre-release is greatest).
        assert!(precedence(&v("0.2.0")) > precedence(&v("0.2.0-rc.1")));
        assert!(precedence(&v("0.2.0-rc.2")) > precedence(&v("0.2.0-rc.1")));
        // Ordinary ordering on the numeric fields still holds.
        assert!(precedence(&v("0.2.0")) > precedence(&v("0.1.29")));
    }

    // Every outdated case shows the same message; assert its stable markers
    // without pinning the exact copy.
    fn assert_warns(msg: &str) {
        assert!(msg.contains("outdated"), "{msg}");
        assert!(msg.contains("stay compatible with the API"), "{msg}");
        assert!(msg.contains("orx update"), "{msg}");
    }

    #[test]
    fn no_warning_when_current_or_ahead() {
        let v = |s: &str| Version::parse(s).unwrap();
        // Exactly current.
        assert!(warning_for(&v("0.1.29"), &v("0.1.29"), "orx", false).is_none());
        // Local build ahead of the latest release.
        assert!(warning_for(&v("0.2.0"), &v("0.1.29"), "orx", false).is_none());
        // Build metadata is ignored by semver ordering, so it's not "outdated".
        assert!(warning_for(&v("0.1.29"), &v("0.1.29+build.5"), "orx", false).is_none());
        // Running ahead of the latest stable on a local pre-release: not outdated.
        assert!(warning_for(&v("0.3.0-dev.1"), &v("0.2.0"), "orx", false).is_none());
    }

    #[test]
    fn warns_on_every_kind_of_outdated_gap() {
        let v = |s: &str| Version::parse(s).unwrap();
        // The same warning fires regardless of how far behind: patch, minor,
        // major, 0.0.x, post-1.0, and a pre-release behind its final release.
        for (cur, latest) in [
            ("0.1.28", "0.1.29"),     // pre-1.0 patch
            ("0.1.29", "0.2.0"),      // pre-1.0 minor
            ("0.0.1", "0.0.2"),       // 0.0.x patch
            ("0.9.0", "1.0.0"),       // 0.x -> 1.0
            ("1.2.3", "1.2.4"),       // post-1.0 patch
            ("1.2.3", "1.3.0"),       // post-1.0 minor
            ("1.4.0", "2.0.0"),       // major
            ("0.2.0-rc.1", "0.2.0"),  // pre-release behind its final release
            ("0.1.29", "0.2.0-rc.1"), // behind the next minor's pre-release
        ] {
            assert_warns(
                &warning_for(&v(cur), &v(latest), "orx", false).unwrap_or_else(|| {
                    panic!("expected a warning for {cur} -> {latest}");
                }),
            );
        }
    }

    #[test]
    fn an_automatic_update_is_reported_not_prescribed() {
        let v = |s: &str| Version::parse(s).unwrap();
        // Auto-updating installs are told what is happening, not what to run:
        // the command would be busywork for something already underway.
        let auto = warning_for(&v("0.1.28"), &v("0.1.29"), "orx", true).unwrap();
        assert!(auto.contains("outdated"), "{auto}");
        assert!(auto.contains("in the background"), "{auto}");
        assert!(!auto.contains("Run `orx update`"), "{auto}");
        // The manual form is unchanged for the channels orx doesn't own.
        assert_warns(&warning_for(&v("0.1.28"), &v("0.1.29"), "orx", false).unwrap());
    }

    #[test]
    fn attempt_backoff_grows_then_caps() {
        assert_eq!(attempt_backoff(0), ATTEMPT_BACKOFF_MIN);
        assert_eq!(attempt_backoff(1), ATTEMPT_BACKOFF_MIN * 2);
        assert_eq!(attempt_backoff(3), ATTEMPT_BACKOFF_MIN * 8);
        // Capped, and never panics on a failure count that would overflow the
        // shift.
        assert_eq!(attempt_backoff(20), ATTEMPT_BACKOFF_MAX);
        assert_eq!(attempt_backoff(u32::MAX), ATTEMPT_BACKOFF_MAX);
    }

    #[test]
    fn an_attempt_is_due_until_one_is_recorded() {
        // No cache at all (fresh install): due immediately.
        assert!(attempt_due(None));

        let cache = |attempted_at, failures| CheckCache {
            attempted_at,
            failures,
            ..Default::default()
        };
        // Just attempted: not due, whether it succeeded or failed.
        assert!(!attempt_due(Some(&cache(now_unix(), 0))));
        assert!(!attempt_due(Some(&cache(now_unix(), 3))));
        // A clean cache that has never attempted (attempted_at 0) is due.
        assert!(attempt_due(Some(&cache(0, 0))));
        // Past the window for its failure count.
        let long_ago = now_unix() - ATTEMPT_BACKOFF_MIN.as_secs() - 1;
        assert!(attempt_due(Some(&cache(long_ago, 0))));
        // ...but not past the doubled window after two failures.
        assert!(!attempt_due(Some(&cache(long_ago, 2))));
    }

    #[test]
    fn channels_that_orx_does_not_own_never_self_update() {
        let channel = |exe: &str| detect_channel(Path::new(exe)).unwrap();
        // The bundle test wins over everything else: the bundled binary has no
        // receipt, so any later branch would misfile it.
        let bundle = channel("/Applications/OpenResearch.app/Contents/MacOS/OpenResearch");
        assert_eq!(bundle.as_str(), "app-bundle");
        assert!(bundle.self_updates());
        assert!(matches!(
            bundle,
            InstallChannel::AppBundle(root) if root == Path::new("/Applications/OpenResearch.app")
        ));

        for (exe, want) in [
            ("/nix/store/abc123-orx/bin/orx", "nix"),
            ("/opt/homebrew/bin/orx", "homebrew"),
            ("/usr/local/Cellar/orx/0.1.0/bin/orx", "homebrew"),
        ] {
            let channel = channel(exe);
            assert_eq!(channel.as_str(), want, "{exe}");
            assert!(!channel.self_updates(), "{exe}");
        }
    }

    #[test]
    fn a_bundle_root_is_the_directory_above_contents() {
        assert_eq!(
            app_bundle_root(Path::new(
                "/Applications/OpenResearch.app/Contents/MacOS/orx"
            )),
            Some(PathBuf::from("/Applications/OpenResearch.app"))
        );
        // Anything not laid out as a bundle executable.
        assert_eq!(app_bundle_root(Path::new("/usr/local/bin/orx")), None);
        assert_eq!(app_bundle_root(Path::new("/a/Contents/orx")), None);
    }

    #[test]
    fn relaunch_target_strips_the_deleted_marker() {
        assert_eq!(
            relaunch_target(PathBuf::from("/x/orx (deleted)")),
            PathBuf::from("/x/orx")
        );
        assert_eq!(
            relaunch_target(PathBuf::from("/x/orx")),
            PathBuf::from("/x/orx")
        );
    }

    #[test]
    fn relaunch_args_add_no_browser_once() {
        let args = |list: &[&str]| relaunch_args(list.iter().map(OsString::from));
        assert_eq!(
            args(&["up", "--port", "1"]),
            ["up", "--port", "1", "--no-browser"]
        );
        assert_eq!(args(&["up", "--no-browser"]), ["up", "--no-browser"]);
    }

    #[test]
    fn parses_dist_manifest() {
        let body = r#"{
            "dist_version": "0.32.0",
            "announcement_tag": "v0.1.15",
            "announcement_is_prerelease": false,
            "releases": [{
                "app_name": "openresearch-cli",
                "app_version": "0.1.15",
                "artifacts": ["openresearch-cli-installer.sh"]
            }]
        }"#;
        let latest = parse_manifest(body).unwrap();
        assert_eq!(latest.tag, "v0.1.15");
        assert_eq!(latest.version, Version::new(0, 1, 15));
    }

    #[test]
    fn manifest_without_our_app_is_an_error() {
        let body = r#"{"announcement_tag": "v1.0.0", "releases": [{"app_name": "other", "app_version": "1.0.0"}]}"#;
        assert!(parse_manifest(body).is_err());
    }

    #[test]
    fn semver_ordering_not_lexicographic() {
        // The case string comparison gets wrong.
        assert!(Version::parse("0.1.15").unwrap() > Version::parse("0.1.9").unwrap());
    }

    #[test]
    fn exe_prefix_matching() {
        let cases = [
            // cargo-home layout: receipt prefix is the root, binary in bin/.
            ("/home/u/.cargo/bin/orx", "/home/u/.cargo", true),
            // flat layout: binary directly in the prefix.
            ("/opt/orx/orx", "/opt/orx", true),
            // foreign binary elsewhere on PATH.
            ("/usr/local/bin/orx", "/home/u/.cargo", false),
        ];
        for (exe, prefix, want) in cases {
            assert_eq!(
                exe_matches_prefix(Path::new(exe), Path::new(prefix)),
                want,
                "exe={} prefix={}",
                exe,
                prefix
            );
        }
    }
}
