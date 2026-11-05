fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/process_uid.c");
        cc::Build::new()
            .file("src/process_uid.c")
            .compile("process_uid");
    }
}
