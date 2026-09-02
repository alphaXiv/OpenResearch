//! The `update` command — update orx in place by re-running the release
//! installer.
//!
//! The mechanism mirrors what axoupdater (uv's `self update`) does: download
//! the `openresearch-cli-installer.sh` asset from the target release and run
//! it pinned to the existing install prefix via `CARGO_DIST_FORCE_INSTALL_DIR`.
//! The installer owns the hard parts — checksum verification and the atomic
//! rename into `~/.cargo/bin` (never an in-place overwrite, which on macOS
//! trips the kernel's per-inode code-signature cache and SIGKILLs the binary).
//!
//! Guards, in order:
//!   - `OPENRESEARCH_CLI_DISABLE_UPDATE=1` refuses outright (same switch the
//!     installer honors).
//!   - an exclusive file lock, so concurrent `orx update` runs fail fast
//!     instead of corrupting each other.
//!   - `updates::preflight`, which classifies the install and refuses the
//!     channels orx doesn't own (cargo/Homebrew/Nix), the receipt mismatches
//!     that mean another copy is in play, and unwritable bin dirs.
//!
//! Inside the macOS `.app`, preflight resolves to the bundle instead and the
//! whole thing routes to `updates::macos_app` — same command, same guards,
//! different payload.

use std::time::Duration;

use crate::error::{anyhow, Result};
use crate::updates::{self, UpdateTarget};

/// Outcomes are recorded so a repeatedly failing update backs off instead of
/// respawning on every command. A foreground run records too — a user who fixes
/// the cause and updates by hand should not stay stuck behind the backoff.
pub async fn run(args: crate::UpdateArgs) -> Result<()> {
    // Checked before `apply` so a switched-off install never records a failed
    // attempt — nothing was attempted, and the backoff must not be waiting on it
    // if the user turns updates back on.
    if std::env::var("OPENRESEARCH_CLI_DISABLE_UPDATE").as_deref() == Ok("1") {
        return Err(anyhow!(
            "Updates are disabled for this install (OPENRESEARCH_CLI_DISABLE_UPDATE=1)."
        ));
    }
    let (dry_run, background) = (args.dry_run, args.background);
    let result = apply(args).await;
    match &result {
        // A dry run changes nothing, so neither outcome is an attempt.
        _ if dry_run => {}
        // Someone else is mid-update; their outcome is the one that counts, and
        // recording either way here would skew the backoff they are building.
        Ok(Outcome::Contended) => updates::record_contended(),
        Ok(_) => updates::record_attempt(true),
        Err(_) => updates::record_attempt(false),
    }
    match result {
        Ok(Outcome::Contended) if !background => {
            Err(anyhow!("Another `orx update` is already running."))
        }
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

enum Outcome {
    Done,
    /// Another updater holds the lock.
    Contended,
}

async fn apply(args: crate::UpdateArgs) -> Result<Outcome> {
    // One updater per channel at a time: the app and a script-installed CLI
    // replace different things, so serializing them against each other would
    // only have one record a contended attempt and back off for an hour it
    // never spent. flock is advisory and released when the process exits.
    let channel = updates::current_channel()
        .map(updates::InstallChannel::as_str)
        .unwrap_or("unknown");
    let lock_path = crate::config::config_dir().join(format!("update-{channel}.lock"));
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    let mut lock = fd_lock::RwLock::new(lock_file);
    let Ok(_guard) = lock.try_write() else {
        // `run` turns this into the user-facing error for a foreground call; it
        // is an outcome rather than an error here so the backoff stays honest.
        return Ok(Outcome::Contended);
    };

    let current = updates::current_version();
    let target = updates::preflight(args.force)?;

    let receipt = match target {
        UpdateTarget::AppBundle(root) => {
            return updates::macos_app::update(&root, &current, args.dry_run, args.background)
                .await
                .map(|_| Outcome::Done)
        }
        UpdateTarget::Installer(receipt) => receipt,
    };

    let latest = updates::fetch_latest(Duration::from_secs(10)).await?;
    // Record what the release actually is before acting on it, so a cache that
    // was wrong about being behind corrects itself on the next run instead of
    // warning off a stale answer until the check TTL lapses.
    updates::write_check_cache(&latest.version.to_string());
    if !updates::is_outdated(&current, &latest.version) {
        if !args.background {
            println!("orx {} is up to date.", current);
        }
        return Ok(Outcome::Done);
    }

    if args.dry_run {
        println!(
            "orx {} → {} is available. Re-run without --dry-run to update.",
            current, latest.version
        );
        return Ok(Outcome::Done);
    }

    if !args.background {
        eprintln!("Updating orx {} → {} ...", current, latest.version);
    }

    // Pin the installer to the same release the manifest described, so the
    // version we report is exactly the version that gets installed.
    let installer = updates::fetch_release_asset(
        &latest.tag,
        &format!("{}-installer.sh", updates::APP_NAME),
        Duration::from_secs(60),
    )
    .await?;
    let script = std::env::temp_dir().join(format!("orx-installer-{}.sh", uuid::Uuid::new_v4()));
    std::fs::write(&script, &installer)?;

    // `sh <script>` rather than executing the file: immune to noexec /tmp
    // mounts. The installer verifies artifact checksums and renames the new
    // binary into place atomically; replacing a running orx is safe on
    // macOS/Linux (old processes keep the old inode).
    let mut cmd = std::process::Command::new("sh");
    cmd.arg(&script)
        .env("CARGO_DIST_FORCE_INSTALL_DIR", &receipt.install_prefix);
    if !receipt.modify_path {
        cmd.env("OPENRESEARCH_CLI_NO_MODIFY_PATH", "1");
    }
    if args.background {
        // Nobody is watching this child, and its stdio is inherited from a
        // terminal the user is still using.
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    let status = cmd.status();
    let _ = std::fs::remove_file(&script);
    let status = status.map_err(|e| anyhow!("Could not run the installer: {}", e))?;
    if !status.success() {
        return Err(anyhow!(
            "The installer exited with {}. The previous orx is untouched.",
            status
        ));
    }

    // Keep the update-check cache in sync so the warning doesn't fire on a stale
    // answer, and so a running `orx up` learns a restart would pick this up.
    updates::record_installed(&latest.version.to_string());
    if !args.background {
        println!("✓ Updated orx {} → {}.", current, latest.version);
    }
    Ok(Outcome::Done)
}
