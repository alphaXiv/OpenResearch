//! `orx install-cli` — put `orx` on the user's PATH from the macOS app.
//!
//! The app bundle already carries an `orx` alias next to its executable (agents
//! the app spawns get it via `chat::prepare_env`), but a human's own terminal
//! never sees it. This links that alias into `~/.local/bin`, so an app-only
//! install gains a CLI that is, by construction, the exact build the app
//! updates — one binary, one version, one updater.
//!
//! `~/.local/bin` rather than `/usr/local/bin`: it needs no admin rights, and
//! writing outside the user's home from a GUI app is a bigger promise than this
//! feature is worth. When it isn't on PATH we print the line to add rather than
//! editing shell rc files, which are the user's to own.

use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};
use crate::updates::{self, InstallChannel};

/// What `install` did, so the dashboard can report it without parsing prose.
pub struct Installed {
    pub link: PathBuf,
    pub target: PathBuf,
    /// False when the link's directory isn't on PATH, so the caller can tell the
    /// user the one extra step.
    pub on_path: bool,
    /// True when the link already pointed at the right place.
    pub already_current: bool,
}

/// Link `orx` into `~/.local/bin`, pointing at the running app bundle.
///
/// `force` overwrites a file that is already there and isn't ours — the one case
/// where the right answer depends on what the user meant.
pub fn install(force: bool) -> Result<Installed> {
    let target = bundle_cli_path()?;
    // A bundle missing its alias would otherwise yield a dangling link reported
    // as success.
    if !target.exists() {
        return Err(anyhow!(
            "{} does not exist, so there is nothing to link.",
            target.display()
        ));
    }
    let dir = local_bin();
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow!("Could not create {}: {}", dir.display(), e))?;
    let link = dir.join("orx");

    // Already ours and pointing at this bundle: nothing to do, and nothing below
    // should be able to turn that into an error.
    if std::fs::read_link(&link).is_ok_and(|current| current == target) {
        return Ok(Installed {
            link,
            target,
            on_path: dir_on_path(&dir),
            already_current: true,
        });
    }

    // Shadowing an existing orx would freeze the user on whichever copy PATH
    // happens to resolve first — and if that one self-updates, on a version
    // neither install controls.
    if let Some(existing) = other_orx_on_path(&link) {
        if !force {
            return Err(anyhow!(
                "orx is already on your PATH at {}.\nLinking the app's binary would leave two \
                 copies competing, on possibly different versions.\nRe-run with --force to give \
                 the app's copy precedence anyway.",
                existing.display()
            ));
        }
    }

    // `symlink_metadata` so a symlink is judged as a symlink rather than by what
    // it resolves to — a dangling one has no `exists()` but must still be
    // replaced rather than collided with.
    match std::fs::symlink_metadata(&link) {
        Ok(meta) if meta.is_symlink() => {
            std::fs::remove_file(&link)
                .map_err(|e| anyhow!("Could not replace {}: {}", link.display(), e))?;
        }
        Ok(meta) => {
            if !force {
                return Err(anyhow!(
                    "{} already exists and is not a symlink.\nRe-run with --force to replace it.",
                    link.display()
                ));
            }
            let removed = if meta.is_dir() {
                std::fs::remove_dir_all(&link)
            } else {
                std::fs::remove_file(&link)
            };
            removed.map_err(|e| anyhow!("Could not replace {}: {}", link.display(), e))?;
        }
        Err(_) => {}
    }

    std::os::unix::fs::symlink(&target, &link).map_err(|e| {
        anyhow!(
            "Could not link {} -> {}: {}",
            link.display(),
            target.display(),
            e
        )
    })?;

    Ok(Installed {
        on_path: dir_on_path(&dir),
        link,
        target,
        already_current: false,
    })
}

pub async fn run(args: crate::InstallCliArgs) -> Result<()> {
    let installed = install(args.force)?;
    if installed.already_current {
        println!("orx is already linked at {}.", installed.link.display());
    } else {
        println!(
            "✓ Linked {} -> {}",
            installed.link.display(),
            installed.target.display()
        );
    }
    if !installed.on_path {
        let dir = installed.link.parent().unwrap_or(&installed.link);
        println!(
            "\n{} is not on your PATH. Add this to your shell profile:\n\n  export PATH=\"{}:$PATH\"",
            dir.display(),
            dir.display()
        );
    }
    Ok(())
}

/// The bundle's `orx` alias. Deliberately the alias and not the bundle
/// executable: `argv[0]` decides whether a launch is the GUI app or the CLI
/// (see `commands::app::is_bundle_exe_launch`), so a link named `orx` pointing
/// at `orx` keeps a bare `orx` in a terminal a plain CLI.
fn bundle_cli_path() -> Result<PathBuf> {
    match updates::current_channel()? {
        InstallChannel::AppBundle(root) => Ok(root.join("Contents/MacOS/orx")),
        channel => Err(anyhow!(
            "`orx install-cli` links the macOS app's binary onto your PATH, but this orx is a \
             {} install that is already on it.",
            channel.as_str()
        )),
    }
}

fn local_bin() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("bin")
}

fn dir_on_path(dir: &Path) -> bool {
    let Some(paths) = crate::local::shell_env::search_path() else {
        return false;
    };
    std::env::split_paths(&paths).any(|entry| entry == dir)
}

/// An `orx` on PATH that isn't the link we manage, if any. Compared after
/// canonicalization so a PATH entry that is itself a symlink to the link's
/// directory (`~/bin` -> `~/.local/bin`) isn't mistaken for a rival install.
fn other_orx_on_path(link: &Path) -> Option<PathBuf> {
    let paths = crate::local::shell_env::search_path()?;
    let link_real = link.canonicalize();
    std::env::split_paths(&paths)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join("orx"))
        .filter(|candidate| candidate.exists())
        .find(|candidate| match (candidate.canonicalize(), &link_real) {
            (Ok(candidate), Ok(link)) => &candidate != link,
            _ => candidate != link,
        })
}
