//! `orx-keep keep DIR` runs a keeper, as `orx keep` and `orx-host keep` do. The other
//! roles are children for the keeper's tests: `cat` echoes stdin, `spam` prints numbered
//! lines until killed, `both` writes to stdout and stderr in turn, `linger` exits while a
//! child it started still holds its stdout, `detach` prints the pid of a child it started
//! in a process group of its own, and `sleep SECONDS` sleeps.

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
        Some("both") => {
            for line in ["out 1", "err 1", "out 2", "err 2"] {
                let written = if line.starts_with("out") {
                    writeln!(std::io::stdout(), "{line}").and_then(|()| std::io::stdout().flush())
                } else {
                    writeln!(std::io::stderr(), "{line}").and_then(|()| std::io::stderr().flush())
                };
                if written.is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            0
        }
        Some("linger") => {
            let started = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["sleep", "4"])
                .spawn();
            i32::from(started.is_err())
        }
        Some("detach") => {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["sleep", "60"])
                .stdout(std::process::Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut command, 0);
            // Left running on purpose: the test stops it through the keeper.
            #[allow(clippy::zombie_processes)]
            let child = command.spawn().unwrap();
            println!("{}", child.id());
            std::thread::sleep(std::time::Duration::from_secs(60));
            0
        }
        Some("sleep") => {
            let seconds = args
                .get(1)
                .and_then(|arg| arg.to_str()?.parse().ok())
                .unwrap_or(1);
            std::thread::sleep(std::time::Duration::from_secs(seconds));
            0
        }
        _ => {
            eprintln!(
                "usage: orx-keep keep DIR | cat | spam | both | linger | detach | sleep SECONDS"
            );
            2
        }
    };
    std::process::exit(status);
}
