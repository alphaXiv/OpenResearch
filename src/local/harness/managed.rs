//! Vendor releases and agent runtimes that orx keeps in its cache.
//! The user's own install runs whenever it is at least as new; a copy here
//! stands in when that install is missing or cannot update itself. This module
//! fetches and places copies; `orx up`'s harness setup decides when.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{anyhow, bail, Context, Result};

mod catalog;
pub(crate) use catalog::{command, environment, key, node, preferred, select, supported};

pub(crate) fn harnesses() -> impl Iterator<Item = &'static str> {
    ["claude-code", "codex", "opencode"].into_iter().chain(
        catalog::AGENTS
            .iter()
            .map(|(id, _, _)| *id)
            .filter(|id| *id != "node"),
    )
}

/// The agent `harness` installs as its runtime, which comes and goes with it.
pub(crate) fn runtime(harness: &str) -> Option<&'static str> {
    (harness == "pi-acp").then_some("pi")
}

const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
const CODEX_CHANNEL: &str = "https://releases.openai.com/codex/channels/latest";
const OPENCODE_RELEASES: &str = "https://api.github.com/repos/anomalyco/opencode/releases";

/// The host a harness's releases come from, as the install dialog names it.
pub(crate) fn source(harness: &str) -> Option<&'static str> {
    match key(harness).as_str() {
        "claude-code" => Some("downloads.claude.ai"),
        "codex" => Some("releases.openai.com"),
        "opencode" => Some("github.com"),
        other => catalog::AGENTS
            .iter()
            .find(|(id, _, _)| *id == other)
            .map(|(_, host, _)| *host),
    }
}

/// Not in the data directory: every copy can be downloaded again.
fn root() -> PathBuf {
    crate::local::git::cache_root().join("managed-agents")
}

/// Hold across install decisions and removal so dashboards cannot race shared runtime cleanup.
pub(crate) fn lock_installations() -> Result<crate::file_lock::FileLock> {
    let root = crate::config::config_dir();
    std::fs::create_dir_all(&root)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("managed-agents.lock"))?;
    lock.try_lock().map_err(|error| {
        anyhow!(
            "Another agent install or removal is in progress. Try again when it finishes: {error}"
        )
    })?;
    Ok(crate::file_lock::FileLock::new(lock))
}

fn executable(harness: &str, release: &Path) -> PathBuf {
    let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
    match harness {
        "codex" => release.join("bin").join(exe("codex")),
        "opencode" => release.join(exe("opencode")),
        "claude-code" => release.join(exe("claude")),
        other => catalog::executable(other, release),
    }
}

/// The newest copy on disk: its executable and version.
pub(crate) fn installed(harness: &str) -> Option<(PathBuf, semver::Version)> {
    installed_for_major(harness, None)
}

pub(crate) fn installed_for_major(
    harness: &str,
    major: Option<u64>,
) -> Option<(PathBuf, semver::Version)> {
    let harness = key(harness);
    let harness = harness.as_str();
    source(harness)?;
    std::fs::read_dir(root().join(harness))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let version = semver::Version::parse(entry.file_name().to_str()?).ok()?;
            if major.is_some_and(|major| version.major != major) {
                return None;
            }
            let bin = executable(harness, &entry.path());
            bin.is_file().then_some((bin, version))
        })
        .max_by(|a, b| a.1.cmp(&b.1))
}

/// The version of `bin` when it is one of orx's copies, read from its release directory.
pub(crate) fn version_of(bin: &Path) -> Option<semver::Version> {
    let root = root();
    let canonical = crate::paths::canonicalize(&root).ok();
    let relative = bin
        .strip_prefix(&root)
        .ok()
        .or_else(|| bin.strip_prefix(canonical.as_ref()?).ok())?;
    let mut parts = relative.components();
    let harness = parts.next()?.as_os_str().to_str()?;
    let release = parts.next()?.as_os_str().to_str()?;
    let version = semver::Version::parse(release).ok()?;
    let expected = executable(harness, &root.join(harness).join(release));
    (source(harness).is_some() && relative == expected.strip_prefix(&root).ok()?).then_some(version)
}

/// The first token of a `--version` line that reads as a version, `v` prefix allowed.
pub(crate) fn parse(line: &str) -> Option<semver::Version> {
    line.split_whitespace()
        .find_map(|token| catalog_version(token.rsplit('/').next()?))
}

fn catalog_version(token: &str) -> Option<semver::Version> {
    semver::Version::parse(token.trim_start_matches('v'))
        .ok()
        .or_else(|| {
            let (date, suffix) = token.split_once('-')?;
            let parts = date
                .split('.')
                .map(str::parse::<u64>)
                .collect::<std::result::Result<Vec<_>, _>>()
                .ok()?;
            (parts.len() == 3)
                .then(|| {
                    semver::Version::parse(&format!(
                        "{}.{}.{}-{suffix}",
                        parts[0], parts[1], parts[2]
                    ))
                    .ok()
                })
                .flatten()
        })
}

/// The user's own working install and its version, leaving orx's copies out.
pub(crate) async fn user_install(harness: &str) -> Option<(PathBuf, semver::Version)> {
    let harness = key(harness);
    if harness == "opencode" {
        let binary = crate::local::opencode::user_binary().await.ok()?;
        return Some((binary.path.clone(), parse(&binary.version)?));
    }
    match user_probe(&harness).await? {
        (bin, super::detect::BinProbe::Answered(Some(line))) => Some((bin, parse(&line)?)),
        _ => None,
    }
}

/// An install of the user's that is there but fails to run, which only orx's copy can stand in for.
pub(crate) async fn user_install_broken(harness: &str) -> bool {
    let harness = key(harness);
    // OpenCode's lookup does not tell a broken install from a missing one.
    harness != "opencode"
        && matches!(
            user_probe(&harness).await,
            Some((_, super::detect::BinProbe::Broken(_)))
        )
}

async fn user_probe(harness: &str) -> Option<(PathBuf, super::detect::BinProbe)> {
    let candidates = match harness {
        "claude-code" => super::claude::claude_candidates(),
        "codex" => super::codex::codex_candidates(),
        "cursor" => super::cursor::cursor_candidates(),
        "antigravity" => super::antigravity::agy_candidates(),
        other => crate::local::acp::user_executable_path(other)
            .into_iter()
            .collect(),
    };
    super::detect::select_working_from(candidates, None, None).await
}

/// A release built for this computer.
pub(crate) struct Release {
    pub(crate) version: semver::Version,
    url: String,
    sha256: String,
    format: Format,
}

#[derive(Clone)]
enum Format {
    /// The zstd-compressed executable, with the SHA-256 of the executable itself.
    Zstd(String),
    TarGz,
    Zip,
    Raw,
    Npm,
}

/// The newest release for this computer. `major` holds OpenCode to one major
/// version, since its majors speak different protocols.
pub(crate) async fn latest(harness: &str, major: Option<u64>) -> Result<Release> {
    let client = client()?;
    let harness = key(harness);
    let harness = harness.as_str();
    if catalog::AGENTS.iter().any(|(id, _, _)| *id == harness) {
        return catalog::latest(&client, harness).await;
    }
    let asset = asset(harness).ok_or_else(|| {
        anyhow!(
            "There is no {harness} release for {} on {}.",
            std::env::consts::ARCH,
            std::env::consts::OS
        )
    })?;
    match harness {
        "claude-code" => claude_release(&client, &asset).await,
        "codex" => {
            let release = get_json(&client, CODEX_CHANNEL).await?;
            let tag = release["tag_name"].as_str().unwrap_or_default();
            let version = semver::Version::parse(tag.trim_start_matches("rust-v"))
                .with_context(|| format!("Codex's latest release has an unexpected tag {tag:?}"))?;
            published_asset(&release, &asset, version, Format::TarGz)
        }
        "opencode" => {
            opencode_release(&client, OPENCODE_RELEASES, &asset, major.unwrap_or(2)).await
        }
        _ => bail!("orx does not install {harness}."),
    }
}

async fn opencode_release(
    client: &reqwest::Client,
    url: &str,
    asset: &str,
    major: u64,
) -> Result<Release> {
    let mut page = 1;
    loop {
        let releases = get_json(client, &format!("{url}?per_page=40&page={page}")).await?;
        let releases = releases
            .as_array()
            .ok_or_else(|| anyhow!("GitHub returned an invalid OpenCode release list."))?;
        if let Some(release) = newest_opencode(releases, asset, major) {
            return release;
        }
        if releases.len() < 40 {
            bail!("GitHub lists no OpenCode {major}.x release.");
        }
        page += 1;
    }
}

/// The newest stable `major` release on one page of OpenCode's releases.
fn newest_opencode(releases: &[Value], asset: &str, major: u64) -> Option<Result<Release>> {
    let (version, release) = releases
        .iter()
        .filter(|release| release["draft"] != true && release["prerelease"] != true)
        .filter_map(|release| {
            let tag = release["tag_name"].as_str()?.strip_prefix('v')?;
            Some((semver::Version::parse(tag).ok()?, release))
        })
        .filter(|(version, _)| version.major == major && version.pre.is_empty())
        .max_by(|a, b| a.0.cmp(&b.0))?;
    let format = if asset.ends_with(".zip") {
        Format::Zip
    } else {
        Format::TarGz
    };
    Some(published_asset(release, asset, version, format))
}

/// Claude Code's own updater follows `autoUpdatesChannel`, so the copy follows it too.
async fn claude_release(client: &reqwest::Client, platform: &str) -> Result<Release> {
    let settings =
        crate::local::native_store::claude_home(crate::local::native_store::NativeStore::Legacy)
            .join("settings.json");
    let stable = std::fs::read(settings)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|settings| settings["autoUpdatesChannel"] == "stable");
    let channel = if stable { "stable" } else { "latest" };
    let text = client
        .get(format!("{CLAUDE_RELEASES}/{channel}"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("Could not look up the latest Claude Code release")?
        .text()
        .await
        .context("Could not read the latest Claude Code release")?;
    let version = semver::Version::parse(text.trim())
        .context("downloads.claude.ai returned something other than a version")?;
    let manifest = get_json(
        client,
        &format!("{CLAUDE_RELEASES}/{version}/manifest.json"),
    )
    .await?;
    let compressed = get_json(
        client,
        &format!("{CLAUDE_RELEASES}/{version}/manifest.zst.json"),
    )
    .await?;
    let entry = &compressed["platforms"][platform];
    let (Some(executable_sha256), Some(file), Some(sha256)) = (
        manifest["platforms"][platform]["checksum"].as_str(),
        entry["binary"].as_str(),
        entry["checksum"].as_str(),
    ) else {
        bail!("Claude Code {version} has no {platform} build.");
    };
    Ok(Release {
        url: format!("{CLAUDE_RELEASES}/{version}/{platform}/{file}"),
        version,
        sha256: sha256.to_ascii_lowercase(),
        format: Format::Zstd(executable_sha256.to_ascii_lowercase()),
    })
}

/// An asset of a GitHub-shaped release, with the SHA-256 its publisher lists.
fn published_asset(
    release: &Value,
    name: &str,
    version: semver::Version,
    format: Format,
) -> Result<Release> {
    let asset = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|asset| asset["name"] == name)
        .ok_or_else(|| anyhow!("Release {version} has no {name}."))?;
    let (Some(url), Some(sha256)) = (
        asset["browser_download_url"].as_str(),
        asset["digest"]
            .as_str()
            .and_then(|digest| digest.strip_prefix("sha256:")),
    ) else {
        bail!("Release {version} lists no download or SHA-256 for {name}.");
    };
    Ok(Release {
        version,
        url: url.to_string(),
        sha256: sha256.to_ascii_lowercase(),
        format,
    })
}

/// The release asset for this computer: a Claude Code platform key, else a file name.
fn asset(harness: &str) -> Option<String> {
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let musl = || {
        [
            "/etc/alpine-release",
            "/lib/libc.musl-x86_64.so.1",
            "/lib/libc.musl-aarch64.so.1",
        ]
        .iter()
        .any(|path| Path::new(path).exists())
    };
    match harness {
        "claude-code" => {
            let os = match os {
                "macos" => "darwin",
                "linux" => "linux",
                "windows" => "win32",
                _ => return None,
            };
            let libc = if os == "linux" && musl() { "-musl" } else { "" };
            Some(format!("{os}-{arch}{libc}"))
        }
        // Codex's Linux builds are static musl binaries that run on glibc too.
        "codex" => {
            let system = match os {
                "macos" => "apple-darwin",
                "linux" => "unknown-linux-musl",
                "windows" => "pc-windows-msvc",
                _ => return None,
            };
            Some(format!(
                "codex-package-{}-{system}.tar.gz",
                std::env::consts::ARCH
            ))
        }
        "opencode" => {
            let os = match os {
                "macos" => "darwin",
                "linux" => "linux",
                "windows" => "windows",
                _ => return None,
            };
            let baseline = if arch == "x64" && !avx2() {
                "-baseline"
            } else {
                ""
            };
            let libc = if os == "linux" && musl() { "-musl" } else { "" };
            let extension = if os == "linux" { "tar.gz" } else { "zip" };
            Some(format!("opencode-{os}-{arch}{baseline}{libc}.{extension}"))
        }
        _ => None,
    }
}

/// OpenCode's default x64 builds need AVX2.
fn avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    return std::is_x86_feature_detected!("avx2");
    #[cfg(not(target_arch = "x86_64"))]
    false
}

/// Download, check and unpack `release` beside any other copies, keeping the
/// previous one for processes still running it. Returns the new executable.
/// `progress` hears the bytes downloaded so far and the total, when known.
pub(crate) async fn install(
    harness: &str,
    release: &Release,
    progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> Result<PathBuf> {
    let harness = key(harness);
    let harness = harness.as_str();
    if matches!(release.format, Format::Npm) {
        let runtime = latest("node", None).await?;
        Box::pin(install("node", &runtime, progress)).await?;
    }
    if let Some(runtime) = runtime(harness) {
        let release = latest(runtime, None).await?;
        Box::pin(install(runtime, &release, progress)).await?;
    }
    let parent = root().join(harness);
    let target = parent.join(release.version.to_string());
    let bin = executable(harness, &target);
    if bin.is_file() && verify(harness, &bin, &release.version).await.is_ok() {
        return Ok(bin);
    }
    tokio::fs::create_dir_all(&parent)
        .await
        .with_context(|| format!("Could not create {}", parent.display()))?;
    sweep_staging(&parent).await;
    let staging = parent.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    let result = async {
        tokio::fs::create_dir_all(&staging).await?;
        let download = staging.join("download");
        let unpacked = staging.join("release");
        if matches!(release.format, Format::Npm) {
            progress(0, None);
            tokio::fs::create_dir_all(&unpacked).await?;
            catalog::npm_install(harness, release, &unpacked, progress).await?;
            if !executable(harness, &unpacked).is_file() {
                bail!("The package has no {harness} executable.");
            }
            return place(&unpacked, &target).await;
        }
        fetch(release, &download, progress).await?;
        let (from, into, format) = (download.clone(), unpacked.clone(), release.format.clone());
        let name = harness.to_string();
        tokio::task::spawn_blocking(move || unpack(&name, &format, &from, &into))
            .await
            .map_err(|error| anyhow!("Unpacking failed: {error}"))??;
        verify(harness, &executable(harness, &unpacked), &release.version).await?;
        if target.exists() {
            tokio::fs::remove_dir_all(&target).await.with_context(|| {
                format!("Could not replace the broken copy at {}", target.display())
            })?;
        }
        place(&unpacked, &target).await
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    result?;
    // A process may still run the previous copy of this major; another major
    // speaks another protocol and goes.
    let mut kept_previous = false;
    for (version, path) in copies(harness).await {
        let previous =
            !kept_previous && version < release.version && version.major == release.version.major;
        kept_previous |= previous;
        if version != release.version && !previous {
            let _ = tokio::fs::remove_dir_all(path).await;
        }
    }
    Ok(executable(harness, &target))
}

/// Delete every copy, once the user's own install is current. One a running
/// process holds open on Windows stays until a later pass.
pub(crate) async fn remove_all(harness: &str) {
    for (_, path) in copies(harness).await {
        let _ = tokio::fs::remove_dir_all(path).await;
    }
}

pub(crate) fn removable_key(harness: &str) -> Result<String> {
    let key = key(harness);
    if matches!(key.as_str(), "claude-code" | "codex" | "opencode") {
        bail!("This agent supports other accounts and providers. Turn it off in Settings → Agents instead.");
    }
    let runtime_only = key == "node"
        || catalog::AGENTS
            .iter()
            .any(|(id, _, _)| runtime(id) == Some(key.as_str()));
    if runtime_only || !catalog::AGENTS.iter().any(|(id, _, _)| *id == key) {
        bail!("Choose an agent installed by OpenResearch to remove.");
    }
    Ok(key)
}

pub(crate) async fn uninstall(harness: &str) -> Result<()> {
    uninstall_at(&root(), harness).await
}

async fn uninstall_at(root: &Path, harness: &str) -> Result<()> {
    let key = removable_key(harness)?;
    if std::fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!("The managed agent directory is a link. Restore it before removing an agent.");
    }
    let keys: Vec<&str> = std::iter::once(key.as_str()).chain(runtime(&key)).collect();
    for key in &keys {
        let path = root.join(key);
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("The managed {key} directory is a link. Restore it before removing the agent.");
        }
    }
    for key in keys {
        let path = root.join(key);
        match tokio::fs::remove_dir_all(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Could not remove {}. Close other apps using this agent and try again.",
                        path.display()
                    )
                })
            }
        }
    }
    Ok(())
}

/// Release directories on disk, newest first.
async fn copies(harness: &str) -> Vec<(semver::Version, PathBuf)> {
    let mut releases = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(root().join(harness)).await else {
        return releases;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Some(version) = entry
            .file_name()
            .to_str()
            .and_then(|name| semver::Version::parse(name).ok())
        {
            releases.push((version, entry.path()));
        }
    }
    releases.sort_by(|a, b| b.0.cmp(&a.0));
    releases
}

/// Staging folders an interrupted install left behind. Recent ones may be
/// another orx process's install in progress.
async fn sweep_staging(parent: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(parent).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_name().to_string_lossy().starts_with(".staging-") {
            continue;
        }
        let stale = entry
            .metadata()
            .await
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(60 * 60));
        if stale {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
}

async fn fetch(
    release: &Release,
    path: &Path,
    progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> Result<()> {
    let response = client()?
        .get(&release.url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Could not download {}", release.url))?;
    let total = response.content_length();
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("Could not create {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut sha512 = sha2::Sha512::new();
    let mut received = 0;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.context("The download was interrupted")?;
        hasher.update(&chunk);
        sha512.update(&chunk);
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        progress(received, total);
    }
    file.flush().await?;
    let actual = if release.sha256.starts_with("sha512:") {
        format!("sha512:{:x}", sha512.finalize())
    } else {
        format!("{:x}", hasher.finalize())
    };
    if !release.sha256.is_empty() && actual != release.sha256 {
        bail!("The download does not match its published SHA-256.");
    }
    Ok(())
}

fn unpack(harness: &str, format: &Format, archive: &Path, into: &Path) -> Result<()> {
    std::fs::create_dir_all(into)?;
    let file = std::io::BufReader::new(std::fs::File::open(archive)?);
    let bin = executable(harness, into);
    match format {
        Format::Npm => bail!("npm packages require the managed runtime"),
        Format::Raw => {
            std::fs::copy(archive, &bin)?;
        }
        Format::Zstd(sha256) => {
            let mut decoder = ruzstd::decoding::StreamingDecoder::new(file)
                .map_err(|error| anyhow!("The download is not zstd: {error}"))?;
            let mut out = std::fs::File::create(&bin)?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0; 1 << 20];
            loop {
                let read = decoder.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                out.write_all(&buffer[..read])?;
            }
            out.flush()?;
            if format!("{:x}", hasher.finalize()) != *sha256 {
                bail!("The unpacked executable does not match its published SHA-256.");
            }
        }
        Format::TarGz => tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(into)
            .context("Could not unpack the download")?,
        Format::Zip => zip::ZipArchive::new(file)
            .and_then(|mut archive| archive.extract(into))
            .context("Could not unpack the download")?,
    }
    if !bin.is_file() {
        let entries = std::fs::read_dir(into)?.collect::<std::io::Result<Vec<_>>>()?;
        if entries.len() == 1 && entries[0].path().is_dir() {
            let nested = entries[0].path();
            for entry in std::fs::read_dir(&nested)? {
                let entry = entry?;
                std::fs::rename(entry.path(), into.join(entry.file_name()))?;
            }
            std::fs::remove_dir(nested)?;
        }
        if harness == "antigravity" && into.join("antigravity").is_file() {
            std::fs::rename(into.join("antigravity"), &bin)?;
        }
    }
    if !bin.is_file() {
        bail!("The download has no {}.", bin.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Runs the unpacked executable, which must name the version it was downloaded as.
pub(crate) async fn verify(harness: &str, bin: &Path, version: &semver::Version) -> Result<()> {
    let reported = if harness == "opencode" {
        // OpenCode writes state on any run, so it answers in a scratch home.
        crate::local::opencode::resolve_binary_at(bin.to_path_buf())
            .await?
            .version
    } else {
        let home = crate::local::git::TemporaryDirectory::new("orx-agent-verify")?;
        let mut command = crate::process::tokio_command(bin);
        command
            .env("HOME", home.path())
            .env("USERPROFILE", home.path());
        for name in [
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ] {
            command.env(name, home.path().join(name));
        }
        command
            .arg("--version")
            .env("NO_COLOR", "1")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(60), command.output())
            .await
            .map_err(|_| anyhow!("The downloaded {harness} did not answer `--version`."))?
            .with_context(|| format!("Could not run the downloaded {harness}"))?;
        if !output.status.success() {
            bail!(
                "The downloaded {harness} failed `--version`: {}",
                output.status
            );
        }
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    if parse(&reported).as_ref() != Some(version) {
        bail!(
            "The downloaded {harness} reports {:?} instead of {version}.",
            reported.trim()
        );
    }
    Ok(())
}

/// Retried: antivirus and the search indexer briefly hold files that were just unpacked.
async fn place(unpacked: &Path, target: &Path) -> Result<()> {
    let mut attempt = 0;
    loop {
        match tokio::fs::rename(unpacked, target).await {
            Ok(()) => return Ok(()),
            // Another orx process finished the same release first.
            Err(_) if target.is_dir() => return Ok(()),
            Err(_) if attempt < 4 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Could not move the release into {}", target.display())
                })
            }
        }
    }
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value> {
    client
        .get(url)
        .header("accept", "application/json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Could not reach {url}"))?
        .json()
        .await
        .with_context(|| format!("Could not read {url}"))
}

fn client() -> Result<reqwest::Client> {
    crate::net::remote_client()
        .user_agent(crate::local::github::UA)
        .connect_timeout(Duration::from_secs(20))
        // Per read, not total: a slow link must still finish a 170 MB download.
        .read_timeout(Duration::from_secs(60))
        .build()
        .context("Could not create an HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn uninstall_validates_before_touching_any_path() {
        let directory =
            Directory(std::env::temp_dir().join(format!("orx-remove-{}", uuid::Uuid::new_v4())));
        let root = directory.0.join("managed-agents");
        std::fs::create_dir_all(root.join("cursor/1.0.0")).unwrap();
        let sentinel = directory.0.join("outside");
        std::fs::write(&sentinel, "keep").unwrap();
        for id in [
            "../x",
            "cursor/..",
            "/tmp/x",
            "",
            "unknown",
            "acp:unknown",
            "node",
            "pi",
            "claude-code",
            "codex",
            "opencode",
        ] {
            assert!(uninstall_at(&root, id).await.is_err(), "{id}");
            assert!(root.join("cursor/1.0.0").is_dir());
            assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        }
    }

    #[tokio::test]
    async fn uninstall_removes_all_copies_and_pi_but_keeps_shared_node() {
        let directory =
            Directory(std::env::temp_dir().join(format!("orx-remove-{}", uuid::Uuid::new_v4())));
        let root = directory.0.join("managed-agents");
        for path in [
            "cursor/1.0.0",
            "cursor/2.0.0",
            "cursor/.staging-partial",
            "dsh/1.0.0",
            "mcode/1.0.0",
            "pi-acp/1.0.0",
            "pi/1.0.0",
            "node/24.0.0",
        ] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        let credentials = directory.0.join("env");
        let saved = "export DEEPSEEK_API_KEY='shared'\nexport OTHER_KEY='keep'\n";
        std::fs::write(&credentials, saved).unwrap();
        let history = directory.0.join("chat-history");
        std::fs::write(&history, "transcript").unwrap();
        let own = directory.0.join("user-bin");
        std::fs::write(&own, "user install").unwrap();
        uninstall_at(&root, "cursor").await.unwrap();
        assert!(!root.join("cursor").exists());
        assert!(root.join("dsh").is_dir());
        let deepseek = crate::local::acp::presets()
            .into_iter()
            .find(|preset| preset.launch.executable == "dsh")
            .unwrap();
        uninstall_at(&root, &deepseek.launch.id).await.unwrap();
        assert!(!root.join("dsh").exists());
        assert!(root.join("node/24.0.0").is_dir());
        assert!(root.join("mcode/1.0.0").is_dir());
        uninstall_at(&root, "mcode").await.unwrap();
        assert!(root.join("node/24.0.0").is_dir());
        uninstall_at(&root, "pi-acp").await.unwrap();
        assert!(!root.join("pi-acp").exists());
        assert!(!root.join("pi").exists());
        assert!(root.join("node/24.0.0").is_dir());
        assert_eq!(std::fs::read_to_string(credentials).unwrap(), saved);
        assert_eq!(std::fs::read_to_string(history).unwrap(), "transcript");
        assert_eq!(std::fs::read_to_string(own).unwrap(), "user install");
        uninstall_at(&root, "cursor").await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uninstall_never_follows_links_outside_managed_root() {
        let directory =
            Directory(std::env::temp_dir().join(format!("orx-remove-{}", uuid::Uuid::new_v4())));
        let root = directory.0.join("managed-agents");
        let outside = directory.0.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), "keep").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("cursor")).unwrap();
        assert!(uninstall_at(&root, "cursor").await.is_err());
        std::fs::remove_file(root.join("cursor")).unwrap();
        std::fs::create_dir_all(root.join("cursor/1.0.0")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("cursor/1.0.0/link")).unwrap();
        uninstall_at(&root, "cursor").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.join("keep")).unwrap(),
            "keep"
        );
        std::os::unix::fs::symlink(&outside, directory.0.join("linked-root")).unwrap();
        assert!(uninstall_at(&directory.0.join("linked-root"), "cursor")
            .await
            .is_err());
        assert!(outside.join("keep").is_file());
    }

    #[test]
    fn managed_opencode_selection_respects_the_users_major() {
        let base = root().join("opencode");
        for version in ["1.99.0", "2.99.0"] {
            let dir = base.join(version);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(executable("opencode", &dir), "fixture").unwrap();
        }
        assert_eq!(installed_for_major("opencode", Some(1)).unwrap().1.major, 1);
        assert_eq!(installed_for_major("opencode", Some(2)).unwrap().1.major, 2);
        assert_eq!(installed_for_major("opencode", Some(3)), None);
        for version in ["1.99.0", "2.99.0"] {
            std::fs::remove_dir_all(base.join(version)).unwrap();
        }
    }

    #[tokio::test]
    async fn opencode_release_follows_pages_for_older_majors() {
        use axum::{extract::Query, routing::get, Json, Router};
        let app = Router::new().route("/releases", get(|Query(query): Query<std::collections::HashMap<String, String>>| async move {
            let version = if query["page"] == "1" { "2.0.0" } else { "1.99.0" };
            let release = serde_json::json!({
                "tag_name": format!("v{version}"),
                "assets": [{"name":"opencode.zip", "browser_download_url":"https://example.test/asset", "digest":"sha256:abc"}]
            });
            Json(if query["page"] == "1" { serde_json::json!(vec![release;40]) } else { serde_json::json!([release]) })
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/releases", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let release = opencode_release(&client().unwrap(), &url, "opencode.zip", 1)
            .await
            .unwrap();
        assert_eq!(release.version, semver::Version::new(1, 99, 0));
        assert!(
            opencode_release(&client().unwrap(), &url, "opencode.zip", 3)
                .await
                .is_err()
        );
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn install_repairs_damaged_and_nonexecutable_current_copies() {
        use axum::{routing::get, Router};
        use std::os::unix::fs::PermissionsExt;
        let version = semver::Version::new(0, 0, 978);
        let bytes = b"#!/bin/sh\necho codex-cli 0.0.978\n";
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "bin/codex", &bytes[..])
            .unwrap();
        let archive = archive.into_inner().unwrap().finish().unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&archive));
        let app = Router::new().route(
            "/archive",
            get(move || {
                let archive = archive.clone();
                async move { archive }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/archive", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let release = Release {
            version: version.clone(),
            url,
            sha256,
            format: Format::TarGz,
        };
        let target = root().join("codex").join(version.to_string());
        let bin = executable("codex", &target);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        for (content, mode) in [
            (b"damaged".as_slice(), 0o755),
            (&bytes[..], 0o644),
            (
                b"#!/bin/sh\necho codex-cli 0.0.978\nexit 1\n".as_slice(),
                0o755,
            ),
        ] {
            std::fs::write(&bin, content).unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(verify("codex", &bin, &version).await.is_err());
            assert!(!matches!(
                crate::local::harness::detect::probe_bin(&bin).await,
                crate::local::harness::detect::BinProbe::Answered(_)
            ));
            assert_eq!(
                install("codex", &release, &mut |_, _| {}).await.unwrap(),
                bin
            );
            verify("codex", &bin, &version).await.unwrap();
            assert_eq!(std::fs::read(&bin).unwrap(), bytes);
        }
        std::fs::remove_dir_all(target).unwrap();
        server.abort();
    }

    #[test]
    fn versions_read_from_cli_banners() {
        assert_eq!(
            parse("2.1.318 (Claude Code)"),
            semver::Version::parse("2.1.318").ok()
        );
        assert_eq!(
            parse("codex-cli 0.166.0"),
            semver::Version::parse("0.166.0").ok()
        );
        assert_eq!(
            parse("opencode v2.2.5"),
            semver::Version::parse("2.2.5").ok()
        );
        assert_eq!(
            parse("codex-cli 0.167.0-alpha.1"),
            semver::Version::parse("0.167.0-alpha.1").ok()
        );
        assert_eq!(parse("omp/18.18.5"), Some(semver::Version::new(18, 18, 5)));
        assert_eq!(parse("not installed"), None);
    }

    #[test]
    fn only_a_copys_own_executable_counts_as_managed() {
        let codex = root().join("codex").join("0.166.0");
        assert_eq!(
            version_of(&executable("codex", &codex)),
            semver::Version::parse("0.166.0").ok()
        );
        assert_eq!(version_of(&codex.join("codex-path").join("rg")), None);
        assert_eq!(
            version_of(&root().join("cursor").join("1.0.0").join("agent")),
            None
        );
        assert_eq!(version_of(Path::new("/usr/local/bin/codex")), None);
    }

    #[test]
    fn assets_name_this_computer() {
        let codex = asset("codex").unwrap();
        assert!(
            codex.starts_with("codex-package-") && codex.ends_with(".tar.gz"),
            "{codex}"
        );
        let opencode = asset("opencode").unwrap();
        assert!(opencode.starts_with("opencode-"), "{opencode}");
        assert_eq!(opencode.ends_with(".tar.gz"), cfg!(target_os = "linux"));
        assert!(!asset("claude-code").unwrap().contains('.'));
        assert_eq!(asset("cursor"), None);
    }

    #[test]
    fn opencode_defaults_to_newest_stable_v2_and_allows_explicit_v1() {
        let releases = serde_json::json!(["1.19.5", "2.1.0", "2.2.5", "2.3.0-beta.1", "3.0.0"].map(|version| serde_json::json!({
            "tag_name":format!("v{version}"),
            "assets":[{"name":"opencode.zip","browser_download_url":"https://example.test/asset","digest":"sha256:abc"}]
        })));
        assert_eq!(
            newest_opencode(releases.as_array().unwrap(), "opencode.zip", 2)
                .unwrap()
                .unwrap()
                .version,
            semver::Version::new(2, 2, 5)
        );
        assert_eq!(
            newest_opencode(releases.as_array().unwrap(), "opencode.zip", 1)
                .unwrap()
                .unwrap()
                .version,
            semver::Version::new(1, 19, 5)
        );
        assert!(newest_opencode(releases.as_array().unwrap(), "opencode.zip", 4).is_none());
    }

    #[test]
    fn published_assets_need_a_sha256() {
        let release = serde_json::json!({"assets": [
            {"name": "codex-package-x.tar.gz", "browser_download_url": "https://example.test/a", "digest": "sha256:ABC"},
            {"name": "unsigned.tar.gz", "browser_download_url": "https://example.test/b"},
        ]});
        let version = semver::Version::new(1, 0, 0);
        let found = published_asset(
            &release,
            "codex-package-x.tar.gz",
            version.clone(),
            Format::TarGz,
        )
        .unwrap();
        assert_eq!(
            (found.url.as_str(), found.sha256.as_str()),
            ("https://example.test/a", "abc")
        );
        assert!(
            published_asset(&release, "unsigned.tar.gz", version.clone(), Format::TarGz).is_err()
        );
        assert!(published_asset(&release, "missing.tar.gz", version, Format::TarGz).is_err());
    }

    #[test]
    fn unpacking_checks_the_executable_and_marks_it_runnable() {
        let dir = crate::local::git::TemporaryDirectory::new("orx-managed-unpack").unwrap();
        let archive = dir.path().join("claude.zst");
        let executable_bytes = b"#!/bin/sh\necho 2.1.318\n";
        // A zstd frame of raw blocks, so the test needs no encoder.
        let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, executable_bytes.len() as u8];
        let header = ((executable_bytes.len() as u32) << 3) | 1;
        frame.extend_from_slice(&header.to_le_bytes()[..3]);
        frame.extend_from_slice(executable_bytes);
        std::fs::write(&archive, &frame).unwrap();
        let sha256 = format!("{:x}", Sha256::digest(executable_bytes));
        let into = dir.path().join("release");
        unpack("claude-code", &Format::Zstd(sha256), &archive, &into).unwrap();
        let bin = executable("claude-code", &into);
        assert_eq!(std::fs::read(&bin).unwrap(), executable_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&bin).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        let tampered = dir.path().join("tampered");
        let wrong = Format::Zstd(format!("{:x}", Sha256::digest(b"other")));
        assert!(unpack("claude-code", &wrong, &archive, &tampered).is_err());
    }
}
