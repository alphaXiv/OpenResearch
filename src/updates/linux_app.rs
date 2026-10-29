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

use super::Outcome;
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
    #[serde(default)]
    pub ancestors: Vec<String>,
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
    if crate::telemetry::update_channel() == super::ReleaseChannel::Canary {
        return Ok(super::fetch_canary_manifest(timeout)
            .await?
            .and_then(|manifest| {
                let assets: HashMap<_, _> = manifest
                    .assets
                    .into_iter()
                    .filter_map(|(key, asset)| {
                        key.strip_prefix("linux-").map(|arch| {
                            (
                                arch.to_string(),
                                AppAsset {
                                    asset: asset.asset,
                                    sha256: asset.sha256,
                                },
                            )
                        })
                    })
                    .collect();
                (!assets.is_empty()).then_some(AppManifest {
                    version: manifest.version,
                    tag: manifest.tag,
                    ancestors: manifest.ancestors,
                    assets,
                })
            }));
    }
    let manifest: Option<AppManifest> = super::fetch_app_manifest(MANIFEST_ASSET, timeout).await?;
    if manifest
        .as_ref()
        .is_some_and(|manifest| !super::valid_desktop_tag(&manifest.tag))
    {
        return Err(anyhow!(
            "The app update belongs to a different release channel."
        ));
    }
    Ok(manifest)
}

/// Where a downloaded, verified AppImage waits beside the installed one for the
/// restart that renames it into place. Deliberately not `sweep_leftovers`'
/// `.OpenResearch-update-`: this one outlives the run that made it.
const STAGED_DIR: &str = ".OpenResearch-staged";
const STAGED_APPIMAGE: &str = "OpenResearch.AppImage";

/// Replace the `.AppImage` at `appimage` with the latest release.
///
/// While a backend runs from it, the release is only downloaded and verified into
/// [`STAGED_DIR`]; the restart that follows just renames it into place.
pub async fn update(
    appimage: &Path,
    current: &Version,
    dry_run: bool,
    background: bool,
    backend_running: bool,
) -> Result<Outcome> {
    let parent = appimage
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", appimage.display()))?;
    let staged = parent.join(STAGED_DIR);

    // The restart installs what an earlier pass staged without asking the network
    // again: offline, or in the minutes before a new release's manifest is
    // published, it would find nothing to install.
    if !backend_running && !dry_run {
        if let Some((version, tag)) = super::staged_release(&staged, current) {
            ensure_replaceable(appimage, parent)?;
            return install(appimage, &staged, current, &version, &tag, background);
        }
    }

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
    if let (Some(manifest), Some(latest)) = (&published, &latest) {
        let compatible = super::release_is_compatible(
            &manifest.tag,
            &manifest.ancestors,
            Duration::from_secs(10),
        )
        .await?;
        super::record_checked_release(&super::LatestRelease {
            version: latest.clone(),
            tag: manifest.tag.clone(),
            compatible,
        });
        if !compatible {
            if !background {
                println!("{}", super::compatibility_wait_message(&manifest.tag));
            }
            return Ok(Outcome::Done);
        }
    }

    let Some((manifest, latest)) = published
        .zip(latest)
        .filter(|(manifest, latest)| super::release_available(current, latest, &manifest.tag))
    else {
        if !background {
            println!("OpenResearch {} is up to date.", current);
        }
        return Ok(Outcome::Done);
    };

    if dry_run {
        println!(
            "OpenResearch {} → {} is available. Re-run without --dry-run to update.",
            current, latest
        );
        return Ok(Outcome::Done);
    }

    ensure_replaceable(appimage, parent)?;
    let staged_already =
        super::staged_release(&staged, current).is_some_and(|(_, tag)| tag == manifest.tag);
    if !staged_already {
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
        stage(parent, appimage, &staged, &bytes, &latest, &manifest.tag)?;
    }

    if backend_running {
        super::record_deferred(&latest.to_string(), &manifest.tag);
        if !background {
            println!(
                "OpenResearch {} is ready. The running app installs it when it restarts.",
                latest
            );
        }
        return Ok(Outcome::Deferred);
    }

    install(
        appimage,
        &staged,
        current,
        &latest,
        &manifest.tag,
        background,
    )
}

/// Refuse the installs where replacing the file is wrong or impossible, each
/// with the action that fixes it.
fn ensure_replaceable(appimage: &Path, parent: &Path) -> Result<()> {
    if crate::telemetry::build_channel() != "production" {
        return Err(anyhow!(
            "This OpenResearch AppImage isn't an official build, so it won't update itself.\n\
             Download the released AppImage from {}/releases/latest.",
            super::REPO_URL
        ));
    }
    super::probe_writable(parent).map_err(|e| {
        anyhow!(
            "Can't write to {} ({e}), so OpenResearch can't update itself.\nMove {} somewhere \
             you can write to, such as ~/Applications.",
            parent.display(),
            appimage.display()
        )
    })
}

/// Write the AppImage and its release label into a scratch directory and rename it
/// to `staged` last, so `staged` only ever holds a complete, labelled AppImage.
fn stage(
    parent: &Path,
    appimage: &Path,
    staged: &Path,
    bytes: &[u8],
    version: &Version,
    tag: &str,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Beside the target, so the final move is a same-filesystem rename. A killed
    // update leaves its full-size scratch copy, so clear older ones first.
    super::macos_app::sweep_leftovers(parent, appimage);
    let scratch = parent.join(format!(".OpenResearch-update-{}", uuid::Uuid::new_v4()));
    let payload = scratch.join(STAGED_APPIMAGE);

    let written = (|| {
        std::fs::create_dir_all(&scratch)
            .map_err(|e| anyhow!("Could not create {}: {}", scratch.display(), e))?;
        std::fs::write(&payload, bytes)
            .map_err(|e| anyhow!("Could not write {}: {}", payload.display(), e))?;
        std::fs::set_permissions(&payload, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| anyhow!("Could not make {} executable: {}", payload.display(), e))?;
        super::write_staged_release(&scratch, version, tag)?;

        let _ = std::fs::remove_dir_all(staged);
        std::fs::rename(&scratch, staged)
            .map_err(|e| anyhow!("Could not stage {}: {}", staged.display(), e))
    })();
    if written.is_err() {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    written
}

/// Rename the AppImage staged in `staged` over `appimage`.
fn install(
    appimage: &Path,
    staged: &Path,
    current: &Version,
    version: &Version,
    tag: &str,
    background: bool,
) -> Result<Outcome> {
    std::fs::rename(staged.join(STAGED_APPIMAGE), appimage)
        .map_err(|e| anyhow!("Could not replace {}: {}", appimage.display(), e))?;
    let _ = std::fs::remove_dir_all(staged);

    super::record_installed(&version.to_string(), tag);
    if !background {
        println!("✓ Updated OpenResearch {} → {}.", current, version);
        println!("Restart the app to run the new version.");
    }
    Ok(Outcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restage_replaces_the_payload_and_its_release_together() {
        use std::os::unix::fs::PermissionsExt;

        let v = |s: &str| Version::parse(s).unwrap();
        let parent = std::env::temp_dir().join(format!("orx-appimage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&parent).unwrap();
        let appimage = parent.join("OpenResearch.AppImage");
        let staged = parent.join(STAGED_DIR);
        let cache = super::super::CheckCache::default();

        for (bytes, version) in [(&b"first"[..], "0.2.17"), (b"second", "0.2.18")] {
            let tag = format!("v{version}");
            stage(&parent, &appimage, &staged, bytes, &v(version), &tag).unwrap();
        }

        let payload = staged.join(STAGED_APPIMAGE);
        assert_eq!(std::fs::read(&payload).unwrap(), b"second");
        assert_eq!(
            std::fs::metadata(&payload).unwrap().permissions().mode() & 0o777,
            0o755
        );
        if option_env!("ORX_DESKTOP_CANARY_TAG").is_none() {
            assert_eq!(
                super::super::staged_release_in(&staged, &cache, &v("0.2.16")),
                Some((v("0.2.18"), "v0.2.18".into()))
            );
        }
        // Only the stage is left beside the AppImage: no scratch directories.
        let entries: Vec<_> = std::fs::read_dir(&parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, [STAGED_DIR]);

        std::fs::remove_dir_all(&parent).unwrap();
    }
}
