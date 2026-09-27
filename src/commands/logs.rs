use std::io::{Read as _, SeekFrom, Write};

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

fn stream_full_log_to<R: std::io::Read, W: std::io::Write>(
    reader: R,
    total: u64,
    stdout: &mut W,
) -> Result<()> {
    let mut reader = reader.take(total);
    let mut buf = [0u8; STREAM_CHUNK];
    let mut bytes_read = 0u64;
    let mut last_byte: Option<u8> = None;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes_read += n as u64;
        last_byte = Some(buf[n - 1]);
        stdout.write_all(&buf[..n])?;
    }
    if bytes_read != total {
        return Err(anyhow::anyhow!(
            "log changed while reading: expected {total} bytes, read {bytes_read}"
        ));
    }
    if bytes_read > 0 && last_byte != Some(b'\n') {
        stdout.write_all(b"\n")?;
    }
    stdout.flush()?;
    Ok(())
}

async fn stream_full_log(file: std::fs::File, total: u64) -> Result<()> {
    let mut stdout = std::io::stdout();
    stream_full_log_to(file, total, &mut stdout)?;
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
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("[local file] no log captured yet for this run.");
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        let total = file.metadata()?.len();
        let bytes = read_preview_suffix(&mut file, total)?;
        let preview = trailing_char_preview(&bytes);
        return print_compact_summary(&path, total, &preview);
    }

    if args.full {
        let path = log_path(&args.run_id);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("[local file] no log captured yet for this run.");
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        let total = file.metadata()?.len();
        return stream_full_log(file, total).await;
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

#[cfg(test)]
mod tests {
    use super::{read_preview_suffix, stream_full_log_to, STREAM_CHUNK};
    use std::io::{Cursor, Read};

    struct AppendingReader {
        original: Cursor<Vec<u8>>,
        appended: Cursor<Vec<u8>>,
        appended_after_first_read: bool,
        original_remaining_at_append: Option<usize>,
    }

    impl AppendingReader {
        fn new(original: Vec<u8>, appended: Vec<u8>) -> Self {
            Self {
                original: Cursor::new(original),
                appended: Cursor::new(appended),
                appended_after_first_read: false,
                original_remaining_at_append: None,
            }
        }
    }

    impl Read for AppendingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.appended_after_first_read {
                let count = self.original.read(buffer)?;
                self.appended_after_first_read = true;
                self.original_remaining_at_append =
                    Some(self.original.get_ref().len() - self.original.position() as usize);
                return Ok(count);
            }

            let count = self.original.read(buffer)?;
            if count > 0 {
                Ok(count)
            } else {
                self.appended.read(buffer)
            }
        }
    }

    #[test]
    fn full_stream_excludes_bytes_appended_before_original_eof() {
        let mut original = vec![b'x'; 4 * STREAM_CHUNK];
        *original.last_mut().unwrap() = b'\n';
        let total = original.len() as u64;
        let appended = b"APPENDED_AFTER_CAPTURE".to_vec();
        let mut reader = AppendingReader::new(original.clone(), appended.clone());
        let mut output = Vec::new();

        stream_full_log_to(&mut reader, total, &mut output).unwrap();

        assert!(reader.appended_after_first_read);
        assert!(reader.original_remaining_at_append.unwrap() > 0);
        assert!(
            !output
                .windows(appended.len())
                .any(|window| window == appended.as_slice()),
            "full stream included appended bytes"
        );
        assert_eq!(output, original);

        let mut unread = Vec::new();
        reader.read_to_end(&mut unread).unwrap();
        assert_eq!(unread, appended);
    }

    #[test]
    fn preview_excludes_bytes_appended_after_length_capture() {
        let captured = b"log bytes present at metadata capture".to_vec();
        let captured_len = captured.len() as u64;
        let mut file = Cursor::new(captured.clone());

        file.get_mut()
            .extend_from_slice(b" appended after metadata capture");

        let preview = read_preview_suffix(&mut file, captured_len).unwrap();
        assert_eq!(preview, captured);
    }

    #[test]
    fn preview_rejects_a_short_read_after_length_capture() {
        let mut file = Cursor::new(b"log bytes at metadata capture".to_vec());
        let captured_len = file.get_ref().len() as u64;
        file.get_mut().clear();

        let error = read_preview_suffix(&mut file, captured_len).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(error
            .to_string()
            .contains(&format!("expected {captured_len} bytes, read 0")));
    }
}
