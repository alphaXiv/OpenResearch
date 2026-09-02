//! Self-update for the downloadable macOS app.
//!
//! The CLI can hand its update to the cargo-dist installer, which owns checksum
//! verification and the atomic rename. There is no equivalent for the `.app`, so
//! this module is the whole pipeline: fetch the manifest, download the DMG,
//! prove it is ours, and swap the bundle.
//!
//! Two independent things establish trust, and both must hold:
//!   1. the `sha256` in `macos-app.json`, which pins the bytes to the release;
//!   2. `codesign` against a Developer ID requirement plus `spctl`, which prove
//!      Apple notarized a bundle signed by *our* team.
//!
//! (2) is the one that matters — it is what makes an unattended swap safe, since
//! it holds even if the release assets themselves were replaced.
//!
//! The swap renames whole directories rather than writing into the installed
//! bundle. Overwriting files inside a running app invalidates its code signature
//! against the kernel's per-inode cache and gets the process SIGKILLed; a rename
//! leaves the running app on its original inode, so it keeps working until the
//! user relaunches.

use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{anyhow, Result};

/// Release asset describing the published app build. Written by
/// `.github/workflows/release-macos-app.yml` next to the DMG it describes.
const MANIFEST_ASSET: &str = "macos-app.json";

/// Apple Developer team the official builds are signed by (`alphaXiv Inc.`).
/// Public — it is stamped into every signed binary — and it is the *expected*
/// value, not a secret.
const EXPECTED_TEAM_ID: &str = "9P69UXUJUK";

/// The Developer ID requirement an official build satisfies: Apple's anchor, our
/// team in the leaf, and the two OID markers that distinguish a Developer ID
/// chain from any other Apple-issued one.
fn signing_requirement() -> String {
    format!(
        "anchor apple generic and certificate leaf[subject.OU] = \"{EXPECTED_TEAM_ID}\" \
         and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
    )
}

#[derive(Debug, Deserialize)]
pub struct AppManifest {
    pub version: String,
    /// Release tag the asset is pinned to, so the download can't drift to a
    /// different release between the manifest fetch and the download.
    pub tag: String,
    pub asset: String,
    pub sha256: String,
}

/// Fetches the published app manifest. `Ok(None)` for a 404 — the expected state
/// between a release being published and its DMG being attached. That is
/// "nothing to update to yet", never an error the user should see.
pub async fn fetch_manifest(timeout: Duration) -> Result<Option<AppManifest>> {
    let url = format!(
        "{}/releases/latest/download/{}",
        super::REPO_URL,
        MANIFEST_ASSET
    );
    let res = super::http()
        .get(&url)
        .header("user-agent", super::UA)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| anyhow!("Could not fetch the macOS app manifest: {}", e))?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = res.status();
    if !status.is_success() {
        return Err(anyhow!(
            "App manifest request failed ({} {})",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        ));
    }
    Ok(Some(serde_json::from_str(&res.text().await?)?))
}

/// Update the installed bundle at `root` in place.
pub async fn update(root: &Path, current: &Version, dry_run: bool, background: bool) -> Result<()> {
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
        // Deliberately before the replaceability checks: reporting that a
        // release exists shouldn't require a writable, signed install.
        println!(
            "OpenResearch {} → {} is available. Re-run without --dry-run to update.",
            current, latest
        );
        return Ok(());
    }

    ensure_replaceable(root).await?;
    if !background {
        eprintln!("Updating OpenResearch {} → {} ...", current, latest);
    }

    // Stage beside the target so the final move is a same-filesystem rename.
    let parent = root
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", root.display()))?;
    // A kill or reboot mid-update leaves ~500MB of staging behind, and this runs
    // unattended for months — so clear any previous run's leftovers first.
    sweep_leftovers(parent, root);
    let staging = parent.join(format!(".OpenResearch-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&staging)
        .map_err(|e| anyhow!("Could not create {}: {}", staging.display(), e))?;

    let result = stage_verified_app(&manifest, &staging).await;
    let staged = match result {
        Ok(staged) => staged,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };

    let swapped = swap_bundle(root, &staged);
    let _ = std::fs::remove_dir_all(&staging);
    swapped?;

    super::record_installed(&latest.to_string());
    if !background {
        println!("✓ Updated OpenResearch {} → {}.", current, latest);
        println!("Restart the app to run the new version.");
    }
    Ok(())
}

/// Refuse the installs where swapping the bundle is wrong or impossible, each
/// with the action that fixes it.
async fn ensure_replaceable(root: &Path) -> Result<()> {
    let display = root.display();
    // Gatekeeper runs an un-moved quarantined app from a randomized read-only
    // mount; the copy the user thinks they installed isn't at this path at all.
    if root
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation")
    {
        return Err(anyhow!(
            "OpenResearch is running from a temporary quarantine location, so it can't update \
             itself.\nMove OpenResearch.app to /Applications and reopen it."
        ));
    }
    let parent = root
        .parent()
        .ok_or_else(|| anyhow!("{display} has no parent directory"))?;
    // Covers the read-only DMG the user never dragged out, without the false
    // positive a `/Volumes/` path test would give an app on an external disk.
    match super::probe_writable(parent) {
        Ok(()) => {}
        Err(e) => {
            return Err(anyhow!(
                "Can't write to {} ({e}), so OpenResearch can't update itself.\nIf you're running \
                 it from the disk image, drag OpenResearch.app to /Applications and open it from \
                 there.",
                parent.display()
            ));
        }
    }
    // Only an official signed build may pull in an official signed update: on an
    // unsigned local build the signature check below would be the only thing
    // standing between a developer's own app and a silent swap for a release.
    if team_id(root).await.as_deref() != Some(EXPECTED_TEAM_ID) {
        return Err(anyhow!(
            "This OpenResearch.app isn't an official signed build, so it won't update itself.\n\
             Install the signed app from {}/releases/latest.",
            super::REPO_URL
        ));
    }
    Ok(())
}

/// The Apple team identifier a bundle is signed by, or `None` when it is
/// unsigned or unreadable.
async fn team_id(root: &Path) -> Option<String> {
    let out = tokio::process::Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(root)
        .output()
        .await
        .ok()?;
    // codesign writes its bundle description to stderr.
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier=").map(str::to_string))
        .filter(|id| id != "not set")
}

/// Download the DMG, verify it, and copy the app out into `staging`. Returns the
/// staged bundle's path.
async fn stage_verified_app(manifest: &AppManifest, staging: &Path) -> Result<PathBuf> {
    let dmg_bytes = super::fetch_release_asset(
        &manifest.tag,
        &manifest.asset,
        // A universal, notarized app bundle is a large download on a slow link.
        Duration::from_secs(600),
    )
    .await?;

    let digest = format!("{:x}", Sha256::digest(&dmg_bytes));
    if !digest.eq_ignore_ascii_case(manifest.sha256.trim()) {
        return Err(anyhow!(
            "The downloaded {} does not match the checksum published for {} \
             (expected {}, got {}). Nothing was installed.",
            manifest.asset,
            manifest.tag,
            manifest.sha256.trim(),
            digest
        ));
    }

    let dmg = staging.join(&manifest.asset);
    std::fs::write(&dmg, &dmg_bytes)
        .map_err(|e| anyhow!("Could not write {}: {}", dmg.display(), e))?;

    let mount = attach(&dmg).await?;
    let staged = copy_verified_app(&mount, staging).await;
    detach(&mount).await;
    let _ = std::fs::remove_file(&dmg);
    staged
}

/// Verify the app on the mounted image and copy it into `staging`. Split from
/// [`stage_verified_app`] so the image is detached on every path out.
async fn copy_verified_app(mount: &Path, staging: &Path) -> Result<PathBuf> {
    let source = mount.join("OpenResearch.app");
    if !source.is_dir() {
        return Err(anyhow!(
            "The downloaded disk image has no OpenResearch.app. Nothing was installed."
        ));
    }

    // `--deep --strict` so a tampered nested binary can't pass on the strength of
    // an intact outer signature.
    run_tool(
        "/usr/bin/codesign",
        &[
            "--verify".into(),
            "--deep".into(),
            "--strict".into(),
            format!("-R={}", signing_requirement()),
            source.to_string_lossy().into_owned(),
        ],
        "The downloaded app is not signed by the expected Developer ID",
    )
    .await?;

    // codesign proves who signed it; only Gatekeeper's assessment proves Apple
    // notarized it, which is what catches a validly-signed but revoked build.
    let assess = tokio::process::Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "exec", "-vv"])
        .arg(&source)
        .output()
        .await
        .map_err(|e| anyhow!("Could not run spctl: {}", e))?;
    let verdict = String::from_utf8_lossy(&assess.stderr);
    if !assess.status.success() || !verdict.contains("source=Notarized Developer ID") {
        return Err(anyhow!(
            "The downloaded app failed Gatekeeper assessment ({}). Nothing was installed.",
            verdict.trim().replace('\n', "; ")
        ));
    }

    // ditto, not a recursive copy: it preserves the extended attributes and
    // symlinks a signed bundle's seal covers.
    let staged = staging.join("OpenResearch.app");
    run_tool(
        "/usr/bin/ditto",
        &[
            source.to_string_lossy().into_owned(),
            staged.to_string_lossy().into_owned(),
        ],
        "Could not copy the new app off the disk image",
    )
    .await?;
    Ok(staged)
}

/// Replace `root` with `staged` by renaming both, and put the old bundle back if
/// the second rename fails — the window where neither is in place is one
/// syscall wide, and never leaves the user without an app.
fn swap_bundle(root: &Path, staged: &Path) -> Result<()> {
    let parent = root
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", root.display()))?;
    let name = root
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", root.display()))?;
    let backup = parent.join(format!(
        ".{}.old-{}",
        name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ));

    std::fs::rename(root, &backup).map_err(|e| {
        anyhow!(
            "Could not move the installed app aside ({e}). {} is untouched.",
            root.display()
        )
    })?;
    if let Err(e) = std::fs::rename(staged, root) {
        let restored = std::fs::rename(&backup, root);
        return Err(match restored {
            Ok(()) => anyhow!(
                "Could not move the new app into place ({e}). The previous app was restored."
            ),
            Err(restore_err) => anyhow!(
                "Could not move the new app into place ({e}), and restoring the previous app \
                 failed too ({restore_err}). It is at {}.",
                backup.display()
            ),
        });
    }
    // The running process still has the old bundle open; removal only unlinks it.
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

/// Remove staging, backup, and probe litter an interrupted update left next to
/// the bundle. Matches only our own `.`-prefixed names, so the live bundle and
/// anything else in `/Applications` are never candidates.
///
/// The age floor keeps this off a *concurrent* updater's staging directory: the
/// lock lives under `config_dir()`, so two user accounts sharing one
/// `/Applications` are not serialized against each other.
fn sweep_leftovers(parent: &Path, root: &Path) {
    const MIN_AGE: Duration = Duration::from_secs(60 * 60);
    let bundle = root.file_name().unwrap_or_default().to_string_lossy();
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if !is_leftover(&entry.file_name().to_string_lossy(), &bundle) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|m| m.elapsed().is_ok_and(|age| age >= MIN_AGE));
        if !stale {
            continue;
        }
        let path = entry.path();
        let _ = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// Whether `name` is one of this module's own temporary artifacts, given the
/// installed bundle's file name. The leading dot is load-bearing: it is what
/// separates our litter from a real application bundle.
fn is_leftover(name: &str, bundle: &str) -> bool {
    name.starts_with(".OpenResearch-update-")
        || name.starts_with(".orx-update-probe-")
        || name.starts_with(&format!(".{bundle}.old-"))
}

/// Mount `dmg` read-only and return its mount point.
async fn attach(dmg: &Path) -> Result<PathBuf> {
    let out = tokio::process::Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen"])
        .arg(dmg)
        .output()
        .await
        .map_err(|e| anyhow!("Could not run hdiutil: {}", e))?;
    if !out.status.success() {
        return Err(anyhow!(
            "Could not mount the downloaded disk image: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    match mount_point(&stdout) {
        Some(mount) => Ok(mount),
        None => {
            // Attached but unparseable: detach by device node, or the image stays
            // mounted for the life of the machine.
            if let Some(device) = stdout.split_whitespace().find(|f| f.starts_with("/dev/")) {
                detach(Path::new(device)).await;
            }
            Err(anyhow!(
                "hdiutil did not report a mount point for the disk image"
            ))
        }
    }
}

/// The mount point in `hdiutil attach` output.
///
/// The table (`/dev/diskN<tab>type<tab>mount point`) is preceded by pages of
/// tab-free checksum-verification chatter, and only the filesystem's row — never
/// the first — carries a mount point. Matching `/Volumes/` rather than "the last
/// field of the last row" is what keeps that preamble out; it holds because we
/// never pass `-mountpoint`.
fn mount_point(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .map(|line| line.split('\t').next_back().unwrap_or_default().trim())
        .find(|field| field.starts_with("/Volumes/"))
        .map(PathBuf::from)
}

async fn detach(mount: &Path) {
    let _ = tokio::process::Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(mount)
        .arg("-force")
        .output()
        .await;
}

/// Run a helper that must succeed, folding its stderr into `context`.
async fn run_tool(program: &str, args: &[String], context: &str) -> Result<()> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| anyhow!("{context}: could not run {program} ({e})"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{context}: {}. Nothing was installed.",
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .replace('\n', "; ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim `hdiutil attach -nobrowse -readonly -noautoopen` output for a
    /// UDZO image — the format we actually parse, preamble and all.
    const ATTACH_OUTPUT: &str = "Checksumming Protective Master Boot Record (MBR : 0)…\n\
        Protective Master Boot Record (MBR :: verified   CRC32 $50BB1F91\n\
        Checksumming disk image (Apple_HFS : 4)…\n\
        \x20         disk image (Apple_HFS : 4): verified   CRC32 $34267BA1\n\
        verified   CRC32 $94FAF33E\n\
        /dev/disk5        \tGUID_partition_scheme          \t\n\
        /dev/disk5s1      \tApple_HFS                      \t/Volumes/OrxParseTest\n";

    #[test]
    fn the_mount_point_is_found_past_the_checksum_preamble() {
        assert_eq!(
            mount_point(ATTACH_OUTPUT),
            Some(PathBuf::from("/Volumes/OrxParseTest"))
        );
        // The partition-scheme row comes first and has an empty mount-point
        // field; taking the last row's last field would pick it up.
        assert!(!ATTACH_OUTPUT.lines().next().unwrap().contains('\t'));
    }

    #[test]
    fn a_volume_name_with_spaces_survives() {
        let out = "/dev/disk5s1\tApple_HFS\t/Volumes/Open Research 2\n";
        assert_eq!(
            mount_point(out),
            Some(PathBuf::from("/Volumes/Open Research 2"))
        );
    }

    #[test]
    fn an_image_that_mounted_nothing_yields_no_mount_point() {
        assert_eq!(mount_point("/dev/disk5\tGUID_partition_scheme\t\n"), None);
        assert_eq!(mount_point(""), None);
    }

    /// The sweep drives an unattended `remove_dir_all` over `/Applications`, so
    /// what it must *not* match matters more than what it does.
    #[test]
    fn only_our_own_litter_is_swept() {
        for name in [
            ".OpenResearch-update-3f2a",
            ".OpenResearch.app.old-3f2a",
            ".orx-update-probe-3f2a",
        ] {
            assert!(is_leftover(name, "OpenResearch.app"), "{name}");
        }
        for name in [
            // The live bundle, and the app the user actually installed.
            "OpenResearch.app",
            "Safari.app",
            // No leading dot: a real bundle someone renamed, not our backup.
            "OpenResearch.app.old-3f2a",
            // A different app whose name merely starts the same way.
            ".OpenResearchNotes-update-3f2a",
            // The backup prefix belongs to the bundle being replaced, so a
            // differently-named bundle's backup is not ours to remove.
            ".Other.app.old-3f2a",
            ".DS_Store",
            "",
        ] {
            assert!(!is_leftover(name, "OpenResearch.app"), "{name}");
        }
    }

    #[test]
    fn the_signing_requirement_pins_our_team_and_a_developer_id_chain() {
        let req = signing_requirement();
        // Anyone can get an Apple-issued certificate; without the team and the
        // two Developer ID OID markers the anchor alone would accept theirs.
        assert!(req.contains(EXPECTED_TEAM_ID), "{req}");
        assert!(req.contains("anchor apple generic"), "{req}");
        assert!(req.contains("1.2.840.113635.100.6.2.6"), "{req}");
        assert!(req.contains("1.2.840.113635.100.6.1.13"), "{req}");
    }
}
