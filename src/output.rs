//! Display helpers for tabular output, and what a command does when its stdout reader goes.

/// Ends the process once stdout's reader has gone: a short command dies as Unix's default
/// SIGPIPE would kill it, and exits cleanly on Windows, which has no such signal.
pub fn stdout_closed() -> ! {
    #[cfg(unix)]
    // SAFETY: restores one signal's default action, then raises it on this thread.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        libc::raise(libc::SIGPIPE);
    }
    std::process::exit(0)
}

/// Routes `println!`'s panic on a closed stdout to [`stdout_closed`]. SIGPIPE stays
/// ignored, so a write to a child's closed stdin still fails with an error its caller reports.
pub fn end_when_stdout_closes() {
    let inner = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let closed = info
            .payload_as_str()
            .and_then(|message| message.strip_prefix("failed printing to stdout: "))
            .is_some_and(is_broken_pipe);
        if closed {
            stdout_closed();
        }
        inner(info);
    }));
}

/// Whether an `io::Error`'s display, `… (os error N)`, names a broken pipe.
fn is_broken_pipe(error: &str) -> bool {
    error
        .rsplit_once("(os error ")
        .and_then(|(_, code)| code.strip_suffix(')')?.parse().ok())
        .is_some_and(|code| {
            std::io::Error::from_raw_os_error(code).kind() == std::io::ErrorKind::BrokenPipe
        })
}

/// Formats a duration in seconds as a compact human string: `1h42m`, `3m12s`,
/// `45s`. Negative inputs render as an em dash.
pub fn format_duration(secs: i64) -> String {
    if secs < 0 {
        return "—".to_string();
    }
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Renders rows as a left-aligned, space-padded table with a header row and a
/// box-drawing separator line,.
///
/// Column widths are the max of the header and every cell. Cells are joined
/// with two spaces, and trailing whitespace is trimmed per line.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let cols = headers.len();
    let mut widths = vec![0usize; cols];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.chars().count();
    }
    for row in rows {
        for (i, w) in widths.iter_mut().enumerate() {
            let len = row.get(i).map(|c| c.chars().count()).unwrap_or(0);
            if len > *w {
                *w = len;
            }
        }
    }

    let line = |cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| pad_end(c, widths.get(i).copied().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };

    let header_cells: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    println!("{}", line(&header_cells));

    let sep: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
    println!("{}", line(&sep));

    for row in rows {
        println!("{}", line(row));
    }
}

/// Pads `s` on the right with spaces to `width` (measured in chars).
fn pad_end(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - len))
    }
}
