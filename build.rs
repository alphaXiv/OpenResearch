fn main() {
    println!("cargo:rerun-if-env-changed=ORX_OFFICIAL_RELEASE_BUILD");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    println!("cargo:rerun-if-env-changed=GITHUB_REPOSITORY");

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

    embed_windows_icon();
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
        .manifest_optional()
        .unwrap();
}
