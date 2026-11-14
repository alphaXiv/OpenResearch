use super::log;
use base64::{engine::general_purpose::STANDARD, Engine};
use orx_host_proto::{ByteRange, FileIdent, LogPage};
use orx_signals::{Config, Patterns, Snapshot, MAX_SNAPSHOT};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;

pub fn validate_run(run: &str) -> io::Result<()> {
    if run.is_empty()
        || run.len() > 200
        || !run
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid run id",
        ));
    }
    Ok(())
}

pub fn publish(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let bytes = serde_json::to_vec(snapshot)?;
    if bytes.len() > MAX_SNAPSHOT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "signal snapshot exceeds 16 MiB",
        ));
    }
    publish_bytes(path, &bytes)
}

fn publish_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(
            path.parent()
                .ok_or_else(|| io::Error::other("missing signals directory"))?,
        )?
        .sync_all()?;
        Ok(())
    })();
    if let Err(error) = fs::remove_file(&temporary) {
        if error.kind() != io::ErrorKind::NotFound {
            eprintln!("signal temporary {}: {error}", temporary.display());
        }
    }
    result.map_err(|e: io::Error| {
        io::Error::new(e.kind(), format!("publish signals {}: {e}", path.display()))
    })
}

/// The configuration the log of the checkpoint at `path` is folded with, from the file beside it;
/// with none, the fold of the wire alone.
pub fn configured(path: &Path) -> io::Result<(Config, Option<Arc<Patterns>>)> {
    let file = path.with_extension("config.json");
    let config: Config = match fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("read signals config {}: {e}", file.display()),
            )
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            return Err(io::Error::new(
                e.kind(),
                format!("read signals config {}: {e}", file.display()),
            ))
        }
    };
    if config.patterns.is_empty() {
        return Ok((config, None));
    }
    let patterns = Patterns::compile(&config).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "signals config {}: pattern {}: {}",
                file.display(),
                e.index + 1,
                e.reason
            ),
        )
    })?;
    Ok((config, Some(Arc::new(patterns))))
}

/// Saves the configuration the log of the checkpoint at `path` is folded with, unless it is
/// already that; the next scan folds with it. Whether it changed.
pub fn configure(path: &Path, config: &Config) -> io::Result<bool> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("missing signals directory"));
    };
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let file = path.with_extension("config.json");
    let bytes = serde_json::to_vec(config)?;
    match fs::read(&file) {
        Ok(old) if old == bytes => return Ok(false),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    publish_bytes(&file, &bytes)?;
    Ok(true)
}

pub fn load(path: &Path) -> io::Result<Option<Snapshot>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(io::Error::new(
                e.kind(),
                format!("read signals {}: {e}", path.display()),
            ))
        }
    };
    if file.metadata()?.len() > MAX_SNAPSHOT as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "signal checkpoint exceeds 16 MiB",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SNAPSHOT as u64 + 1).read_to_end(&mut bytes)?;
    let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse signals {}: {e}", path.display()),
        )
    })?;
    FileIdent::try_from(snapshot.cursor.ident.clone())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    validate_run(&snapshot.run_id).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let pending = STANDARD
        .decode(&snapshot.cursor.pending_base64)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut links = std::collections::BTreeMap::new();
    for name in &snapshot.row_order {
        *links.entry(name.as_str()).or_insert(0usize) += 1;
    }
    let row_links_valid = snapshot
        .rows
        .iter()
        .all(|(name, rows)| links.get(name.as_str()).copied().unwrap_or(0) == rows.len())
        && links.keys().all(|name| snapshot.rows.contains_key(*name));
    if pending.len() > orx_signals::MAX_FRAME
        || snapshot.cursor.frame_start > snapshot.cursor.offset
        || snapshot.rev >= 9_007_199_254_740_991
        || !row_links_valid
        || serde_json::to_vec(&snapshot.rows)?.len() > 2 * 1024 * 1024
        || snapshot.latest.len() > 64
        || snapshot.phases.len() > 64
        || snapshot.series.values().any(|p| p.len() > 2048)
        || snapshot
            .series_widths
            .values()
            .any(|width| !width.is_finite() || *width <= 0.0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid bounded checkpoint {}", path.display()),
        ));
    }
    Ok(Some(snapshot))
}

/// Says, once, that `snapshot` folds a log other than the one an earlier checkpoint read: a
/// rebuild that replaces that checkpoint explains what vanished with the old log.
pub fn note_replaced(snapshot: &mut Snapshot, len: u64) {
    if !snapshot.diagnostics.counts.contains_key("log_replaced") {
        snapshot.diagnose(
            "log_replaced",
            0,
            len,
            "raw log shrank or changed identity; rebuilt without splicing generations",
        );
    }
}

/// When the log last changed, in ms since the epoch; a bound on when anything in it was written.
fn written_at(log_path: &Path) -> Option<i64> {
    let modified = fs::metadata(log_path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// Whether folding on from `old`'s cursor continues it over the log at `log_path`, which has
/// `ident` and `len`: the same log, read as far as `old` read it, under the same fold.
pub fn resumes(
    old: &Snapshot,
    run: &str,
    log_path: &Path,
    ident: &FileIdent,
    len: u64,
    config_hash: &str,
) -> io::Result<bool> {
    let terminal = old
        .extra
        .get("terminal")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let same = old.run_id == run
        && old.cursor.ident == String::from(ident.clone())
        && old.cursor.offset <= len
        && old.config_hash == config_hash
        && (!terminal || old.cursor.offset == len);
    let Some(anchor) = old
        .extra
        .get("anchorBase64")
        .and_then(serde_json::Value::as_str)
        .filter(|_| same)
    else {
        return Ok(same);
    };
    let expected = STANDARD
        .decode(anchor)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let range = ByteRange {
        start: old.cursor.offset.saturating_sub(64),
        end: old.cursor.offset,
    };
    Ok(matches!(
        log::read(log_path, Some(ident), &range, None)?,
        LogPage::Data { bytes, .. } if bytes == expected
    ))
}

/// A bounded scan commits raw-frame carry, aggregates and cursor in one owner-only publication.
/// `started` is when the payload started, if the host recorded it.
pub fn scan(
    log_path: &Path,
    path: &Path,
    run: &str,
    terminal: bool,
    now: i64,
    started: Option<i64>,
) -> io::Result<Option<Snapshot>> {
    validate_run(run)?;
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("missing signals directory"));
    };
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut lock = fd_lock::RwLock::new(options.open(path.with_extension("lock"))?);
    let _guard = lock.try_write()?;
    let mut damaged = None;
    let old = match load(path) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            let bad = path.with_extension(format!("corrupt-{}.json", uuid::Uuid::new_v4()));
            fs::rename(path, &bad)?;
            damaged = Some(format!("{e}; retained at {}", bad.display()));
            None
        }
        Err(e) => return Err(e),
    };
    let info = match log::read(log_path, None, &ByteRange { start: 0, end: 0 }, None)? {
        LogPage::Data {
            ident,
            len: Some(len),
            ..
        } => Some((ident, len)),
        LogPage::Data { len: None, .. } => {
            return Err(io::Error::other("raw log length is unavailable"))
        }
        LogPage::Reset { info } => info.map(|i| (i.ident, i.len)),
    };
    let Some((ident, len)) = info else {
        if let Some(mut old) = old {
            if old
                .extra
                .get("terminal")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                old.coverage.add_reason("log_missing");
            }
            return Ok(Some(old));
        }
        return Ok(None);
    };
    let identity: String = ident.clone().into();
    let (config, patterns) = configured(path)?;
    let hash = config.hash();
    let resume = match &old {
        Some(old) => resumes(old, run, log_path, &ident, len, &hash)?,
        None => false,
    };
    let observed = resume;
    // Read again with new patterns, the same log replaces nothing.
    let reconfigured = old.as_ref().is_some_and(|old| old.config_hash != hash);
    let replaced = old.is_some() && !resume && !reconfigured;
    let mut snapshot = if resume {
        old.unwrap()
    } else {
        Snapshot::new(run, &identity, &uuid::Uuid::new_v4().to_string())
    };
    if !resume {
        snapshot.updated_at = now;
    }
    snapshot.configure(patterns);
    snapshot.timing.started_ms = snapshot.timing.started_ms.or(started);
    if replaced {
        snapshot.coverage.add_reason("log_replaced");
        note_replaced(&mut snapshot, len);
    }
    if let Some(error) = damaged {
        snapshot.coverage.add_reason("rebuilding");
        snapshot.diagnose("checkpoint_unreadable", 0, 0, &error);
    }
    let old_rev = snapshot.rev;
    for _ in 0..4 {
        if snapshot.cursor.offset >= len {
            break;
        }
        let offset = snapshot.cursor.offset;
        match log::read(
            log_path,
            Some(&ident),
            &ByteRange {
                start: offset,
                end: (offset + 1024 * 1024).min(len),
            },
            None,
        )? {
            LogPage::Data { bytes, .. } => snapshot
                .feed(&bytes, false, now, observed)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            LogPage::Reset { .. } => {
                return Err(io::Error::other(
                    "log changed during signal scan; retry inspection",
                ))
            }
        }
    }
    if snapshot.cursor.offset < len {
        snapshot.coverage.add_reason("catching_up");
    }
    if snapshot.cursor.offset == len {
        snapshot.remove_reason("catching_up");
        snapshot.remove_reason("rebuilding");
        snapshot.remove_reason("log_replaced");
        if terminal {
            snapshot.remove_reason("collection_incomplete");
            snapshot.extra.remove("collectionError");
        }
        snapshot
            .feed(&[], terminal, now, observed)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        snapshot.reached_end(written_at(log_path).map_or(now, |at| at.min(now)));
    }
    if let LogPage::Data { bytes, .. } = log::read(
        log_path,
        Some(&ident),
        &ByteRange {
            start: snapshot.cursor.offset.saturating_sub(64),
            end: snapshot.cursor.offset,
        },
        None,
    )? {
        snapshot.extra.insert(
            "anchorBase64".into(),
            serde_json::Value::String(STANDARD.encode(bytes)),
        );
    } else {
        return Err(io::Error::other("log changed before signal publication"));
    }
    if snapshot.rev != old_rev || !resume {
        publish(path, &snapshot)?;
    }
    Ok(Some(snapshot))
}
