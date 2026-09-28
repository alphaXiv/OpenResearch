use std::io::{Read as _, SeekFrom, Write};

use crate::error::Result;
use crate::plane::resolve_run;
use crate::store::{log_path, Store};

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

/// Prints a run's local log path, size, and bounded preview.
pub async fn run(args: crate::LogsArgs) -> Result<()> {
    crate::local::chat::record_chat_target("runs", &args.run_id);
    resolve_run(Store::open()?, &args.run_id)?;

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
    print_compact_summary(&path, total, &preview)
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
