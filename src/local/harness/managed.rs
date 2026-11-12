//! Releases of Claude Code, Codex and OpenCode that orx keeps in its cache.
//! The user's own install runs whenever it is at least as new; a copy here
//! stands in when that install is missing or cannot update itself. This module
//! fetches and places copies; `orx up`'s harness setup decides when.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use futures::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use tokio::io::AsyncWriteExt;

use crate::error::{anyhow, bail, Context, Result};

pub(crate) const HARNESSES: [&str; 3] = ["claude-code", "codex", "opencode"];

const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
const CODEX_CHANNEL: &str = "https://releases.openai.com/codex/channels/latest";
const OPENCODE_CHANNEL: &str = "https://opencode.ai/update/api/latest/cli/npm";
const NPM_REGISTRY: &str = "https://registry.npmjs.org";

// A new binary can retire serve children that share a database.
pub(crate) static OPENCODE_IN_USE: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// The host a harness's releases come from, as the install dialog names it.
pub(crate) fn source(harness: &str) -> Option<&'static str> {
    match harness {
        "claude-code" => Some("downloads.claude.ai"),
        "codex" => Some("releases.openai.com"),
        "opencode" => Some("registry.npmjs.org"),
        _ => None,
    }
}

/// Not in the data directory: every copy can be downloaded again.
fn root() -> PathBuf {
    crate::local::git::cache_root().join("managed-agents")
}

fn executable(harness: &str, release: &Path) -> PathBuf {
    let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
    match harness {
        "codex" => release.join("bin").join(exe("codex")),
        "opencode" => release.join(exe("opencode")),
        _ => release.join(exe("claude")),
    }
}

/// The newest copy on disk: its executable and version.
pub(crate) fn installed(harness: &str) -> Option<(PathBuf, semver::Version)> {
    if !HARNESSES.contains(&harness) {
        return None;
    }
    std::fs::read_dir(root().join(harness))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let version = semver::Version::parse(entry.file_name().to_str()?).ok()?;
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
    let expected = executable(harness, &Path::new(harness).join(release));
    (HARNESSES.contains(&harness) && relative == expected.as_path()).then_some(version)
}

/// The first token of a `--version` line that reads as a version, `v` prefix allowed.
pub(crate) fn parse(line: &str) -> Option<semver::Version> {
    line.split_whitespace()
        .find_map(|token| semver::Version::parse(token.trim_start_matches('v')).ok())
}

/// The user's own working install and its version, leaving orx's copies out.
pub(crate) async fn user_install(harness: &str) -> Option<(PathBuf, semver::Version)> {
    if harness == "opencode" {
        let binary = crate::local::opencode::user_binary().await.ok()?;
        return Some((binary.path.clone(), parse(&binary.version)?));
    }
    let candidates = match harness {
        "claude-code" => super::claude::claude_candidates(),
        "codex" => super::codex::codex_candidates(),
        _ => return None,
    };
    match super::detect::select_working_from(candidates, None, None).await? {
        (bin, super::detect::BinProbe::Answered(Some(line))) => Some((bin, parse(&line)?)),
        _ => None,
    }
}

/// A release built for this computer.
pub(crate) struct Release {
    pub(crate) version: semver::Version,
    url: String,
    checksum: Checksum,
    format: Format,
}

enum Checksum {
    Sha256(String),
    Sha512(String),
}

#[derive(Clone)]
enum Format {
    /// The zstd-compressed executable, with the SHA-256 of the executable itself.
    Zstd(String),
    TarGz,
    Npm,
}

/// The newest release for this computer.
pub(crate) async fn latest(harness: &str) -> Result<Release> {
    let client = client()?;
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
        "opencode" => opencode_release(&client, OPENCODE_CHANNEL, NPM_REGISTRY, &asset).await,
        _ => bail!("orx does not install {harness}."),
    }
}

async fn opencode_release(
    client: &reqwest::Client,
    channel: &str,
    registry: &str,
    platform: &str,
) -> Result<Release> {
    let latest = get_json(client, channel).await?;
    let version = latest["version"]
        .as_str()
        .and_then(|version| semver::Version::parse(version).ok())
        .filter(|version| version.pre.is_empty())
        .ok_or_else(|| anyhow!("OpenCode's latest channel did not return a stable version."))?;
    if crate::local::opencode::Protocol::for_major(version.major).is_none() {
        bail!("OpenCode {version} is not supported by this version of OpenResearch.");
    }
    let scope = match latest["metadata"]["package"].as_str() {
        Some("@opencode/cli") => "@opencode",
        Some("@opencode-ai/cli") => "@opencode-ai",
        _ => bail!("OpenCode's latest channel did not return a known CLI package."),
    };
    let package = get_json(
        client,
        &format!("{registry}/{scope}%2fcli-{platform}/{version}"),
    )
    .await?;
    let sha512 = package["dist"]["integrity"]
        .as_str()
        .and_then(|integrity| integrity.strip_prefix("sha512-"))
        .ok_or_else(|| anyhow!("OpenCode {version} lists no SHA-512 for {platform}."))?;
    Ok(Release {
        url: format!("{registry}/{scope}/cli-{platform}/-/cli-{platform}-{version}.tgz"),
        version,
        checksum: Checksum::Sha512(sha512.to_string()),
        format: Format::Npm,
    })
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
        checksum: Checksum::Sha256(sha256.to_ascii_lowercase()),
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
        checksum: Checksum::Sha256(sha256.to_ascii_lowercase()),
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
            Some(format!("{os}-{arch}{baseline}{libc}"))
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
    opencode_host: Option<&crate::local::opencode::AgentHost>,
) -> Result<PathBuf> {
    static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = INSTALLING.lock().await;
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
        fetch(release, &download, progress).await?;
        let unpacked = staging.join("release");
        let (from, into, format) = (download.clone(), unpacked.clone(), release.format.clone());
        let name = harness.to_string();
        tokio::task::spawn_blocking(move || unpack(&name, &format, &from, &into))
            .await
            .map_err(|error| anyhow!("Unpacking failed: {error}"))??;
        verify(harness, &executable(harness, &unpacked), &release.version).await?;
        let _activation = if harness == "opencode" {
            Some(OPENCODE_IN_USE.try_write().map_err(|_| {
                anyhow!("OpenCode is in use. Retry the update after its active chats finish.")
            })?)
        } else {
            None
        };
        if target.exists() {
            tokio::fs::remove_dir_all(&target).await.with_context(|| {
                format!("Could not replace the broken copy at {}", target.display())
            })?;
        }
        if let Some(host) = opencode_host {
            host.retire_idle_children().await;
        }
        place(&unpacked, &target).await
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    result?;
    // A process may still run the previous copy.
    let mut kept_previous = false;
    for (version, path) in copies(harness).await {
        let previous = !kept_previous && version < release.version;
        kept_previous |= previous;
        if version != release.version && !previous {
            let _ = tokio::fs::remove_dir_all(path).await;
        }
    }
    Ok(bin)
}

/// Delete every copy, once the user's own install is current. One a running
/// process holds open on Windows stays until a later pass.
pub(crate) async fn remove_all(harness: &str) {
    for (_, path) in copies(harness).await {
        let _ = tokio::fs::remove_dir_all(path).await;
    }
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
    let mut hasher: Box<dyn sha2::digest::DynDigest + Send> = match release.checksum {
        Checksum::Sha256(_) => Box::new(Sha256::new()),
        Checksum::Sha512(_) => Box::new(Sha512::new()),
    };
    let mut received = 0;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.context("The download was interrupted")?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        progress(received, total);
    }
    file.flush().await?;
    let digest = hasher.finalize();
    let matches = match &release.checksum {
        Checksum::Sha256(expected) => crate::engine::runtime::hex(&digest) == *expected,
        Checksum::Sha512(expected) => {
            base64::engine::general_purpose::STANDARD.encode(digest) == *expected
        }
    };
    if !matches {
        bail!("The download does not match its published checksum.");
    }
    Ok(())
}

fn unpack(harness: &str, format: &Format, archive: &Path, into: &Path) -> Result<()> {
    std::fs::create_dir_all(into)?;
    let file = std::io::BufReader::new(std::fs::File::open(archive)?);
    let bin = executable(harness, into);
    match format {
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
        Format::Npm => {
            let package_bin = Path::new("package/bin").join(bin.file_name().unwrap());
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
            for entry in archive.entries()? {
                let mut entry = entry?;
                if entry.header().entry_type().is_file() && entry.path()? == package_bin {
                    std::io::copy(&mut entry, &mut std::fs::File::create(&bin)?)?;
                    break;
                }
            }
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
        let mut command = crate::process::tokio_command(bin);
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

    #[tokio::test]
    async fn opencode_uses_the_stable_channel_and_exact_platform_package() {
        use axum::{routing::get, Json, Router};
        let app = Router::new()
            .route(
                "/latest",
                get(|| async {
                    Json(serde_json::json!({
                        "version": "2.2.7", "metadata": {"package": "@opencode/cli"}
                    }))
                }),
            )
            .route(
                "/@opencode%2fcli-linux-x64/2.2.7",
                get(|| async { Json(serde_json::json!({"dist": {"integrity": "sha512-test"}})) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let registry = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let release = opencode_release(
            &client().unwrap(),
            &format!("{registry}/latest"),
            &registry,
            "linux-x64",
        )
        .await
        .unwrap();
        assert_eq!(release.version, semver::Version::new(2, 2, 7));
        assert_eq!(
            release.url,
            format!("{registry}/@opencode/cli-linux-x64/-/cli-linux-x64-2.2.7.tgz")
        );
        assert!(matches!(release.checksum, Checksum::Sha512(expected) if expected == "test"));
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn opencode_update_waits_for_active_turns_and_supersedes_the_path_install() {
        use axum::{routing::get, Router};
        use std::os::unix::fs::PermissionsExt;

        if !crate::config::in_test_child(&[]) {
            return;
        }
        let user_bin = crate::config::test_home().join(".opencode/bin/opencode");
        std::fs::create_dir_all(user_bin.parent().unwrap()).unwrap();
        std::fs::write(&user_bin, "#!/bin/sh\necho 1.17.12\n").unwrap();
        std::fs::set_permissions(&user_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let resolve = crate::local::opencode::resolve_binary;
        assert_eq!(
            resolve().await.unwrap().path,
            user_bin.canonicalize().unwrap()
        );

        let bytes = b"#!/bin/sh\necho opencode v2.2.7\n";
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "package/bin/opencode", &bytes[..])
            .unwrap();
        let archive = archive.into_inner().unwrap().finish().unwrap();
        let sha512 = base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&archive));
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
            version: semver::Version::new(2, 2, 7),
            url,
            checksum: Checksum::Sha512(sha512),
            format: Format::Npm,
        };

        let in_use = OPENCODE_IN_USE.read().await;
        assert!(install("opencode", &release, &mut |_, _| {}, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("active chats"));
        assert!(installed("opencode").is_none());
        drop(in_use);

        let installed = install("opencode", &release, &mut |_, _| {}, None)
            .await
            .unwrap();
        assert_eq!(
            resolve().await.unwrap().path,
            installed.canonicalize().unwrap()
        );
        assert_eq!(
            crate::local::opencode::resolve_binary_at(user_bin)
                .await
                .unwrap()
                .version,
            "1.17.12"
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
            checksum: Checksum::Sha256(sha256),
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
                install("codex", &release, &mut |_, _| {}, None)
                    .await
                    .unwrap(),
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
        let platform = if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        };
        assert!(opencode.starts_with(&format!("{platform}-")), "{opencode}");
        assert!(!opencode.contains('.'), "{opencode}");
        assert!(!asset("claude-code").unwrap().contains('.'));
        assert_eq!(asset("cursor"), None);
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
        assert_eq!(found.url, "https://example.test/a");
        assert!(matches!(found.checksum, Checksum::Sha256(expected) if expected == "abc"));
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
