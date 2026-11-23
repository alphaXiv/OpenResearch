use sha2::{Digest, Sha256};

fn main() {
    if std::path::Path::new("ui/dist").exists() {
        println!("cargo:rerun-if-changed=ui/dist");
    }

    println!("cargo:rerun-if-env-changed=ORX_OFFICIAL_RELEASE_BUILD");
    println!("cargo:rerun-if-env-changed=ORX_DESKTOP_CANARY_TAG");
    println!("cargo:rerun-if-env-changed=ORX_DESKTOP_CANARY_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    println!("cargo:rerun-if-env-changed=GITHUB_REPOSITORY");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");

    let canary_tag = std::env::var("ORX_DESKTOP_CANARY_TAG").ok();
    let canary_sha = std::env::var("ORX_DESKTOP_CANARY_SHA").ok();
    assert_eq!(
        canary_tag.is_some(),
        canary_sha.is_some(),
        "canary builds need both a tag and source SHA"
    );
    if let Some(sha) = &canary_sha {
        assert!(
            sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid canary source SHA"
        );
        if std::env::var("ORX_OFFICIAL_RELEASE_BUILD").as_deref() == Ok("1") {
            assert_eq!(
                std::env::var("GITHUB_SHA").as_deref(),
                Ok(sha.as_str()),
                "canary source SHA must match the workflow commit"
            );
        }
    }

    let channel = match std::env::var("ORX_OFFICIAL_RELEASE_BUILD") {
        Ok(value)
            if value == "1"
                && std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
                && std::env::var("GITHUB_REPOSITORY").as_deref() == Ok("alphaXiv/OpenResearch") =>
        {
            "production"
        }
        Ok(value) if value == "1" => panic!(
            "ORX_OFFICIAL_RELEASE_BUILD=1 is only valid in alphaXiv/OpenResearch GitHub Actions"
        ),
        Ok(value) => {
            panic!("ORX_OFFICIAL_RELEASE_BUILD must be unset or exactly `1`, got `{value}`")
        }
        Err(std::env::VarError::NotPresent) => "development",
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("ORX_OFFICIAL_RELEASE_BUILD must be valid UTF-8")
        }
    };

    println!("cargo:rustc-env=ORX_BUILD_CHANNEL={channel}");

    // `desktop_app`: this build includes the windowed app (src/commands/app.rs).
    println!("cargo:rustc-check-cfg=cfg(desktop_app)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let linux_desktop = os == "linux" && std::env::var_os("CARGO_FEATURE_DESKTOP").is_some();
    if os == "macos" || os == "windows" || linux_desktop {
        println!("cargo:rustc-cfg=desktop_app");
    }

    embed_host_binaries(channel == "production");
    embed_agent_skills();
    embed_windows_icon();
}

/// Writes `$OUT_DIR/agent_skills.rs`: one `AgentSkill` per `agent-skills/<name>/SKILL.md`,
/// with every other file in that directory as a resource.
fn embed_agent_skills() {
    println!("cargo:rerun-if-changed=agent-skills");
    let root =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("agent-skills");

    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .expect("read agent-skills")
        .map(|entry| entry.unwrap().path())
        .filter(|dir| dir.join("SKILL.md").is_file())
        .collect();
    dirs.sort();

    let mut out = String::from("&[\n");
    for dir in dirs {
        let name = dir.file_name().unwrap().to_str().unwrap();
        let mut files = Vec::new();
        collect_files(&dir, &mut files);
        files.sort();

        let mut resources = String::new();
        for file in files {
            let rel = file
                .strip_prefix(&dir)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            if rel == "SKILL.md" {
                continue;
            }
            resources.push_str(&format!(
                "AgentSkillResource {{ path: {rel:?}, content: include_str!({:?}) }},\n",
                file.to_str().unwrap()
            ));
        }
        out.push_str(&format!(
            "AgentSkill {{ name: {name:?}, content: include_str!({:?}), resources: &[\n{resources}] }},\n",
            dir.join("SKILL.md").to_str().unwrap()
        ));
    }
    out.push(']');

    let path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("agent_skills.rs");
    std::fs::write(path, out).expect("write agent_skills.rs");
}

fn collect_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read skill directory") {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Resource 1 is the icon Explorer shows for orx.exe and the app window loads.
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=windows/OpenResearch.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("windows").join("OpenResearch.ico");
    // An absolute path, so it doesn't matter which directory rc.exe resolves against.
    let rc = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icon.rc");
    let escaped = icon.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc, format!("1 ICON \"{escaped}\"\n")).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}

fn embed_host_binaries(required: bool) {
    use std::{fmt::Write, path::PathBuf};

    println!("cargo:rerun-if-env-changed=ORX_EMBED_HOST_DIR");
    let mut generated = String::from("&[\n");
    if let Some(directory) = std::env::var_os("ORX_EMBED_HOST_DIR") {
        let directory = std::path::absolute(directory).unwrap();
        println!("cargo:rerun-if-changed={}", directory.display());
        for (host_os, arch) in [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("darwin", "universal"),
        ] {
            let path = directory.join(format!("orx-host-{host_os}-{arch}"));
            if !path.exists() && !required {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap_or_else(|error| {
                panic!(
                    "Missing {}: {error}; run node scripts/build-host.mjs",
                    path.display()
                )
            });
            assert!(
                !bytes.is_empty() && bytes.len() <= 8 * 1024 * 1024,
                "Invalid orx-host size: {}",
                path.display()
            );
            let sha = format!("{:x}", Sha256::digest(&bytes));
            // Snapshot the hashed bytes so include_bytes! cannot race an input replacement.
            let snapshot =
                PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join(path.file_name().unwrap());
            std::fs::write(&snapshot, bytes).unwrap();
            let arches: &[&str] = if arch == "universal" {
                &["x86_64", "aarch64"]
            } else {
                std::slice::from_ref(&arch)
            };
            for arch in arches {
                writeln!(
                    generated,
                    "({host_os:?}, {arch:?}, {sha:?}, include_bytes!({snapshot:?}).as_slice()),"
                )
                .unwrap();
            }
        }
    } else {
        assert!(!required, "Official builds require ORX_EMBED_HOST_DIR; run node scripts/build-host.mjs and set it to the output directory");
    }
    generated.push_str("]\n");
    std::fs::write(
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("host_binaries.rs"),
        generated,
    )
    .unwrap();
}
