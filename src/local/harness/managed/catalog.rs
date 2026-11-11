use super::*;

pub(super) const AGENTS: &[(&str, &str, &str)] = &[
    ("dsh", "registry.npmjs.org", "@deepseek-ai/dsh"),
    ("mcode", "registry.npmjs.org", "@minimax-ai/code"),
    ("pi-acp", "registry.npmjs.org", "pi-acp"),
    (
        "pi",
        "registry.npmjs.org",
        "@earendil-works/pi-coding-agent",
    ),
    ("zcode-acp-server", "registry.npmjs.org", "zcode-acp-server"),
    ("kimi", "code.kimi.com", ""),
    ("grok", "storage.googleapis.com", ""),
    ("devin", "static.devin.ai", ""),
    ("omp", "build.stencil.so", ""),
    ("fx", "releases.fx.sh", ""),
    ("cursor", "downloads.cursor.com", ""),
    (
        "antigravity",
        "antigravity-cli-auto-updater-974169037036.us-central1.run.app",
        "",
    ),
    ("node", "nodejs.org", ""),
];

pub(crate) fn key(harness: &str) -> String {
    crate::local::acp::presets()
        .into_iter()
        .find(|preset| preset.launch.id == harness)
        .map(|preset| preset.launch.executable)
        .unwrap_or_else(|| harness.to_string())
}

pub(super) fn package(harness: &str) -> Option<&'static str> {
    AGENTS
        .iter()
        .find(|(id, _, _)| *id == harness)
        .map(|(_, _, package)| *package)
        .filter(|package| !package.is_empty())
}

pub(crate) fn supported(harness: &str) -> bool {
    supported_on(&key(harness), std::env::consts::OS, std::env::consts::ARCH)
}

fn supported_on(harness: &str, os: &str, arch: &str) -> bool {
    matches!(os, "macos" | "linux" | "windows")
        && matches!(arch, "aarch64" | "x86_64")
        && !(os == "windows" && harness == "fx")
}

fn platform<'a>(os: &'a str, arch: &'a str) -> (&'a str, &'a str) {
    (
        match os {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        },
        match arch {
            "aarch64" => "arm64",
            _ => "x64",
        },
    )
}

fn target(harness: &str, os: &str, arch: &str) -> String {
    let (os, arch) = platform(os, arch);
    match harness {
        "devin" => format!(
            "{}-{}",
            if arch == "arm64" { "aarch64" } else { "x86_64" },
            match os {
                "darwin" => "apple-darwin",
                "win32" => "pc-windows",
                _ => "unknown-linux",
            }
        ),
        "antigravity" => format!(
            "{}_{}",
            if os == "win32" { "windows" } else { os },
            if arch == "x64" { "amd64" } else { arch }
        ),
        "omp" | "fx" | "grok" => format!(
            "{}-{}",
            match os {
                "darwin" => "macos",
                "win32" => "windows",
                other => other,
            },
            match (harness, arch) {
                ("omp" | "fx" | "grok", "x64") => "x86_64",
                ("fx" | "grok", "arm64") => "aarch64",
                (_, other) => other,
            }
        ),
        _ => format!("{os}-{arch}"),
    }
}

pub(super) fn executable(harness: &str, release: &Path) -> PathBuf {
    if package(harness).is_some() {
        let directory = release.join("node_modules").join(package(harness).unwrap());
        let entry = std::fs::read(directory.join("package.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|metadata| {
                metadata["bin"]
                    .as_str()
                    .or_else(|| metadata["bin"][harness].as_str())
                    .map(str::to_owned)
            });
        return directory.join(entry.unwrap_or_else(|| "missing-entry".into()));
    }
    match harness {
        "node" if cfg!(windows) => release.join("node.exe"),
        "node" => release.join("bin/node"),
        "devin" => release
            .join("bin")
            .join(format!("devin{}", std::env::consts::EXE_SUFFIX)),
        "cursor" => release.join(if cfg!(windows) {
            "cursor-agent.cmd"
        } else {
            "cursor-agent"
        }),
        "antigravity" => release.join(format!("agy{}", std::env::consts::EXE_SUFFIX)),
        _ => release.join(format!("{harness}{}", std::env::consts::EXE_SUFFIX)),
    }
}

async fn text(client: &reqwest::Client, url: &str) -> Result<String> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?
        .trim()
        .to_string())
}

fn version(text: &str) -> Result<semver::Version> {
    // Cursor uses calendar versions with zero-padded month and day.
    let (numbers, suffix) = text
        .trim_start_matches('v')
        .split_once('-')
        .unwrap_or((text.trim_start_matches('v'), ""));
    let numbers = numbers
        .split('.')
        .map(|n| n.parse::<u64>().map(|n| n.to_string()))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .join(".");
    Ok(semver::Version::parse(&format!(
        "{numbers}{}{suffix}",
        if suffix.is_empty() { "" } else { "-" }
    ))?)
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Release manifest has no {name}."))
}

fn matches_engine(version: &semver::Version, range: &str) -> bool {
    range.split("||").any(|alternative| {
        let range = alternative.split_whitespace().collect::<Vec<_>>().join(",");
        semver::VersionReq::parse(&range).is_ok_and(|range| range.matches(version))
    })
}

fn checksum(text: &str, filename: &str) -> Result<String> {
    text.lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?.trim_start_matches('*');
            (name == filename && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| hash.to_ascii_lowercase())
        })
        .ok_or_else(|| anyhow!("No SHA-256 published for {filename}."))
}

pub(super) async fn latest(client: &reqwest::Client, harness: &str) -> Result<Release> {
    if !supported(harness) {
        bail!("{harness} has no release for this platform.");
    }
    let (os, arch) = platform(std::env::consts::OS, std::env::consts::ARCH);
    let mut format = Format::Raw;
    let (v, url, hash) = if let Some(package) = package(harness) {
        let metadata = get_json(
            client,
            &format!("https://registry.npmjs.org/{package}/latest"),
        )
        .await?;
        return Ok(Release {
            version: version(field(&metadata, "version")?)?,
            url: String::new(),
            sha256: String::new(),
            format: Format::Npm,
        });
    } else {
        match harness {
            "node" => {
                let releases = get_json(client, "https://nodejs.org/dist/index.json").await?;
                let mut requirements = Vec::new();
                for package in AGENTS.iter().filter_map(|(id, _, _)| package(id)) {
                    let metadata = get_json(
                        client,
                        &format!("https://registry.npmjs.org/{package}/latest"),
                    )
                    .await?;
                    if let Some(engine) = metadata["engines"]["node"].as_str() {
                        requirements.push(engine.to_string());
                    }
                }
                let v = releases
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|r| r["lts"].is_string())
                    .filter_map(|r| version(r["version"].as_str()?).ok())
                    .filter(|v| requirements.iter().all(|range| matches_engine(v, range)))
                    .max()
                    .ok_or_else(|| {
                        anyhow!(
                            "No Node.js LTS release satisfies the agent runtimes: {}",
                            requirements.join(", ")
                        )
                    })?;
                let node_os = if os == "win32" { "win" } else { os };
                let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
                let file = format!("node-v{v}-{node_os}-{arch}.{ext}");
                let base = format!("https://nodejs.org/dist/v{v}");
                let hash = checksum(
                    &text(client, &format!("{base}/SHASUMS256.txt")).await?,
                    &file,
                )?;
                format = if cfg!(windows) {
                    Format::Zip
                } else {
                    Format::TarGz
                };
                (v, format!("{base}/{file}"), hash)
            }
            "kimi" => {
                let base = "https://code.kimi.com/kimi-code";
                let tag = text(client, &format!("{base}/latest")).await?;
                let manifest =
                    get_json(client, &format!("{base}/binaries/{tag}/manifest.json")).await?;
                let entry = &manifest["platforms"]
                    [target(harness, std::env::consts::OS, std::env::consts::ARCH)];
                (
                    version(&tag)?,
                    format!("{base}/binaries/{tag}/{}", field(entry, "filename")?),
                    field(entry, "checksum")?.into(),
                )
            }
            "devin" => {
                let manifest =
                    get_json(client, "https://static.devin.ai/cli/current/manifest.json").await?;
                let target = target(harness, std::env::consts::OS, std::env::consts::ARCH);
                let entry = &manifest["platforms"][target];
                format = if cfg!(windows) {
                    Format::Zip
                } else {
                    Format::TarGz
                };
                (
                    version(field(&manifest, "version")?)?,
                    field(entry, "url")?.into(),
                    field(entry, "sha256")?.into(),
                )
            }
            "antigravity" => {
                let target = target(harness, std::env::consts::OS, std::env::consts::ARCH);
                let manifest = get_json(client, &format!("https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/{target}.json")).await?;
                let url = field(&manifest, "url")?.to_string();
                format = if url.contains(".tar.gz") {
                    Format::TarGz
                } else if url.contains(".zip") {
                    Format::Zip
                } else {
                    Format::Raw
                };
                (
                    version(field(&manifest, "version")?)?,
                    url,
                    format!("sha512:{}", field(&manifest, "sha512")?),
                )
            }
            "omp" => {
                let target = target(harness, std::env::consts::OS, std::env::consts::ARCH);
                let manifest = get_json(
                    client,
                    &format!(
                        "https://build.stencil.so/api/products/omp/latest/{target}?channel=stable"
                    ),
                )
                .await?;
                let entry = &manifest["file"];
                (
                    version(field(&manifest["build"], "version")?)?,
                    field(&manifest, "download")?.into(),
                    field(entry, "sha256")?.into(),
                )
            }
            "grok" => {
                let base = "https://storage.googleapis.com/grok-build-public-artifacts/cli";
                let tag = text(client, &format!("{base}/stable")).await?;
                let target = target(harness, std::env::consts::OS, std::env::consts::ARCH);
                (
                    version(&tag)?,
                    format!(
                        "{base}/grok-{tag}-{target}{}",
                        if cfg!(windows) { ".exe" } else { "" }
                    ),
                    String::new(),
                )
            }
            "fx" => {
                let tag = text(client, "https://releases.fx.sh/latest.txt").await?;
                let target = target(harness, std::env::consts::OS, std::env::consts::ARCH);
                format = Format::TarGz;
                (
                    version(&tag)?,
                    format!("https://releases.fx.sh/{tag}/fx-{target}.tar.gz"),
                    String::new(),
                )
            }
            "cursor" => {
                let script = text(
                    client,
                    if cfg!(windows) {
                        "https://cursor.com/install?win32=true"
                    } else {
                        "https://cursor.com/install"
                    },
                )
                .await?;
                let tag = script
                    .split("https://downloads.cursor.com/lab/")
                    .nth(1)
                    .and_then(|s| s.split('/').next())
                    .ok_or_else(|| anyhow!("Cursor installer has no release URL."))?;
                format = if cfg!(windows) {
                    Format::Zip
                } else {
                    Format::TarGz
                };
                (
                    version(tag)?,
                    format!(
                        "https://downloads.cursor.com/lab/{tag}/{}/{arch}/agent-cli-package.{}",
                        if os == "win32" { "windows" } else { os },
                        if cfg!(windows) { "zip" } else { "tar.gz" }
                    ),
                    String::new(),
                )
            }
            _ => bail!("Unknown managed agent {harness}."),
        }
    };
    Ok(Release {
        version: v,
        url,
        sha256: hash,
        format,
    })
}

pub(crate) fn preferred(harness: &str, own: Option<PathBuf>) -> Option<PathBuf> {
    super::super::detect::selected_bin(harness, own.into_iter().collect())
}

pub(crate) async fn select(harness: &str) {
    let key = key(harness);
    if let Some((key, _, _)) = AGENTS.iter().find(|(id, _, _)| *id == key) {
        let candidates = crate::local::acp::user_executable_path(key)
            .into_iter()
            .collect();
        super::super::detect::select_working(key, candidates, None).await;
    }
}

pub(crate) fn node() -> Option<PathBuf> {
    installed("node").map(|(bin, _)| bin)
}

pub(crate) fn command(bin: &Path) -> tokio::process::Command {
    let mut command = if version_of(bin).is_some()
        && bin.components().any(|c| c.as_os_str() == "node_modules")
    {
        let mut cmd =
            crate::process::tokio_command(node().unwrap_or_else(|| root().join("missing-node")));
        cmd.arg(bin);
        cmd
    } else {
        crate::process::tokio_command(bin)
    };
    crate::local::chat::prepare_env(&mut command);
    environment(&mut command, bin);
    command
}

pub(crate) fn environment(command: &mut tokio::process::Command, bin: &Path) {
    if version_of(bin).is_none() {
        return;
    }
    let mut paths = Vec::new();
    if let Some(node) = node() {
        paths.extend(node.parent().map(Path::to_path_buf));
    }
    if let Some(root) = bin
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "node_modules"))
    {
        paths.push(root.join(".bin"));
    } else {
        paths.extend(bin.parent().map(Path::to_path_buf));
    }
    if let Some((pi, _)) = installed("pi") {
        if let Some(modules) = pi
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "node_modules"))
        {
            paths.push(modules.join(".bin"));
        }
    }
    let prepared = command
        .as_std()
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value.map(ToOwned::to_owned))
        .or_else(|| crate::local::shell_env::var("PATH"));
    if let Some(path) = prepared {
        paths.extend(std::env::split_paths(&path));
    }
    if let Ok(path) = std::env::join_paths(paths) {
        command.env("PATH", path);
    }
    command.env_remove("NODE_OPTIONS").env_remove("NODE_PATH");
}

pub(super) async fn npm_install(
    harness: &str,
    release: &Release,
    into: &Path,
    progress: &mut (dyn FnMut(u64, Option<u64>) + Send),
) -> Result<()> {
    let node = node().ok_or_else(|| anyhow!("orx's Node.js runtime is missing."))?;
    let runtime = if cfg!(windows) {
        node.parent().unwrap()
    } else {
        node.parent().unwrap().parent().unwrap()
    };
    let npm = runtime.join(if cfg!(windows) {
        "node_modules/npm/bin/npm-cli.js"
    } else {
        "lib/node_modules/npm/bin/npm-cli.js"
    });
    let mut command = crate::process::tokio_command(&node);
    crate::local::chat::prepare_env(&mut command);
    let path = std::env::join_paths(
        std::iter::once(node.parent().unwrap().to_path_buf()).chain(
            crate::local::shell_env::var("PATH")
                .into_iter()
                .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()),
        ),
    )?;
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with("npm_config_")
        {
            command.env_remove(name);
        }
    }
    let home = into.join("install-home");
    tokio::fs::create_dir_all(&home).await?;
    command
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env_remove("NODE_AUTH_TOKEN");
    let config = into.join("npmrc");
    tokio::fs::write(&config, "").await?;
    let global_config = into.join("global-npmrc");
    tokio::fs::write(&global_config, "").await?;
    command
        .arg(npm)
        .args([
            "install",
            "--global=false",
            "--no-audit",
            "--no-fund",
            "--engine-strict",
            "--registry=https://registry.npmjs.org",
        ])
        .arg("--prefix")
        .arg(into)
        .arg(format!("{}@{}", package(harness).unwrap(), release.version))
        .env("PATH", path)
        .env("NPM_CONFIG_USERCONFIG", &config)
        .env("NPM_CONFIG_GLOBALCONFIG", &global_config)
        .env("NPM_CONFIG_CACHE", into.join("npm-cache"))
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .current_dir(into)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let install = tokio::time::timeout(Duration::from_secs(600), command.output());
    tokio::pin!(install);
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let output = loop {
        tokio::select! {
            output = &mut install => break output.context("Agent package installation timed out")??,
            _ = interval.tick() => {
                let mut directories = vec![into.join("npm-cache/_cacache/content-v2")];
                let mut bytes = 0;
                while let Some(directory) = directories.pop() {
                    let Ok(mut entries) = tokio::fs::read_dir(directory).await else { continue };
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let Ok(metadata) = entry.metadata().await else { continue };
                        if metadata.is_dir() { directories.push(entry.path()); }
                        else if metadata.is_file() { bytes += metadata.len(); }
                    }
                }
                progress(bytes, None);
            }
        }
    };
    if !output.status.success() {
        bail!(
            "Agent package installation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _ = tokio::fs::remove_dir_all(into.join("npm-cache")).await;
    let _ = tokio::fs::remove_dir_all(home).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "downloads vendor releases; run with an isolated HOME and XDG directories"]
    async fn sandbox_install() {
        let _lock = super::super::lock_installations().unwrap();
        let agent = std::env::var("ORX_TEST_AGENT").expect("ORX_TEST_AGENT");
        let release = super::super::latest(&agent, None).await.unwrap();
        let bin = super::super::install(&agent, &release, &mut |_, _| {})
            .await
            .unwrap();
        let mut command = command(&bin);
        if package(&agent).is_some() {
            assert_eq!(command.as_std().get_program(), node().unwrap());
            assert_eq!(command.as_std().get_args().next(), Some(bin.as_os_str()));
            let path = command
                .as_std()
                .get_envs()
                .find(|(name, _)| *name == "PATH")
                .unwrap()
                .1
                .unwrap();
            let paths = std::env::split_paths(path).collect::<Vec<_>>();
            assert_eq!(paths[0], node().unwrap().parent().unwrap());
            assert_eq!(paths[1].file_name().unwrap(), ".bin");
            assert!(paths[1].join(&agent).is_file());
            if agent == "pi-acp" {
                assert!(paths[2].join("pi").is_file());
            }
        }
        let output =
            tokio::time::timeout(Duration::from_secs(60), command.arg("--version").output())
                .await
                .unwrap()
                .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{agent}: {}", String::from_utf8_lossy(&output.stdout));
    }

    #[test]
    fn managed_runtime_directories_go_ahead_of_the_prepared_path() {
        let bin = super::super::executable("codex", &root().join("codex").join("0.166.0"));
        let prepared = ["/orx/bin", "/git/cmd"].map(PathBuf::from);
        let mut command = crate::process::tokio_command(&bin);
        command.env("PATH", std::env::join_paths(&prepared).unwrap());
        environment(&mut command, &bin);
        let path = command
            .as_std()
            .get_envs()
            .find(|(name, _)| *name == "PATH")
            .and_then(|(_, value)| value)
            .unwrap();
        let paths: Vec<_> = std::env::split_paths(path).collect();
        assert!(paths.contains(&bin.parent().unwrap().to_path_buf()));
        assert!(paths.ends_with(&prepared), "{paths:?}");
    }

    #[test]
    fn vendor_targets_select_each_supported_platform() {
        for (os, arch, kimi, devin, agy, omp, fx, grok) in [
            (
                "macos",
                "aarch64",
                "darwin-arm64",
                "aarch64-apple-darwin",
                "darwin_arm64",
                "macos-arm64",
                "macos-aarch64",
                "macos-aarch64",
            ),
            (
                "macos",
                "x86_64",
                "darwin-x64",
                "x86_64-apple-darwin",
                "darwin_amd64",
                "macos-x86_64",
                "macos-x86_64",
                "macos-x86_64",
            ),
            (
                "linux",
                "aarch64",
                "linux-arm64",
                "aarch64-unknown-linux",
                "linux_arm64",
                "linux-arm64",
                "linux-aarch64",
                "linux-aarch64",
            ),
            (
                "linux",
                "x86_64",
                "linux-x64",
                "x86_64-unknown-linux",
                "linux_amd64",
                "linux-x86_64",
                "linux-x86_64",
                "linux-x86_64",
            ),
            (
                "windows",
                "x86_64",
                "win32-x64",
                "x86_64-pc-windows",
                "windows_amd64",
                "windows-x86_64",
                "windows-x86_64",
                "windows-x86_64",
            ),
            (
                "windows",
                "aarch64",
                "win32-arm64",
                "aarch64-pc-windows",
                "windows_arm64",
                "windows-arm64",
                "windows-aarch64",
                "windows-aarch64",
            ),
        ] {
            for (agent, expected) in [
                ("kimi", kimi),
                ("devin", devin),
                ("antigravity", agy),
                ("omp", omp),
                ("fx", fx),
                ("grok", grok),
            ] {
                assert_eq!(target(agent, os, arch), expected);
            }
            let manifest =
                serde_json::json!({"platforms":{kimi:{"filename":"binary","checksum":"abc"}}});
            assert_eq!(
                field(&manifest["platforms"][target("kimi", os, arch)], "filename").unwrap(),
                "binary"
            );
        }
    }

    #[test]
    fn node_lts_must_satisfy_every_engine_range() {
        assert!(matches_engine(
            &semver::Version::new(24, 19, 0),
            ">=22.19 <23 || >=24 <27"
        ));
        assert!(!matches_engine(
            &semver::Version::new(23, 0, 0),
            ">=22.19 <23 || >=24 <27"
        ));
        assert!(!matches_engine(&semver::Version::new(24, 8, 0), ">=24.9.0"));
    }

    #[test]
    fn checksums_match_exact_filenames() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum(&format!("{hash}  node.tar.gz\n"), "node.tar.gz").unwrap(),
            hash
        );
        assert!(checksum(&format!("{hash}  other"), "node.tar.gz").is_err());
        assert!(checksum("BAD_CHECKSUM node.tar.gz", "node.tar.gz").is_err());
    }
    #[test]
    fn platform_and_calendar_versions() {
        assert!(!supported_on("fx", "windows", "x86_64"));
        assert!(supported_on("kimi", "macos", "x86_64"));
        assert_eq!(platform("macos", "aarch64"), ("darwin", "arm64"));
        assert_eq!(
            version("2026.11.05-86047df").unwrap().to_string(),
            "2026.11.5-86047df"
        );
    }
}
