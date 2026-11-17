use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{anyhow, Context as _, Error, Result};
use crate::local::files::{self, artifact_version};
use crate::store::{Store, StoredRun};

pub(super) const LAST: usize = 6;
pub(super) const BLOCKS: usize = 4;
pub(super) const MAX_PATH: usize = 240;
const MAX_ENTRIES: usize = 2048;
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SIDE: u32 = 4096;

fn safe_path(path: &str) -> bool {
    path.len() <= MAX_PATH
        && files::is_safe_rel_path(path)
        && !path.contains(':')
        && !path.chars().any(char::is_control)
}

pub(super) fn valid_glob(glob: &str) -> bool {
    safe_path(glob)
        && !glob.contains(['?', '[', ']'])
        && glob.matches('*').count() == 1
        && glob
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains('*'))
}

fn open_run(project: &Path, run: &str) -> Result<PathBuf> {
    if !safe_path(run) || run.contains('/') {
        return Err(anyhow!("Invalid run ID: {run}"));
    }
    let runs = project.join("runs");
    let root = runs.join(run);
    for dir in [project, &runs, &root] {
        if !dir
            .symlink_metadata()
            .with_context(|| format!("Couldn't inspect {}", dir.display()))?
            .is_dir()
        {
            return Err(anyhow!("Expected a directory: {}", dir.display()));
        }
    }
    files::resolve_contained(project, &format!("runs/{run}"))
}

fn image_mime(path: &str) -> Option<&'static str> {
    let mime = files::content_type_for_path(path);
    matches!(mime, "image/png" | "image/jpeg" | "image/webp").then_some(mime)
}

#[derive(Serialize)]
struct Image {
    path: String,
    version: String,
}

#[derive(Default, Serialize)]
struct Gallery {
    items: Vec<Image>,
    skipped: usize,
    truncated: bool,
}

fn scan(run: &Path, glob: &str, last: usize) -> Result<Gallery> {
    let (parent, name) = glob.rsplit_once('/').unwrap_or(("", glob));
    let (prefix, suffix) = name.split_once('*').expect("compiled glob");
    let dir = if parent.is_empty() {
        run.to_path_buf()
    } else {
        files::resolve_contained(run, parent)?
    };
    let mut gallery = Gallery::default();
    let mut newest = Vec::new();
    let entries = fs::read_dir(&dir).with_context(|| format!("Couldn't list {}", dir.display()))?;
    for (index, entry) in entries.enumerate() {
        if index == MAX_ENTRIES {
            gallery.truncated = true;
            break;
        }
        let entry = entry.with_context(|| format!("Couldn't read {}", dir.display()))?;
        let filename = entry.file_name();
        let Some(name) = filename.to_str().filter(|name| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| rest.ends_with(suffix))
        }) else {
            continue;
        };
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Couldn't inspect {}", entry.path().display()))
            }
        };
        if metadata.is_dir() {
            continue;
        }
        let path = if parent.is_empty() {
            name.to_owned()
        } else {
            format!("{parent}/{name}")
        };
        if !safe_path(&path)
            || image_mime(&path).is_none()
            || !metadata.is_file()
            || !(1..=MAX_BYTES).contains(&metadata.len())
        {
            gallery.skipped += 1;
            continue;
        }
        let modified = metadata.modified().with_context(|| {
            format!(
                "Couldn't read the modification time of {}",
                entry.path().display()
            )
        })?;
        newest.push((modified, path, metadata.len()));
        newest.sort_unstable_by(|a, b| b.cmp(a));
        newest.truncate(last);
    }
    gallery.items = newest
        .into_iter()
        .map(|(modified, path, size)| Image {
            path,
            version: artifact_version(size, modified),
        })
        .collect();
    Ok(gallery)
}

pub(super) fn backend(store: &Store, run: &StoredRun) -> Result<String> {
    let state = store
        .engine_run_state(&run.id)?
        .filter(|_| super::is_engine_row(run));
    let launch = state.as_ref().and_then(|state| state.launch.as_ref());
    let descriptor = crate::store::public_backend(&run.backend_json, launch);
    Ok(descriptor
        .as_ref()
        .and_then(|backend| backend["kind"].as_str())
        .unwrap_or_default()
        .to_owned())
}

pub(super) fn project(store: &Store, run: &StoredRun, backend: &str, view: &Value) -> Value {
    let mut requested = std::collections::BTreeMap::<&str, usize>::new();
    for block in view["blocks"].as_array().into_iter().flatten() {
        if let Some(glob) = block["media"].as_str() {
            let last = block["last"].as_u64().unwrap_or(LAST as u64) as usize;
            let kept = requested.entry(glob).or_default();
            *kept = (*kept).max(last);
        }
    }
    if requested.is_empty() {
        return json!({});
    }
    let root = (backend == "local_job").then(|| {
        let project = store
            .get_local_project(&run.project_id)?
            .ok_or_else(|| anyhow!("Project {} was not found", run.project_id))?;
        open_run(&files::files_dir(&project), &run.id)
    });
    let unreadable =
        |detail: &str| json!({"items": [], "error": {"kind": "unreadable", "detail": detail}});
    let failure = |error: &Error| {
        if error
            .downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::NotFound)
        {
            json!(Gallery::default())
        } else {
            unreadable(&format!("{error:#}"))
        }
    };
    requested
        .into_iter()
        .map(|(glob, last)| {
            let gallery = match &root {
                None if backend.is_empty() => unreadable("Couldn't determine the run's backend"),
                None => json!({"items": [], "error": {"kind": "unsupported", "backend": backend}}),
                Some(Err(error)) => failure(error),
                Some(Ok(root)) => scan(root, glob, last)
                    .map_or_else(|error| failure(&error), |gallery| json!(gallery)),
            };
            (glob.to_owned(), gallery)
        })
        .collect()
}

pub(crate) fn open_preview(
    store: &Store,
    run: &StoredRun,
    path: &str,
    expected: &str,
) -> Result<Option<(&'static str, Vec<u8>)>> {
    if backend(store, run)? != "local_job" {
        return Ok(None);
    }
    let Some(project) = store.get_local_project(&run.project_id)? else {
        return Ok(None);
    };
    let base = files::files_dir(&project);
    match open_run(&base, &run.id).and_then(|root| read_preview(&root, path, expected)) {
        Err(error)
            if error.downcast_ref::<io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                )
            }) =>
        {
            Ok(None)
        }
        preview => preview.with_context(|| format!("Couldn't open run {} image {path}", run.id)),
    }
}

fn read_preview(
    root: &Path,
    path: &str,
    expected: &str,
) -> Result<Option<(&'static str, Vec<u8>)>> {
    let Some(mime) = image_mime(path).filter(|_| safe_path(path)) else {
        return Ok(None);
    };
    if !root.join(path).symlink_metadata()?.is_file() {
        return Ok(None);
    }
    let resolved = files::resolve_contained(root, path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(resolved)?;
    let metadata = file.metadata()?;
    let (size, modified) = (metadata.len(), metadata.modified()?);
    if !metadata.is_file()
        || !(1..=MAX_BYTES).contains(&size)
        || artifact_version(size, modified) != expected
    {
        return Ok(None);
    }

    // Serve the bytes whose dimensions were checked, even if the writer replaces the file.
    let mut bytes = Vec::with_capacity(size as usize + 1);
    let mut capped = file.take(size + 1);
    capped.read_to_end(&mut bytes)?;
    let after = capped.get_ref().metadata()?;
    if bytes.len() as u64 != size || after.len() != size || after.modified()? != modified {
        return Ok(None);
    }
    if !dimensions(mime, &bytes).is_some_and(|(width, height)| {
        (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height)
    }) {
        return Ok(None);
    }
    Ok(Some((mime, bytes)))
}

fn dimensions(mime: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    match mime {
        "image/png"
            if bytes.len() >= 33 && bytes.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR") =>
        {
            let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
            let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
            Some((width, height))
        }
        "image/jpeg" => jpeg_dimensions(bytes.strip_prefix(b"\xff\xd8")?),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    }
}

fn jpeg_dimensions(mut bytes: &[u8]) -> Option<(u32, u32)> {
    while let Some(rest) = bytes.strip_prefix(b"\xff") {
        let (&marker, rest) = rest.split_first()?;
        if marker == 0xff {
            bytes = bytes.get(1..)?;
            continue;
        }
        if matches!(marker, 0 | 0xd8..=0xda) {
            return None;
        }
        let length = usize::from(u16::from_be_bytes(rest.get(..2)?.try_into().ok()?));
        let segment = rest.get(2..length)?;
        if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let height = u16::from_be_bytes(segment.get(1..3)?.try_into().ok()?);
            let width = u16::from_be_bytes(segment.get(3..5)?.try_into().ok()?);
            return Some((width.into(), height.into()));
        }
        bytes = rest.get(length..)?;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let riff = bytes.strip_prefix(b"RIFF")?;
    let size = u32::from_le_bytes(riff.get(..4)?.try_into().ok()?) as usize;
    let chunk = riff.get(4..)?.get(..size)?.strip_prefix(b"WEBP")?;
    let length = u32::from_le_bytes(chunk.get(4..8)?.try_into().ok()?) as usize;
    let data = chunk.get(8..)?.get(..length)?;
    match (chunk.get(..4)?, data) {
        (b"VP8 ", [tag, _, _, 0x9d, 0x01, 0x2a, w0, w1, h0, h1, ..]) if tag & 1 == 0 => {
            let width = u16::from_le_bytes([*w0, *w1]) & 0x3fff;
            let height = u16::from_le_bytes([*h0, *h1]) & 0x3fff;
            Some((width.into(), height.into()))
        }
        (b"VP8L", [0x2f, b0, b1, b2, b3, ..]) => {
            let packed = u32::from_le_bytes([*b0, *b1, *b2, *b3]);
            (packed >> 29 == 0).then_some(((packed & 0x3fff) + 1, ((packed >> 14) & 0x3fff) + 1))
        }
        (b"VP8X", [_, _, _, _, w0, w1, w2, h0, h1, h2]) => Some((
            u32::from_le_bytes([*w0, *w1, *w2, 0]) + 1,
            u32::from_le_bytes([*h0, *h1, *h2, 0]) + 1,
        )),
        _ => None,
    }
}

pub(crate) fn describe(
    glob: &str,
    gallery: &Value,
    last: usize,
    active: bool,
    run: &str,
) -> String {
    let images = gallery["items"]
        .as_array()
        .into_iter()
        .flatten()
        .take(last)
        .filter_map(|image| image["path"].as_str())
        .collect::<Vec<_>>();
    let mut words = match gallery["error"]["kind"].as_str() {
        Some("unsupported") => format!(
            "image previews aren't available for runs on {} yet",
            gallery["error"]["backend"]
                .as_str()
                .filter(|kind| !kind.is_empty())
                .unwrap_or("this backend")
        ),
        Some("unreadable") => format!(
            "couldn't read the image folder ({})",
            gallery["error"]["detail"].as_str().unwrap_or_default()
        ),
        _ if images.is_empty() && gallery["truncated"] == true => {
            "no previewable images in the entries scanned".into()
        }
        _ if images.is_empty() => format!(
            "no previewable images {} project artifacts under runs/{run}/{glob}",
            if active { "yet in" } else { "were found in" }
        ),
        _ => images.join(", "),
    };
    if gallery["truncated"] == true {
        words.push_str(&format!(
            ". Scan stopped after {MAX_ENTRIES} directory entries"
        ));
    }
    if let Some(skipped) = gallery["skipped"].as_u64().filter(|n| *n != 0) {
        words.push_str(&format!(
            ". {skipped} matching files could not be previewed"
        ));
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::git::TemporaryDirectory;

    #[test]
    fn media_scan_and_open_enforce_preview_bounds() {
        let temp = TemporaryDirectory::new("media-bounds").unwrap();
        let dir = temp.path().join("project/runs/r1");
        std::fs::create_dir_all(&dir).unwrap();
        let root = open_run(&temp.path().join("project"), "r1").unwrap();
        let too_large = std::fs::File::create(dir.join("large.png")).unwrap();
        too_large.set_len(MAX_BYTES + 1).unwrap();
        std::fs::write(dir.join("not-an-image.png"), b"plain text").unwrap();
        use base64::{engine::general_purpose::STANDARD, Engine as _};

        for (filename, encoded) in [
            (
                "sample.jpg",
                concat!(
                    "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8",
                    "UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDB",
                    "gNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMj",
                    "IyMjIyMjL/wAARCAABAAEDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAj/xAAU",
                    "EAEAAAAAAAAAAAAAAAAAAAAA/8QAFAEBAAAAAAAAAAAAAAAAAAAAAP/EABQRAQAAAAAAAA",
                    "AAAAAAAAAAAAD/2gAMAwEAAhEDEQA/AJ/AB//Z",
                ),
            ),
            (
                "sample.webp",
                "UklGRhoAAABXRUJQVlA4TA4AAAAvAAAAAAcQEf0PRET/Aw==",
            ),
            (
                "lossy.webp",
                "UklGRiQAAABXRUJQVlA4IBgAAAAwAQCdASoDAAUAAUAmJaQAA3AA/v02aAA=",
            ),
            (
                "alpha.webp",
                concat!(
                    "UklGRkgAAABXRUJQVlA4WAoAAAAQAAAABgAACAAAQUxQSAkAAAABBxAREYiI/gcAVlA4",
                    "IBgAAAAwAQCdASoHAAkAAUAmJaQAA3AA/v02aAA=",
                ),
            ),
            (
                "too-wide.png",
                concat!(
                    "iVBORw0KGgoAAAANSUhEUgAAEAEAAAABCAIAAAA+gZcVAAAAI0lEQVR4nO3BAQ0AAADCoP",
                    "dPbQ43oAAAAAAAAAAAAAAAgA4MMAQAAVBo6+cAAAAASUVORK5CYII=",
                ),
            ),
        ] {
            fs::write(dir.join(filename), STANDARD.decode(encoded).unwrap()).unwrap();
        }

        let gallery = scan(&root, "*", LAST).unwrap();
        assert_eq!(gallery.skipped, 1);
        assert_eq!(gallery.items.len(), 6);
        for item in gallery.items {
            let preview = read_preview(&root, &item.path, &item.version).unwrap();
            let expected = match item.path.as_str() {
                "sample.jpg" => Some("image/jpeg"),
                "sample.webp" | "lossy.webp" | "alpha.webp" => Some("image/webp"),
                _ => None,
            };
            assert_eq!(preview.map(|(mime, _)| mime), expected, "{}", item.path);
        }
        for index in 0..MAX_ENTRIES {
            std::fs::File::create(dir.join(format!("{index}.log"))).unwrap();
        }
        assert!(scan(&root, "*.png", LAST).unwrap().truncated);
    }

    #[cfg(unix)]
    #[test]
    fn media_paths_stay_under_the_run_during_scan_and_open() {
        use std::os::unix::fs::symlink;

        let temp = TemporaryDirectory::new("media-links").unwrap();
        let project = temp.path().join("project");
        let run = project.join("runs/r1");
        let sibling = project.join("runs/r2");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("outside.png"), [1]).unwrap();
        symlink(&sibling, run.join("outside")).unwrap();
        symlink(sibling.join("outside.png"), run.join("outside.png")).unwrap();
        symlink(&sibling, project.join("runs/alias")).unwrap();
        assert!(open_run(&project, "alias").is_err());

        let root = open_run(&project, "r1").unwrap();
        let gallery = scan(&root, "*.png", LAST).unwrap();
        assert!(gallery.items.is_empty());
        assert_eq!(gallery.skipped, 1);
        assert!(scan(&root, "outside/*.png", LAST).is_err());
        assert!(read_preview(&root, "outside.png", "").unwrap().is_none());
        assert!(read_preview(&root, "outside/outside.png", "").is_err());
    }
}
