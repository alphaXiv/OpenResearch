//! `orx-keep keep DIR` runs a keeper, as `orx keep` and `orx-host keep` do. The other
//! roles are children for the keeper's tests: `cat` echoes stdin, `spam` prints numbered
//! lines until killed.

#![allow(clippy::disallowed_methods)]

use std::io::{BufRead, Write};

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let status = match args.first().and_then(|arg| arg.to_str()) {
        Some("keep") if args.len() == 2 => {
            orx_keep::run(std::path::Path::new(&args[1])).map_or(1, |()| 0)
        }
        Some("cat") => {
            let mut stdout = std::io::stdout();
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let _ = writeln!(stdout, "{line}").and_then(|()| stdout.flush());
            }
            0
        }
        Some("spam") => {
            let mut stdout = std::io::stdout();
            for line in 0.. {
                if writeln!(stdout, "line {line:08}")
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            0
        }
        _ => {
            eprintln!("usage: orx-keep keep DIR | cat | spam");
            2
        }
    };
    std::process::exit(status);
}
