use std::io::{Read as _, SeekFrom, Write};

use crate::error::Result;
use crate::plane::resolve_run;
use crate::store::Store;

const PREVIEW_CHARS: usize = 500;
const PREVIEW_SUFFIX_BYTES: usize = 2048;

fn read_preview_suffix(
    reader: &mut (impl std::io::Read + std::io::Seek),
    total_bytes: u64,
) -> std::io::Result<Vec<u8>> {
    let start = total_bytes.saturating_sub(PREVIEW_SUFFIX_BYTES as u64);
    reader.seek(SeekFrom::Start(start))?;
    let expected_bytes = total_bytes - start;
    let mut bytes = Vec::with_capacity(expected_bytes as usize);
    reader.take(expected_bytes).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "log changed while reading preview: expected {expected_bytes} bytes, read {}",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
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
    writeln!(
        stdout,
        "This is the path to the full log file: {}",
        path.display()
    )?;
    writeln!(stdout, "It is {} bytes.", total_bytes)?;
    writeln!(stdout)?;
    writeln!(stdout, "Here are the last 500 characters of the log file:")?;
    stdout.write_all(preview.as_bytes())?;
    if preview.is_empty() || !preview.ends_with('\n') {
        writeln!(stdout)?;
    }
    writeln!(
        stdout,
        "Use targeted search on this path (for example `rg PATTERN` or an editor) to inspect portions of the log. This preview is not proof of absence."
    )?;
    writeln!(stdout, "Avoid reading the entire log file at once into the context window; search it and read only the relevant portions.")?;
    stdout.flush()?;
    Ok(())
}

/// Prints a run's local log path, size, and bounded preview; follows it through the server
/// with `-f`.
pub async fn run(args: crate::LogsArgs) -> Result<()> {
    crate::local::chat::record_chat_target("runs", &args.run_id);
    if args.follow {
        return crate::plane::local_plane::ApiClient::discover()
            .await?
            .logs(&args.run_id, true)
            .await;
    }
    resolve_run(Store::open()?, &args.run_id)?;
    let ranges = crate::commands::up::api_v2::kept_ranges(&crate::store::data_dir(), &args.run_id)?;
    if let Some(tail) = ranges.iter().max_by_key(|range| range.offset) {
        let total = ranges
            .iter()
            .map(|range| range.offset + range.len)
            .max()
            .unwrap();
        let mut stdout = std::io::stdout();
        writeln!(stdout, "Original log: {total} bytes.")?;
        for range in &ranges {
            writeln!(
                stdout,
                "Kept bytes {}..{}: {}",
                range.offset,
                range.offset + range.len,
                range.file.display()
            )?;
        }
        let kept: u64 = ranges.iter().map(|range| range.len).sum();
        if kept < total {
            writeln!(stdout, "The middle of the log is not on this computer.")?;
        }
        let mut file = std::fs::File::open(&tail.file)?;
        let bytes = read_preview_suffix(&mut file, tail.len)?;
        writeln!(stdout, "Last {PREVIEW_CHARS} characters of the kept tail:")?;
        writeln!(stdout, "{}", trailing_char_preview(&bytes))?;
        return Ok(());
    }

    let log_path = crate::store::log_path(&crate::store::data_dir(), &args.run_id);
    let (mut file, path, total) = match crate::jobs::ssh::service::logs::open_current(log_path) {
        Ok(current) => current,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            let live = Store::open()?
                .get_run(&args.run_id)?
                .is_some_and(|run| !crate::local::is_terminal(&run.status));
            if live {
                return host_preview(&args.run_id).await;
            }
            eprintln!("[local file] no log captured yet for this run.");
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let bytes = read_preview_suffix(&mut file, total)?;
    let preview = trailing_char_preview(&bytes);
    print_compact_summary(&path, total, &preview)
}

/// A live engine run's log stays on its host until the run finishes.
async fn host_preview(run_id: &str) -> Result<()> {
    let client = crate::plane::local_plane::ApiClient::discover().await?;
    let page = |start: u64, end: u64| {
        let client = client.clone();
        async move {
            let page: crate::engine::LogPage = serde_json::from_value(
                client
                    .json(
                        reqwest::Method::GET,
                        &format!(
                            "runs/{}/log?start={start}&end={end}",
                            urlencoding::encode(run_id)
                        ),
                        None,
                    )
                    .await?,
            )?;
            Ok::<_, crate::error::Error>(page)
        }
    };
    let total = match page(u64::MAX - 1, u64::MAX).await? {
        crate::engine::LogPage::Reset { info: Some(info) } => info.len,
        _ => 0,
    };
    let bytes = match page(total.saturating_sub(PREVIEW_SUFFIX_BYTES as u64), total).await? {
        crate::engine::LogPage::Data { bytes, .. } => bytes,
        crate::engine::LogPage::Reset { .. } => Vec::new(),
    };
    let preview = trailing_char_preview(&bytes);
    let mut stdout = std::io::stdout();
    writeln!(
        stdout,
        "This run is still running; its log is on the compute host and is kept on this computer when the run finishes."
    )?;
    writeln!(stdout, "It is {total} bytes so far.")?;
    writeln!(stdout)?;
    writeln!(stdout, "Here are the last 500 characters of the log:")?;
    stdout.write_all(preview.as_bytes())?;
    if preview.is_empty() || !preview.ends_with('\n') {
        writeln!(stdout)?;
    }
    writeln!(
        stdout,
        "Follow it with `{} logs -f {run_id}`. This preview is not proof of absence.",
        crate::invocation::orx()
    )?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::read_preview_suffix;
    use std::io::Cursor;

    #[test]
    fn preview_uses_captured_length_and_rejects_truncation() {
        let captured = b"log bytes at metadata capture".to_vec();
        let captured_len = captured.len() as u64;
        let mut file = Cursor::new(captured.clone());
        file.get_mut().extend_from_slice(b" appended later");
        assert_eq!(
            read_preview_suffix(&mut file, captured_len).unwrap(),
            captured
        );

        file.get_mut().clear();
        let error = read_preview_suffix(&mut file, captured_len).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
