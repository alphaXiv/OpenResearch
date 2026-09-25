/// Resource 1 is the icon Explorer, the Start menu, and the taskbar show.
fn main() {
    println!("cargo:rerun-if-changed=../OpenResearch.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("..").join("OpenResearch.ico");
    // An absolute path, so it doesn't matter which directory rc.exe resolves against.
    let rc = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icon.rc");
    let escaped = icon.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc, format!("1 ICON \"{escaped}\"\n")).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
