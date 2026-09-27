use std::io::{Read as _, Write};

use crate::error::Result;
use crate::plane::{resolve_run, LogRequest};
use crate::store::{log_path, Store};

const PREVIEW_CHARS: usize = 500;
/// Suffix bytes read before taking the last `PREVIEW_CHARS` Unicode scalars.
const PREVIEW_SUFFIX_BYTES: usize = 2048;
const STREAM_CHUNK: usize = 64 * 1024;

/// Parses a string the way JS `Number(s)` does for our purposes and returns it
/// only if it represents an integer (matching `Number.isInteger`). An empty or
/// non-numeric string yields `None` (JS produces NaN, which is not an integer).
fn parse_integer(s: &str) -> Option<i64> {
    let trimmed = s.trim();
    // JS Number("") === 0, but that branch never matters here because the inputs
    // either come from a non-empty flag value or a split that produced a piece.
    let value: f64 = trimmed.parse().ok()?;
    if value.is_finite() && value.fract() == 0.0 {
        Some(value as i64)
    } else {
        None
    }
}

fn trailing_char_preview(bytes: &[u8]) -> String {
    let suffix = if bytes.len() > PREVIEW_SUFFIX_BYTES {
        &bytes[bytes.len() - PREVIEW_SUFFIX_BYTES..]
    } else {
        bytes
    };
    let text = String::from_utf8_lossy(suffix);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= PREVIEW_CHARS {
        chars.into_iter().collect()
    } else {
        chars[chars.len() - PREVIEW_CHARS..].iter().collect()
    }
}

fn print_compact_summary(path: &std::path::Path, total_bytes: u64, preview: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{}", path.display())?;
    writeln!(stdout, "{} bytes", total_bytes)?;
    writeln!(stdout)?;
    stdout.write_all(preview.as_bytes())?;
    if preview.is_empty() || !preview.ends_with('\n') {
        writeln!(stdout)?;
    }
    writeln!(
        stdout,
        "Use targeted search on this path (for example `rg PATTERN` or an editor) to inspect portions of the log. This preview is not proof of absence."
    )?;
    stdout.flush()?;
    Ok(())
}

async fn stream_full_log(run_id: &str, total: i64) -> Result<()> {
    let path = log_path(run_id);
    let mut file = std::fs::File::open(&path)?;
    let mut stdout = std::io::stdout();
    let mut buf = [0u8; STREAM_CHUNK];
    let mut wrote_any = false;
    let mut last_byte: Option<u8> = None;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        wrote_any = true;
        last_byte = Some(buf[n - 1]);
        stdout.write_all(&buf[..n])?;
    }
    if wrote_any && last_byte != Some(b'\n') {
        stdout.write_all(b"\n")?;
    }
    stdout.flush()?;
    eprintln!("[local file] bytes 0–{} of {}", total, total);
    Ok(())
}

/// Prints a run's terminal log. By default, a compact path/size/preview summary;
/// `--full`, `--head`, `--bytes`, and `--range` keep raw stdout for pipes.
pub async fn run(args: crate::LogsArgs) -> Result<()> {
    crate::local::chat::record_chat_target("runs", &args.run_id);

    let explicit_raw = args.full || args.head || args.bytes.is_some() || args.range.is_some();

    let store = Store::open()?;
    let plane = resolve_run(store, &args.run_id)?;

    if !explicit_raw {
        let path = log_path(&args.run_id);
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                eprintln!("[local file] no log captured yet for this run.");
                return Ok(());
            }
        };
        let total = meta.len();
        let bytes = std::fs::read(&path)?;
        let preview = trailing_char_preview(&bytes);
        return print_compact_summary(&path, total, &preview);
    }

    if args.full {
        let path = log_path(&args.run_id);
        let total = match std::fs::metadata(&path) {
            Ok(m) => m.len() as i64,
            Err(_) => {
                eprintln!("[local file] no log captured yet for this run.");
                return Ok(());
            }
        };
        return stream_full_log(&args.run_id, total).await;
    }

    let mut mode: &str = if args.head { "head" } else { "tail" };
    let mut start_byte: Option<i64> = None;
    let mut end_byte: Option<i64> = None;

    if let Some(range) = args.range.as_deref() {
        let mut parts = range.splitn(2, ':');
        let s = parts.next().unwrap_or("");
        let e = parts.next().unwrap_or("");
        let sb = parse_integer(s);
        let eb = parse_integer(e);
        match (sb, eb) {
            (Some(sb), Some(eb)) if eb > sb => {
                start_byte = Some(sb);
                end_byte = Some(eb);
            }
            _ => {
                eprintln!("--range must be <start>:<end> byte offsets with end > start.");
                std::process::exit(1);
            }
        }
        mode = "range";
    }

    let max_bytes = match args.bytes.as_deref() {
        Some(b) => match parse_integer(b) {
            Some(v) => Some(v),
            None => {
                eprintln!("--bytes must be an integer.");
                std::process::exit(1);
            }
        },
        None => None,
    };

    let log = plane
        .read_log(LogRequest {
            mode: mode.to_string(),
            max_bytes,
            start_byte,
            end_byte,
        })
        .await?;

    if log.missing_local {
        eprintln!("[local file] no log captured yet for this run.");
        return Ok(());
    }

    let mut stdout = std::io::stdout();
    stdout.write_all(&log.content)?;
    if !log.content.is_empty() && !log.content.ends_with(b"\n") {
        stdout.write_all(b"\n")?;
    }
    stdout.flush()?;

    eprintln!("{}", log.footer());
    Ok(())
}
