//! Provider-native state used by chats launched through OpenResearch.
//!
//! New sessions live below the ORX data directory. Account and configuration
//! files remain owned by each provider's normal home and are linked into the
//! isolated Claude/Codex homes before every spawn.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{anyhow, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStore {
    Isolated,
    Legacy,
}

const MANIFEST: &str = ".openresearch-managed-links.json";
static MANAGED_LINK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Default, Deserialize, Serialize)]
struct LinkManifest {
    files: BTreeMap<String, String>,
}

fn user_env_path(key: &str) -> Option<PathBuf> {
    crate::local::shell_env::var(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            crate::config::synced_env_var(key)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn opencode_db(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir()
            .join("agents")
            .join("opencode")
            .join("opencode.db"),
        NativeStore::Legacy => user_env_path("OPENCODE_DB").unwrap_or_else(|| {
            let data = user_env_path("XDG_DATA_HOME")
                .unwrap_or_else(|| home_dir().join(".local").join("share"));
            data.join("opencode").join("opencode.db")
        }),
    }
}

pub fn claude_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents").join("claude"),
        NativeStore::Legacy => {
            user_env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home_dir().join(".claude"))
        }
    }
}

pub fn codex_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents").join("codex"),
        NativeStore::Legacy => {
            user_env_path("CODEX_HOME").unwrap_or_else(|| home_dir().join(".codex"))
        }
    }
}

pub fn opencode_stores_are_distinct() -> bool {
    opencode_db(NativeStore::Isolated) != opencode_db(NativeStore::Legacy)
}

pub fn claude_stores_are_distinct() -> bool {
    claude_home(NativeStore::Isolated) != claude_home(NativeStore::Legacy)
}

pub fn codex_stores_are_distinct() -> bool {
    codex_home(NativeStore::Isolated) != codex_home(NativeStore::Legacy)
}

pub fn codex_sqlite_override(store: NativeStore, home: &Path) -> Option<String> {
    (store == NativeStore::Isolated).then(|| {
        let path = toml_string(home);
        format!("sqlite_home={path}")
    })
}

/// Quote a path for a TOML `-c` override; JSON escaping is compatible except for DEL.
pub fn toml_string(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy())
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace('\u{7f}', "\\u007F")
}

pub fn prepare_opencode(store: NativeStore) -> Result<PathBuf> {
    let db = opencode_db(store);
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(db)
}

pub fn prepare_claude(store: NativeStore) -> Result<PathBuf> {
    let root = claude_home(store);
    if store == NativeStore::Legacy || !claude_stores_are_distinct() {
        std::fs::create_dir_all(&root)
            .map_err(|error| anyhow!("could not create {}: {error}", root.display()))?;
        return Ok(root);
    }
    let legacy = claude_home(NativeStore::Legacy);
    let mut links = vec![
        (legacy.join("settings.json"), PathBuf::from("settings.json")),
        (
            legacy.join("settings.local.json"),
            PathBuf::from("settings.local.json"),
        ),
        (legacy.join("CLAUDE.md"), PathBuf::from("CLAUDE.md")),
        (
            legacy.join(".credentials.json"),
            PathBuf::from(".credentials.json"),
        ),
        (legacy.join("plugins"), PathBuf::from("plugins")),
        (legacy.join("skills"), PathBuf::from("skills")),
    ];
    links.push((
        home_dir().join(".claude.json"),
        PathBuf::from(".claude.json"),
    ));
    reconcile_links(&root, &links)?;
    Ok(root.canonicalize().unwrap_or(root))
}

pub fn prepare_codex(store: NativeStore) -> Result<PathBuf> {
    let root = codex_home(store);
    if store == NativeStore::Legacy || !codex_stores_are_distinct() {
        std::fs::create_dir_all(&root)
            .map_err(|error| anyhow!("could not create {}: {error}", root.display()))?;
        return Ok(root);
    }
    let legacy = codex_home(NativeStore::Legacy);
    let mut links = vec![
        (legacy.join("auth.json"), PathBuf::from("auth.json")),
        (legacy.join("config.toml"), PathBuf::from("config.toml")),
        (legacy.join("AGENTS.md"), PathBuf::from("AGENTS.md")),
        (legacy.join("plugins"), PathBuf::from("plugins")),
        (legacy.join("skills"), PathBuf::from("skills")),
        (legacy.join("prompts"), PathBuf::from("prompts")),
        (legacy.join("packages"), PathBuf::from("packages")),
    ];
    if let Ok(entries) = std::fs::read_dir(&legacy) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_text) = name.to_str() else {
                continue;
            };
            if name_text.ends_with(".config.toml") {
                links.push((entry.path(), PathBuf::from(name)));
            }
        }
    }
    reconcile_links(&root, &links)?;
    Ok(root.canonicalize().unwrap_or(root))
}

fn reconcile_links(root: &Path, links: &[(PathBuf, PathBuf)]) -> Result<()> {
    let _guard = MANAGED_LINK_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    std::fs::create_dir_all(root)
        .map_err(|error| anyhow!("could not create {}: {error}", root.display()))?;
    let lock_path = root.join(".openresearch-managed-links.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|error| anyhow!("could not open {}: {error}", lock_path.display()))?;
    let mut process_lock = fd_lock::RwLock::new(lock_file);
    let _process_guard = process_lock
        .write()
        .map_err(|error| anyhow!("could not lock {}: {error}", lock_path.display()))?;
    let manifest_path = root.join(MANIFEST);
    let mut manifest = match std::fs::read(&manifest_path) {
        Ok(bytes) => match serde_json::from_slice::<LinkManifest>(&bytes) {
            Ok(manifest) => manifest,
            Err(error) => {
                let preserved = preserve_conflict(&manifest_path)?;
                eprintln!(
                    "orx: preserved unreadable managed-link manifest at {}: {error}",
                    preserved.display()
                );
                LinkManifest::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LinkManifest::default(),
        Err(error) => {
            return Err(anyhow!(
                "could not read {}: {error}",
                manifest_path.display()
            ));
        }
    };
    for (source, relative) in links {
        reconcile_link(root, source, relative, &mut manifest)?;
    }
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    atomic_write(&manifest_path, &bytes)?;
    Ok(())
}

fn reconcile_link(
    root: &Path,
    source: &Path,
    relative: &Path,
    manifest: &mut LinkManifest,
) -> Result<()> {
    let destination = root.join(relative);
    if source == destination {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| anyhow!("could not create {}: {error}", parent.display()))?;
    }
    let key = relative.to_string_lossy().into_owned();
    let source_hash = file_hash(source)?;
    let source_exists = std::fs::symlink_metadata(source).is_ok();
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = std::fs::read_link(&destination).map_err(|error| {
                anyhow!("could not read link {}: {error}", destination.display())
            })?;
            if target != source {
                let preserved = preserve_conflict(&destination)?;
                eprintln!(
                    "orx: preserved unexpected config link {} at {}",
                    destination.display(),
                    preserved.display()
                );
            } else if !source_exists {
                remove_path(&destination)?;
            }
        }
        Ok(metadata) if metadata.is_file() => {
            let destination_hash = file_hash(&destination)?.unwrap_or_default();
            match source_hash.as_deref() {
                None => {
                    manifest.files.remove(&key);
                    return Ok(());
                }
                Some(source_hash) if source_hash == destination_hash => {
                    remove_path(&destination)?;
                }
                Some(source_hash) => {
                    let baseline = manifest.files.get(&key).map(String::as_str);
                    if baseline == Some(source_hash) {
                        atomic_replace_from(&destination, source)?;
                        remove_path(&destination)?;
                    } else if baseline == Some(destination_hash.as_str()) {
                        remove_path(&destination)?;
                    } else {
                        let preserved = preserve_conflict(&destination)?;
                        eprintln!(
                            "orx: preserved conflicting config update at {}; using {}",
                            preserved.display(),
                            source.display()
                        );
                    }
                }
            }
        }
        Ok(_) => {
            if !source_exists {
                manifest.files.remove(&key);
                return Ok(());
            }
            let preserved = preserve_conflict(&destination)?;
            eprintln!(
                "orx: preserved conflicting config path at {}; using {}",
                preserved.display(),
                source.display()
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(anyhow!(
                "could not inspect {}: {error}",
                destination.display()
            ));
        }
    }
    if source_exists && std::fs::symlink_metadata(&destination).is_err() {
        create_symlink(source, &destination).map_err(|error| {
            anyhow!(
                "could not link {} to {}: {error}",
                destination.display(),
                source.display()
            )
        })?;
    }
    match file_hash(source)? {
        Some(hash) => {
            manifest.files.insert(key, hash);
        }
        None => {
            manifest.files.remove(&key);
        }
    }
    Ok(())
}

fn file_hash(path: &Path) -> Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => return Ok(None),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(format!("{:x}", Sha256::digest(bytes)))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    let temporary = parent.join(format!(
        ".openresearch-managed-links-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    if let Err(error) = std::fs::write(&temporary, bytes) {
        let _ = std::fs::remove_file(&temporary);
        return Err(anyhow!("could not write {}: {error}", path.display()));
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(anyhow!("could not replace {}: {error}", path.display()));
    }
    Ok(())
}

fn atomic_replace_from(new_content: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", target.display()))?;
    let temporary = parent.join(format!(
        ".openresearch-managed-config-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    if let Err(error) = std::fs::copy(new_content, &temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(anyhow!(
            "could not copy {} to {}: {error}",
            new_content.display(),
            target.display()
        ));
    }
    if let Err(error) = std::fs::rename(&temporary, target) {
        let _ = std::fs::remove_file(&temporary);
        return Err(anyhow!(
            "could not replace {} from {}: {error}",
            target.display(),
            new_content.display()
        ));
    }
    Ok(())
}

fn preserve_conflict(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let preserved = path.with_file_name(format!("{name}.orx-conflict-{}", uuid::Uuid::new_v4()));
    std::fs::rename(path, &preserved).map_err(|error| {
        anyhow!(
            "could not preserve conflicting path {} as {}: {error}",
            path.display(),
            preserved.display()
        )
    })?;
    Ok(preserved)
}

#[cfg(not(windows))]
fn remove_path(path: &Path) -> Result<()> {
    std::fs::remove_file(path)
        .map_err(|error| anyhow!("could not remove {}: {error}", path.display()))
}

#[cfg(windows)]
fn remove_path(path: &Path) -> Result<()> {
    use std::os::windows::fs::FileTypeExt;

    let file_type = std::fs::symlink_metadata(path)
        .map_err(|error| anyhow!("could not inspect {}: {error}", path.display()))?
        .file_type();
    let result = if file_type.is_symlink_dir() {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|error| anyhow!("could not remove {}: {error}", path.display()))
}

#[cfg(unix)]
fn create_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
fn create_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::os::windows::fs::symlink_dir(source, destination)
    } else {
        std::os::windows::fs::symlink_file(source, destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("orx-native-store-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn isolated_provider_paths_share_the_orx_data_root() {
        let root = crate::store::data_dir();
        assert_eq!(
            opencode_db(NativeStore::Isolated),
            root.join("agents/opencode/opencode.db")
        );
        assert_eq!(
            claude_home(NativeStore::Isolated),
            root.join("agents/claude")
        );
        assert_eq!(codex_home(NativeStore::Isolated), root.join("agents/codex"));
        assert!(
            codex_sqlite_override(NativeStore::Isolated, &codex_home(NativeStore::Isolated))
                .unwrap()
                .contains("agents/codex")
        );
        assert!(
            codex_sqlite_override(NativeStore::Legacy, &codex_home(NativeStore::Legacy)).is_none()
        );
    }

    #[test]
    fn managed_file_adopts_an_isolated_atomic_update() {
        let root = temp_root("adopt");
        let source = root.join("legacy/config.toml");
        let isolated = root.join("isolated");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "old").unwrap();
        reconcile_links(&isolated, &[(source.clone(), PathBuf::from("config.toml"))]).unwrap();
        std::fs::remove_file(isolated.join("config.toml")).unwrap();
        std::fs::write(isolated.join("config.toml"), "new").unwrap();

        reconcile_links(&isolated, &[(source.clone(), PathBuf::from("config.toml"))]).unwrap();

        assert_eq!(std::fs::read_to_string(&source).unwrap(), "new");
        assert!(std::fs::symlink_metadata(isolated.join("config.toml"))
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn managed_directory_stays_shared() {
        let root = temp_root("directory");
        let source = root.join("legacy/plugins");
        let isolated = root.join("isolated");
        std::fs::create_dir_all(&source).unwrap();

        reconcile_links(&isolated, &[(source.clone(), PathBuf::from("plugins"))]).unwrap();
        std::fs::write(isolated.join("plugins/example.txt"), "shared").unwrap();

        assert_eq!(
            std::fs::read_to_string(source.join("example.txt")).unwrap(),
            "shared"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn managed_file_preserves_two_divergent_updates() {
        let root = temp_root("conflict");
        let source = root.join("legacy/config.toml");
        let isolated = root.join("isolated");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "old").unwrap();
        reconcile_links(&isolated, &[(source.clone(), PathBuf::from("config.toml"))]).unwrap();
        std::fs::remove_file(isolated.join("config.toml")).unwrap();
        std::fs::write(&source, "native change").unwrap();
        std::fs::write(isolated.join("config.toml"), "isolated change").unwrap();

        reconcile_links(&isolated, &[(source.clone(), PathBuf::from("config.toml"))]).unwrap();

        assert_eq!(std::fs::read_to_string(&source).unwrap(), "native change");
        assert!(std::fs::symlink_metadata(isolated.join("config.toml"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(std::fs::read_dir(&isolated)
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.orx-conflict-")
            }));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn absent_legacy_file_stays_isolated_without_a_dangling_link() {
        let root = temp_root("absent");
        let source = root.join("legacy/config.toml");
        let isolated = root.join("isolated");

        reconcile_links(&isolated, &[(source.clone(), PathBuf::from("config.toml"))]).unwrap();

        assert!(!source.exists());
        assert!(std::fs::symlink_metadata(isolated.join("config.toml")).is_err());
        std::fs::write(isolated.join("config.toml"), "isolated").unwrap();
        reconcile_links(&isolated, &[(source.clone(), PathBuf::from("config.toml"))]).unwrap();
        assert_eq!(
            std::fs::read_to_string(isolated.join("config.toml")).unwrap(),
            "isolated"
        );
        assert!(!source.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn managed_directory_preserves_an_existing_isolated_directory() {
        let root = temp_root("directory-conflict");
        let source = root.join("legacy/plugins");
        let isolated = root.join("isolated");
        std::fs::create_dir_all(isolated.join("plugins")).unwrap();
        std::fs::write(isolated.join("plugins/local.txt"), "isolated").unwrap();
        std::fs::create_dir_all(&source).unwrap();

        reconcile_links(&isolated, &[(source.clone(), PathBuf::from("plugins"))]).unwrap();

        assert!(std::fs::symlink_metadata(isolated.join("plugins"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(std::fs::read_dir(&isolated)
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("plugins.orx-conflict-")
            }));
        std::fs::remove_dir_all(root).ok();
    }
}
