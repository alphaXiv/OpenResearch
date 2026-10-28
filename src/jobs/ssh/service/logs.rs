use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::transport::Session;
use crate::error::{anyhow, Result};

#[derive(Clone, Serialize, Deserialize)]
struct Cursor {
    generation: String,
    identity: String,
    boundary: String,
    bytes: u64,
    #[serde(default)]
    replay_until: Option<u64>,
}

struct Paths {
    base: PathBuf,
}
impl Paths {
    fn new(run: &str) -> Self {
        Self {
            base: crate::store::log_path(run),
        }
    }
    fn cursor(&self) -> PathBuf {
        self.base.with_extension("cursor.json")
    }
    fn pending(&self) -> PathBuf {
        self.base.with_extension("pending.json")
    }
    fn lock(&self) -> Result<fd_lock::RwLock<std::fs::File>> {
        std::fs::create_dir_all(
            self.base
                .parent()
                .ok_or_else(|| anyhow!("Missing log directory"))?,
        )?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.base.with_extension("cursor.lock"))?;
        Ok(fd_lock::RwLock::new(file))
    }

    fn version(&self, generation: &str) -> Result<PathBuf> {
        let generation = uuid::Uuid::parse_str(generation)?;
        Ok(self.base.with_extension(format!("{generation}.log")))
    }
}
fn load(path: &Path) -> Result<Option<Cursor>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn save(path: &Path, cursor: &Cursor) -> Result<()> {
    crate::local::git::atomic_write_with_mode(path, &serde_json::to_vec(cursor)?, Some(0o600))?;
    #[cfg(unix)]
    std::fs::File::open(
        path.parent()
            .ok_or_else(|| anyhow!("Missing cursor directory"))?,
    )?
    .sync_all()?;
    Ok(())
}

pub(super) async fn collect(session: &Session, run: &str, dir: &str) -> Result<Value> {
    uuid::Uuid::parse_str(run)?;
    let paths = Paths::new(run);
    let published = load(&paths.cursor())?;
    let pending = load(&paths.pending())?.filter(|pending| {
        published
            .as_ref()
            .is_none_or(|published| published.generation != pending.generation)
    });
    let previous = pending.as_ref().or(published.as_ref());
    let chunk = session
        .request(json!({"op":"log", "dir":dir,
        "offset":previous.map_or(0, |cursor| cursor.bytes),
        "identity":previous.map(|cursor| &cursor.identity),
        "boundary":previous.map(|cursor| &cursor.boundary)}))
        .await?;
    persist(&paths, published, pending, chunk)
}

fn persist(
    paths: &Paths,
    published: Option<Cursor>,
    pending: Option<Cursor>,
    chunk: Value,
) -> Result<Value> {
    let mut lock = paths.lock()?;
    let _guard = lock.write()?;
    let previous = pending.as_ref().or(published.as_ref());
    if chunk["missing"] == true {
        return Err(anyhow!(
            "Remote log is not available yet; retaining the last collected log."
        ));
    }
    let reset = chunk["reset"] == true || previous.is_none();
    let offset = chunk["offset"]
        .as_u64()
        .ok_or_else(|| anyhow!("Invalid remote log offset"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(
        chunk["data"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing log bytes"))?,
    )?;
    if bytes.len() > 256 * 1024 {
        return Err(anyhow!("Remote log chunk exceeds limit"));
    }
    let mut cursor = if reset {
        Cursor {
            generation: uuid::Uuid::new_v4().to_string(),
            identity: String::new(),
            boundary: String::new(),
            bytes: 0,
            replay_until: chunk["size"].as_u64(),
        }
    } else {
        previous
            .cloned()
            .ok_or_else(|| anyhow!("Missing durable log cursor"))?
    };
    if offset != cursor.bytes {
        return Err(anyhow!("Remote log offset changed unexpectedly"));
    }
    let path = paths.version(&cursor.generation)?;
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("Missing log directory"))?,
    )?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    // Discard bytes written before a crash whose cursor was never committed.
    file.set_len(cursor.bytes)?;
    file.seek(SeekFrom::Start(cursor.bytes))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    cursor.bytes += bytes.len() as u64;
    cursor.identity = chunk["identity"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing remote log identity"))?
        .to_owned();
    cursor.boundary = chunk["boundary"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing remote log boundary"))?
        .to_owned();
    let eof = chunk["eof"] == true;
    let replacing = reset || pending.is_some();
    if replacing && cursor.replay_until.is_none() {
        cursor.replay_until = chunk["size"].as_u64();
    }
    let publish = eof || cursor.replay_until.is_some_and(|end| cursor.bytes >= end);
    if replacing && !publish {
        save(&paths.pending(), &cursor)?;
    } else {
        if replacing {
            match std::fs::remove_file(paths.pending()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        cursor.replay_until = None;
        save(&paths.cursor(), &cursor)?;
    }
    for obsolete in [
        pending.as_ref(),
        if publish { published.as_ref() } else { None },
    ]
    .into_iter()
    .flatten()
    {
        if obsolete.generation != cursor.generation {
            let _ = std::fs::remove_file(paths.version(&obsolete.generation)?);
        }
    }
    Ok(json!({"eof":eof, "generation":cursor.generation, "bytes":cursor.bytes}))
}

pub(crate) struct Snapshot {
    pub generation: String,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub size: u64,
}

pub(crate) fn read(run: &str, offset: u64, limit: u64, expected: Option<&str>) -> Result<Snapshot> {
    read_at(&Paths::new(run), offset, limit, expected)
}

fn read_at(paths: &Paths, offset: u64, limit: u64, expected: Option<&str>) -> Result<Snapshot> {
    let lock = paths.lock()?;
    let _guard = lock.read()?;
    let cursor = load(&paths.cursor())?;
    let (path, generation, size) = match cursor {
        Some(cursor) => (
            paths.version(&cursor.generation)?,
            cursor.generation,
            cursor.bytes,
        ),
        None => {
            let path = paths.base.clone();
            let size = std::fs::metadata(&path).map_or(0, |file| file.len());
            (path, "legacy".to_owned(), size)
        }
    };
    let offset = if expected.is_some_and(|expected| expected != generation) || offset > size {
        0
    } else {
        offset
    };
    let mut bytes = Vec::new();
    match std::fs::File::open(&path) {
        Ok(mut file) => {
            file.seek(SeekFrom::Start(offset))?;
            file.take(limit.min(size.saturating_sub(offset)))
                .read_to_end(&mut bytes)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && size == 0 => {}
        Err(error) => return Err(error.into()),
    }
    Ok(Snapshot {
        generation,
        offset,
        bytes,
        size,
    })
}

pub(crate) fn open_current(run: &str) -> Result<(std::fs::File, PathBuf, u64)> {
    let paths = Paths::new(run);
    let lock = paths.lock()?;
    let _guard = lock.read()?;
    let (path, size) = match load(&paths.cursor())? {
        Some(cursor) => (paths.version(&cursor.generation)?, cursor.bytes),
        None => {
            let path = paths.base.clone();
            let size = std::fs::metadata(&path)?.len();
            (path, size)
        }
    };
    Ok((std::fs::File::open(&path)?, path, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(offset: u64, data: &[u8], reset: bool, eof: bool) -> Value {
        json!({"offset":offset, "data":base64::engine::general_purpose::STANDARD.encode(data),
            "reset":reset, "eof":eof, "identity":"remote-file", "boundary":"boundary"})
    }

    #[test]
    fn replay_is_atomic_and_restart_discards_uncommitted_bytes() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-log-publication").unwrap();
        let paths = Paths {
            base: temp.path().join("run.log"),
        };
        std::fs::write(&paths.base, b"old legacy log").unwrap();
        persist(&paths, None, None, chunk(0, b"new \xff", true, false)).unwrap();
        assert_eq!(
            read_at(&paths, 0, 100, None).unwrap().bytes,
            b"old legacy log"
        );
        let pending = load(&paths.pending()).unwrap().unwrap();
        let file = paths.version(&pending.generation).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(file)
            .unwrap()
            .write_all(b"uncommitted")
            .unwrap();
        persist(
            &paths,
            None,
            Some(pending),
            chunk(5, b"partial", false, true),
        )
        .unwrap();
        let first = read_at(&paths, 0, 100, None).unwrap();
        assert_eq!(first.bytes, b"new \xffpartial");
        assert!(!paths.pending().exists());
        let published = load(&paths.cursor()).unwrap();
        persist(
            &paths,
            published,
            None,
            chunk(0, b"a larger replacement log", true, true),
        )
        .unwrap();
        let replaced = read_at(&paths, 12, 100, Some(&first.generation)).unwrap();
        assert_eq!(replaced.offset, 0);
        assert_eq!(replaced.bytes, b"a larger replacement log");
        assert_ne!(replaced.generation, first.generation);
    }

    #[test]
    fn replacement_publishes_at_its_initial_boundary_while_remote_log_keeps_growing() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-growing-log").unwrap();
        let paths = Paths {
            base: temp.path().join("run.log"),
        };
        std::fs::write(&paths.base, b"old").unwrap();
        let mut first = chunk(0, b"abcd", true, false);
        first["size"] = json!(8);
        persist(&paths, None, None, first).unwrap();
        assert_eq!(read_at(&paths, 0, 100, None).unwrap().bytes, b"old");
        let pending = load(&paths.pending()).unwrap();
        let mut next = chunk(4, b"efgh", false, false);
        next["size"] = json!(1000);
        persist(&paths, None, pending, next).unwrap();
        assert_eq!(read_at(&paths, 0, 100, None).unwrap().bytes, b"abcdefgh");
        assert!(!paths.pending().exists());
        let published = load(&paths.cursor()).unwrap();
        persist(&paths, published, None, chunk(8, b"ijkl", false, false)).unwrap();
        assert_eq!(
            read_at(&paths, 0, 100, None).unwrap().bytes,
            b"abcdefghijkl"
        );
    }

    #[test]
    fn readers_never_publish_uncommitted_tail_or_missing_remote_log() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-log-commit").unwrap();
        let paths = Paths {
            base: temp.path().join("run.log"),
        };
        persist(&paths, None, None, chunk(0, b"saved", true, true)).unwrap();
        let published = load(&paths.cursor()).unwrap().unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(paths.version(&published.generation).unwrap())
            .unwrap()
            .write_all(b"unsaved")
            .unwrap();
        assert_eq!(read_at(&paths, 0, 100, None).unwrap().bytes, b"saved");
        assert!(persist(&paths, Some(published), None, json!({"missing":true})).is_err());
        assert_eq!(read_at(&paths, 0, 100, None).unwrap().bytes, b"saved");
    }
}
