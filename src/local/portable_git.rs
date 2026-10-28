//! The Git for Windows orx installs for a user who has none: PortableGit, unpacked
//! into `%LOCALAPPDATA%\OpenResearch\PortableGit` with no administrator prompt and
//! no PATH change. [`super::shell_env::search_path`] appends its `cmd` directory,
//! which is also how [`super::bash`] finds the bash it ships.

#[cfg(windows)]
use crate::error::{anyhow, Context};
use crate::error::{bail, Result};
#[cfg(windows)]
use std::path::{Path, PathBuf};

#[cfg(windows)]
fn base() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("OpenResearch"))
}

/// The installed copy's `cmd` directory, the one Git's own installer puts on PATH.
#[cfg(windows)]
pub fn cmd_dir() -> Option<PathBuf> {
    let dir = base()?.join("PortableGit").join("cmd");
    dir.join("git.exe").is_file().then_some(dir)
}

#[cfg(not(windows))]
pub async fn install() -> Result<()> {
    bail!("orx installs Git only on Windows.")
}

/// Download the latest PortableGit, check it against GitHub's digest, and unpack it.
#[cfg(windows)]
pub async fn install() -> Result<()> {
    static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = INSTALLING.lock().await;
    if cmd_dir().is_some() {
        return Ok(());
    }
    let parent = base().ok_or_else(|| anyhow!("Could not find %LOCALAPPDATA%."))?;
    let root = parent.join("PortableGit");
    tokio::fs::create_dir_all(&parent)
        .await
        .with_context(|| format!("Could not create {}", parent.display()))?;
    sweep_stale(&parent).await;
    // A copy without `cmd\git.exe` is a damaged leftover the rename cannot replace.
    if root.exists() {
        tokio::fs::remove_dir_all(&root)
            .await
            .with_context(|| format!("Could not remove {}", root.display()))?;
    }
    let (url, sha256) = latest_asset().await?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let archive = parent.join(format!("PortableGit-{token}.7z.exe"));
    let staging = parent.join(format!("PortableGit-{token}"));
    let result = async {
        download(&url, &sha256, &archive).await?;
        unpack(&archive, &staging).await?;
        verify(&staging).await?;
        rename_into_place(&staging, &root).await
    }
    .await;
    let _ = tokio::fs::remove_file(&archive).await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
    }
    result
}

/// Runs the unpacked `git.exe` itself: whatever `git` PATH finds first may not be this one.
#[cfg(windows)]
async fn verify(staging: &Path) -> Result<()> {
    let output = tokio::process::Command::new(staging.join(r"cmd\git.exe"))
        .arg("--version")
        .output()
        .await
        .context("Could not run the downloaded Git")?;
    if !output.status.success() {
        bail!("The downloaded Git does not run ({}).", output.status);
    }
    Ok(())
}

/// Retried: antivirus and the search indexer briefly hold files that were just unpacked.
#[cfg(windows)]
async fn rename_into_place(staging: &Path, root: &Path) -> Result<()> {
    let mut attempt = 0;
    loop {
        match tokio::fs::rename(staging, root).await {
            Ok(()) => return Ok(()),
            Err(_) if attempt < 4 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt)).await;
            }
            // Another orx process finished first.
            Err(_) if cmd_dir().is_some() => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not move Git into {}", root.display()))
            }
        }
    }
}

/// Downloads and staging folders an interrupted install left beside `PortableGit`.
/// Recent ones are left alone: they may be another orx process's install in progress.
#[cfg(windows)]
async fn sweep_stale(parent: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(parent).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("PortableGit-")
        {
            continue;
        }
        let recent = entry
            .metadata()
            .await
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_none_or(|age| age < std::time::Duration::from_secs(60 * 60));
        if recent {
            continue;
        }
        let path = entry.path();
        let _ = if path.is_dir() {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_file(&path).await
        };
    }
}

/// The x64 release asset and its SHA-256, from GitHub's own digest. orx ships
/// Windows for x64 only, which Arm machines run emulated.
#[cfg(windows)]
async fn latest_asset() -> Result<(String, String)> {
    let suffix = "-64-bit.7z.exe";
    let release: serde_json::Value = client()?
        .get("https://api.github.com/repos/git-for-windows/git/releases/latest")
        .timeout(std::time::Duration::from_secs(60))
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("Could not look up the latest Git for Windows release")?
        .json()
        .await
        .context("Could not read the latest Git for Windows release")?;
    let asset = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|asset| {
            asset["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("PortableGit-") && name.ends_with(suffix))
        })
        .ok_or_else(|| anyhow!("The latest Git for Windows release has no PortableGit{suffix}."))?;
    let url = asset["browser_download_url"]
        .as_str()
        .ok_or_else(|| anyhow!("The PortableGit asset has no download URL."))?;
    let sha256 = asset["digest"]
        .as_str()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or_else(|| anyhow!("GitHub reports no SHA-256 for the PortableGit download."))?;
    Ok((url.to_string(), sha256.to_ascii_lowercase()))
}

#[cfg(windows)]
async fn download(url: &str, sha256: &str, archive: &Path) -> Result<()> {
    use futures::StreamExt;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    let response = client()?
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("Could not download Git for Windows")?;
    let mut file = tokio::fs::File::create(archive)
        .await
        .with_context(|| format!("Could not create {}", archive.display()))?;
    let mut hasher = Sha256::new();
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.context("The Git for Windows download was interrupted")?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    if format!("{:x}", hasher.finalize()) != sha256 {
        bail!("The Git for Windows download does not match GitHub's checksum.");
    }
    Ok(())
}

/// PortableGit is a 7-Zip self-extractor; `post-install.bat` finishes its `/etc`
/// setup. The copy stays relocatable, so it is set up in staging and then moved.
#[cfg(windows)]
async fn unpack(archive: &Path, staging: &Path) -> Result<()> {
    // Quoted by hand: 7-Zip's extractor reads `-o"<dir>"`, not an argument quoted whole.
    let mut output = std::ffi::OsString::from("-o\"");
    output.push(staging);
    output.push("\"");
    let status = tokio::process::Command::new(archive)
        .arg("-y")
        .raw_arg(output)
        .status()
        .await
        .context("Could not start the Git for Windows extractor")?;
    if !status.success() {
        bail!("Extracting Git for Windows failed ({status}).");
    }
    // The extractor normally runs and deletes it; run it here only if it remains.
    if staging.join("post-install.bat").is_file() {
        let status = tokio::process::Command::new(staging.join("git-bash.exe"))
            .args([
                "--no-needs-console",
                "--hide",
                "--no-cd",
                "--command=post-install.bat",
            ])
            .current_dir(staging)
            .status()
            .await
            .context("Could not run Git for Windows' post-install step")?;
        if !status.success() {
            bail!("Git for Windows' post-install step failed ({status}).");
        }
    }
    if !staging.join(r"bin\bash.exe").is_file() {
        bail!("The Git for Windows download is missing bash.exe.");
    }
    Ok(())
}

#[cfg(windows)]
fn client() -> Result<reqwest::Client> {
    crate::net::remote_client()
        .user_agent(super::github::UA)
        .connect_timeout(std::time::Duration::from_secs(20))
        // Per read, not total: a slow link must still finish the 60 MB download.
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .context("Could not create an HTTP client")
}
