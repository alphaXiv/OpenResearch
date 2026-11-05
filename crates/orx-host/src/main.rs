#[cfg(unix)]
fn main() {
    use std::path::Path;
    let args: Vec<_> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("wrap") if args.len() == 3 => orx_host::wrapper::wrap(Path::new(&args[2])),
        Some("check-run") if args.len() == 3 => orx_host::wrapper::has_live_processes(Path::new(&args[2])).map(|busy| {
            println!("{}", if busy { "busy" } else { "idle" });
        }),
        Some("serve") if args.len() == 4 && args[2] == "--root" => {
            orx_host::legacy::directory(&args[3])
                .and_then(|root| orx_host::server::serve(root, std::env::current_exe()?))
        }
        Some("verify-self") if args.len() == 3 => (|| {
            let actual = orx_host::sha(&std::fs::read(std::env::current_exe()?)?);
            if actual == args[2] {
                Ok(())
            } else {
                Err(std::io::Error::other("binary digest mismatch"))
            }
        })(),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: orx-host serve --root ROOT | wrap RUN_DIR | check-run RUN_DIR | verify-self SHA256",
        )),
    };
    if let Err(error) = result {
        eprintln!("orx-host: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("orx-host requires a Unix host");
    std::process::exit(1);
}
