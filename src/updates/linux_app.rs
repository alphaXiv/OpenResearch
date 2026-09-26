//! Self-update for the Linux AppImage.
//!
//! The AppImage is one file, so an update is: fetch `linux-app.json`, download
//! this architecture's AppImage, check it against the manifest's sha256, and
//! rename it over the old file. The rename leaves the running app on the old
//! file's inode, which its mount keeps reading until the user relaunches.
//!
//! Unlike the macOS app there is no platform signature to check, so the release's
//! sha256 is the whole of the trust, as it is for the CLI's own installer. Only
//! an official build updates itself, so a developer's local AppImage is never
//! silently swapped for a release.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{anyhow, Result};

/// Release asset describing the published AppImages. Written by
/// `.github/workflows/release-linux-app.yml` after both are attached.
const MANIFEST_ASSET: &str = "linux-app.json";

#[derive(Debug, Deserialize)]
pub struct AppManifest {
    pub version: String,
    /// Release tag the assets are pinned to, so the download can't drift to a
    /// different release between the manifest fetch and the download.
    pub tag: String,
    /// Keyed by `std::env::consts::ARCH` (`x86_64`, `aarch64`).
    pub assets: HashMap<String, AppAsset>,
}

#[derive(Debug, Deserialize)]
pub struct AppAsset {
    pub asset: String,
    pub sha256: String,
}

/// Fetches the published app manifest; `Ok(None)` until the AppImages are attached.
pub async fn fetch_manifest(timeout: Duration) -> Result<Option<AppManifest>> {
    super::fetch_app_manifest(MANIFEST_ASSET, timeout).await
}

/// Replace the `.AppImage` at `appimage` with the latest release.
pub async fn update(
    appimage: &Path,
    current: &Version,
    dry_run: bool,
    background: bool,
) -> Result<()> {
    let published = fetch_manifest(Duration::from_secs(10)).await?;
    let latest = published
        .as_ref()
        .map(|manifest| {
            Version::parse(&manifest.version).map_err(|e| {
                anyhow!(
                    "Could not parse the published app version {:?}: {}",
                    manifest.version,
                    e
                )
            })
        })
        .transpose()?;

    // Keep the cache honest even when this install can't apply the update: it is
    // what the dashboard and the outdated warning read.
    if let Some(latest) = &latest {
        super::write_check_cache(&latest.to_string());
    }

    let Some((manifest, latest)) = published
        .zip(latest)
        .filter(|(_, latest)| super::is_outdated(current, latest))
    else {
        if !background {
            println!("OpenResearch {} is up to date.", current);
        }
        return Ok(());
    };

    if dry_run {
        println!(
            "OpenResearch {} → {} is available. Re-run without --dry-run to update.",
            current, latest
        );
        return Ok(());
    }

    let parent = ensure_replaceable(appimage)?;
    let arch = std::env::consts::ARCH;
    let asset = manifest.assets.get(arch).ok_or_else(|| {
        anyhow!(
            "{} has no AppImage for {arch}. Download OpenResearch from {}/releases/latest.",
            manifest.tag,
            super::REPO_URL
        )
    })?;
    if !background {
        eprintln!("Updating OpenResearch {} → {} ...", current, latest);
    }

    let bytes = super::fetch_release_asset(
        &manifest.tag,
        &asset.asset,
        // The AppImage carries WebKitGTK, so it is a large download.
        Duration::from_secs(600),
    )
    .await?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if !digest.eq_ignore_ascii_case(asset.sha256.trim()) {
        return Err(anyhow!(
            "The downloaded {} does not match the checksum published for {} \
             (expected {}, got {}). Nothing was installed.",
            asset.asset,
            manifest.tag,
            asset.sha256.trim(),
            digest
        ));
    }

    // Beside the target, so the final move is a same-filesystem rename.
    let staged = parent.join(format!(
        ".OpenResearch-update-{}.AppImage",
        uuid::Uuid::new_v4()
    ));
    let installed = install_staged(&staged, &bytes, appimage);
    if installed.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    installed?;

    super::record_installed(&latest.to_string());
    if !background {
        println!("✓ Updated OpenResearch {} → {}.", current, latest);
        println!("Restart the app to run the new version.");
    }
    Ok(())
}

/// Refuse the installs where replacing the file is wrong or impossible, each
/// with the action that fixes it. Returns the directory the AppImage is in.
fn ensure_replaceable(appimage: &Path) -> Result<&Path> {
    if crate::telemetry::build_channel() != "production" {
        return Err(anyhow!(
            "This OpenResearch AppImage isn't an official build, so it won't update itself.\n\
             Download the released AppImage from {}/releases/latest.",
            super::REPO_URL
        ));
    }
    let parent = appimage
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", appimage.display()))?;
    super::probe_writable(parent).map_err(|e| {
        anyhow!(
            "Can't write to {} ({e}), so OpenResearch can't update itself.\nMove {} somewhere \
             you can write to, such as ~/Applications.",
            parent.display(),
            appimage.display()
        )
    })?;
    Ok(parent)
}

fn install_staged(staged: &Path, bytes: &[u8], appimage: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(staged, bytes)
        .map_err(|e| anyhow!("Could not write {}: {}", staged.display(), e))?;
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| anyhow!("Could not make {} executable: {}", staged.display(), e))?;
    std::fs::rename(staged, appimage)
        .map_err(|e| anyhow!("Could not replace {}: {}", appimage.display(), e))
}
