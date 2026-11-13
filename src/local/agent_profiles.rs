use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{anyhow, bail, Context, Result};
use crate::local::native_store::{self, NativeStore};

static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    harness: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<Value>,
    created_at: i64,
}

pub fn harness_of(id: &str) -> Option<&'static str> {
    let (prefix, suffix) = id.split_once('-')?;
    if suffix.len() != 10
        || !suffix
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return None;
    }
    match prefix {
        "claude" => Some("claude-code"),
        "codex" => Some("codex"),
        _ => None,
    }
}

pub fn validate_icon(icon: &Value) -> Result<()> {
    let object = icon
        .as_object()
        .ok_or_else(|| anyhow!("Choose an agent icon"))?;
    // Without a color, the mark sits in a plain outline.
    let color = object.get("color");
    let known = color.is_none_or(|color| {
        color.as_str().is_some_and(|color| {
            [
                "gray", "red", "orange", "amber", "green", "teal", "blue", "indigo", "purple",
                "pink",
            ]
            .contains(&color)
        })
    });
    if !known || object.len() != 1 + usize::from(color.is_some()) {
        bail!("Choose an icon and color");
    }
    let valid = if let Some(brand) = object.get("brand") {
        brand == true
    } else if let Some(glyph) = object.get("glyph") {
        glyph.as_str().is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 40
                && s.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
    } else if let Some(emoji) = object.get("emoji") {
        emoji
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.chars().count() <= 16)
    } else if let Some(image) = object.get("image") {
        image.as_object().is_some_and(|o| o.len() == 2)
            && matches!(image["type"].as_str(), Some("image/webp" | "image/png"))
            && image["data"].as_str().is_some_and(|data| {
                data.len() <= 4 * (65536_usize.div_ceil(3))
                    && base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .is_ok_and(|bytes| !bytes.is_empty() && bytes.len() <= 65536)
            })
    } else {
        false
    };
    if !valid {
        bail!("Choose an icon, emoji, or PNG or WebP image up to 64 KiB");
    }
    Ok(())
}

/// An agent's name, trimmed; every row's name keeps to the 80 characters its input takes.
pub fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Enter an agent name");
    }
    if name.chars().count() > 80 {
        bail!("Agent names must be at most 80 characters");
    }
    Ok(name)
}

pub fn validate_identity(name: &str, icon: Option<&Value>) -> Result<()> {
    validate_name(name)?;
    icon.map(validate_icon).transpose()?;
    Ok(())
}

/// A change to a row's name or icon; `null` clears a found agent's name back to its own, and any row's icon.
#[derive(Deserialize)]
pub struct Edit {
    #[serde(default, deserialize_with = "present")]
    name: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    icon: Option<Option<Value>>,
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl Edit {
    /// Applies this edit to a record that always has a name; true when it renames the record.
    pub fn apply(&self, name: &mut String, icon: &mut Option<Value>) -> Result<bool> {
        if let Some(new) = &self.name {
            *name = validate_name(new.as_deref().unwrap_or_default())?.to_owned();
        }
        if let Some(new) = &self.icon {
            new.as_ref().map(validate_icon).transpose()?;
            icon.clone_from(new);
        }
        Ok(self.name.is_some())
    }
}

/// Writes the name and icon of the Settings row `key`: a found agent's id, an account's id, or
/// `opencode:` and a provider's or endpoint's id. True when OpenCode's model names carry it.
pub fn edit(key: &str, edit: &Edit) -> Result<bool> {
    if let Some(id) = key.strip_prefix("opencode:") {
        return crate::local::local_models::edit(id, edit);
    }
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if harness_of(key).is_some() {
        let mut profiles = read()?;
        let profile = profiles
            .get_mut(key)
            .ok_or_else(|| anyhow!("Agent account was removed"))?;
        edit.apply(&mut profile.name, &mut profile.icon)?;
        save(&profiles)?;
    } else {
        label(key, edit)?;
    }
    Ok(false)
}

fn read() -> Result<BTreeMap<String, Profile>> {
    let path = crate::store::data_dir().join("agent-profiles.json");
    let profiles: BTreeMap<String, Profile> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("Could not parse agent accounts in {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Could not read agent accounts in {}", path.display()))
        }
    };
    for (id, profile) in &profiles {
        if harness_of(id) != Some(profile.harness.as_str()) {
            bail!("Agent profile has an incorrect id: {id}");
        }
        validate_identity(&profile.name, profile.icon.as_ref())?;
    }
    Ok(profiles)
}

fn save(profiles: &BTreeMap<String, Profile>) -> Result<()> {
    crate::local::git::atomic_write_with_mode(
        &crate::store::data_dir().join("agent-profiles.json"),
        &serde_json::to_vec_pretty(profiles)?,
        Some(0o600),
    )
}

fn labels() -> Result<BTreeMap<String, Value>> {
    match std::fs::read(crate::store::data_dir().join("agent-labels.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}

fn label(id: &str, edit: &Edit) -> Result<()> {
    if !matches!(
        id,
        "claude-code" | "codex" | "opencode" | "cursor" | "antigravity"
    ) && !crate::local::acp::registered_definitions()
        .iter()
        .any(|definition| definition.id == id)
    {
        bail!("Choose an existing agent");
    }
    let mut labels = labels()?;
    let entry = labels
        .entry(id.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();
    if let Some(name) = &edit.name {
        match name {
            Some(name) => entry.insert("name".into(), json!(validate_name(name)?)),
            None => entry.remove("name"),
        };
    }
    if let Some(icon) = &edit.icon {
        match icon {
            Some(icon) => {
                validate_icon(icon)?;
                entry.insert("icon".into(), icon.clone())
            }
            None => entry.remove("icon"),
        };
    }
    if entry.is_empty() {
        labels.remove(id);
    }
    std::fs::create_dir_all(crate::store::data_dir())?;
    crate::local::git::atomic_write_with_mode(
        &crate::store::data_dir().join("agent-labels.json"),
        &serde_json::to_vec_pretty(&labels)?,
        Some(0o600),
    )
}

pub fn create(harness: &str, name: &str, icon: Option<Value>) -> Result<String> {
    let prefix = match harness {
        "claude-code" => "claude",
        "codex" => "codex",
        _ => bail!("Choose Claude Code or Codex for this account"),
    };
    let name = validate_name(name)?;
    icon.as_ref().map(validate_icon).transpose()?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut profiles = read()?;
    let id = format!(
        "{prefix}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    );
    let root = crate::store::data_dir().join("agents/profiles").join(&id);
    if root.symlink_metadata().is_ok() {
        bail!("Agent account already exists. Try adding it again.");
    }
    if let Err(error) = prepare_home(&root, harness) {
        let _ = std::fs::remove_dir_all(&root);
        return Err(error);
    }
    profiles.insert(
        id.clone(),
        Profile {
            harness: harness.to_string(),
            name: name.to_string(),
            icon,
            created_at: crate::store::now_ms(),
        },
    );
    if let Err(error) = save(&profiles) {
        let _ = std::fs::remove_dir_all(&root);
        return Err(error);
    }
    Ok(id)
}

fn removed(harness: &str) -> crate::error::Error {
    let name = if harness == "claude-code" {
        "Claude Code"
    } else {
        "OpenAI Codex"
    };
    anyhow!("This chat's {name} account was removed. Choose another agent.")
}

pub fn home(id: &str, harness: &str) -> Result<PathBuf> {
    if harness_of(id) != Some(harness) {
        return Err(removed(harness));
    }
    if read()?
        .get(id)
        .is_none_or(|profile| profile.harness != harness)
    {
        return Err(removed(harness));
    }
    let root = crate::store::data_dir().join("agents/profiles").join(id);
    match root.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => bail!("Agent account home {} must be a directory", root.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(removed(harness)),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Could not read agent account home {}", root.display()))
        }
    }
    Ok(root)
}

pub fn prepare(id: &str, harness: &str) -> Result<PathBuf> {
    let root = home(id, harness)?;
    prepare_home(&root, harness)?;
    Ok(root)
}

pub fn login_env(id: &str) -> Result<Vec<(&'static str, OsString)>> {
    let harness = harness_of(id).ok_or_else(|| anyhow!("Choose an existing agent account"))?;
    let home = prepare(id, harness)?;
    Ok(if harness == "claude-code" {
        vec![
            ("CLAUDE_CONFIG_DIR", home.clone().into_os_string()),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", home.into_os_string()),
        ]
    } else {
        vec![("CODEX_HOME", home.into_os_string())]
    })
}

fn private_file(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => {
            if !m.is_file() || m.file_type().is_symlink() {
                bail!("Agent account file must be private: {}", path.display());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if m.nlink() != 1 {
                    bail!("Agent account file must not be linked: {}", path.display());
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                use windows_sys::Win32::Storage::FileSystem::{
                    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
                };
                let file = std::fs::File::open(path)?;
                let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
                // The file handle is live and the record has room for the complete Win32 result.
                if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) }
                    == 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                // A successful call initialized the complete record.
                if unsafe { info.assume_init() }.nNumberOfLinks != 1 {
                    bail!("Agent account file must not be linked: {}", path.display());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn prepare_home(root: &Path, harness: &str) -> Result<()> {
    let legacy = if harness == "claude-code" {
        native_store::claude_home(NativeStore::Legacy)
    } else {
        native_store::codex_home(NativeStore::Legacy)
    };
    let isolated = if harness == "claude-code" {
        native_store::claude_home(NativeStore::Isolated)
    } else {
        native_store::codex_home(NativeStore::Isolated)
    };
    prepare_home_in(
        root,
        harness,
        &legacy,
        &isolated,
        &native_store::claude_config_file(),
    )
}

fn prepare_home_in(
    root: &Path,
    harness: &str,
    legacy: &Path,
    isolated: &Path,
    config: &Path,
) -> Result<()> {
    for directory in [root.parent().unwrap(), root] {
        if let Ok(m) = directory.symlink_metadata() {
            if !m.is_dir() || m.file_type().is_symlink() {
                bail!("Agent account home must be a directory");
            }
        }
        std::fs::create_dir_all(directory)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
    }
    let trees: &[&str] = if harness == "claude-code" {
        private_file(&root.join(".claude.json"))?;
        private_file(&root.join(".credentials.json"))?;
        native_store::prepare_claude_links(root, legacy, None)?;
        let path = root.join(".claude.json");
        let mut settings: Value = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                json!({"hasCompletedOnboarding": true})
            }
            Err(e) => return Err(e.into()),
        };
        let main: Value = match std::fs::read(config) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e.into()),
        };
        let object = settings
            .as_object_mut()
            .ok_or_else(|| anyhow!("Claude Code account settings must be an object"))?;
        // Claude rewrites this file while it runs, so it is only written when the servers changed.
        if object.get("mcpServers") != main.get("mcpServers") || !path.exists() {
            match main.get("mcpServers") {
                Some(servers) => object.insert("mcpServers".to_string(), servers.clone()),
                None => object.remove("mcpServers"),
            };
            crate::local::git::atomic_write_with_mode(
                &path,
                &serde_json::to_vec_pretty(&settings)?,
                Some(0o600),
            )?;
        }
        &[
            "projects",
            "todos",
            "file-history",
            "plans",
            "shell-snapshots",
            "session-env",
        ]
    } else {
        private_file(&root.join("auth.json"))?;
        native_store::prepare_codex_links(root, legacy, false)?;
        &["sessions", "archived_sessions"]
    };
    let sources = trees
        .iter()
        .map(|tree| isolated.join(tree))
        .collect::<Vec<_>>();
    for path in &sources {
        std::fs::create_dir_all(path)?;
    }
    native_store::prepare_links(root, isolated, &sources)
}

pub fn remove(id: &str) -> Result<()> {
    let harness = harness_of(id).ok_or_else(|| anyhow!("Choose an existing agent account"))?;
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut profiles = read()?;
    if !profiles.contains_key(id) {
        return Err(removed(harness));
    }
    let root = crate::store::data_dir().join("agents/profiles").join(id);
    if root
        .parent()
        .unwrap()
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        bail!("Agent account homes must be a directory");
    }

    match root.symlink_metadata() {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(&root)?,
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(&root)?,
        Ok(_) => bail!("Agent account home must be a directory"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    profiles.remove(id);
    save(&profiles)
}

pub async fn entries(base: &[Value]) -> Result<Vec<Value>> {
    let profiles = read()?;
    let pending = profiles.into_iter().filter_map(|(id, profile)| {
        let own = base.iter().find(|entry| entry["id"] == profile.harness)?;
        Some(async move {
            let waiting = own["catalogPending"] == true;
            let bin = own["binPath"].as_str().map(Path::new);
            let version = own["version"].as_str();
            let auth = match home(&id, &profile.harness) {
                Ok(home) if !waiting && profile.harness == "claude-code" => {
                    crate::local::harness::claude::profile_auth(bin, &home, version).await
                }
                Ok(home) if !waiting => {
                    crate::local::harness::codex::profile_auth(bin, &home, version).await
                }
                // A provisional entry spawns nothing; its profiles wait for the fill with it.
                Ok(_) => unprobed(waiting),
                Err(error) => json!({
                    "authenticated": false,
                    "supportsSteering": false,
                    "loginEligible": true,
                    "authState": "unknown",
                    "authCheckFailed": true,
                    "agentNote": format!("Could not read agent account {}: {error:#}", crate::store::data_dir().join("agents/profiles").join(&id).display()),
                }),
            };
            profile_entry(own, &id, &profile, auth)
        })
    });
    Ok(futures::future::join_all(pending).await)
}

/// Brings a cached harness list in line with the records: a removed profile's entry goes, a
/// renamed one takes its name and icon, and one added since stands signed out until its
/// sign-in publishes.
pub fn reconcile(payload: &mut Value) -> Result<()> {
    let profiles = read()?;
    let labels = labels()?;
    let Some(harnesses) = payload["harnesses"].as_array_mut() else {
        return Ok(());
    };
    for entry in harnesses.iter_mut() {
        entry.as_object_mut().unwrap().remove("label");
        if entry.get("profile").is_none() {
            if let Some(label) = entry["id"].as_str().and_then(|id| labels.get(id)) {
                entry["label"] = label.clone();
            }
        }
    }
    harnesses.retain(|entry| {
        entry["profile"]["id"]
            .as_str()
            .is_none_or(|id| profiles.contains_key(id))
    });
    for (id, profile) in &profiles {
        if let Some(entry) = harnesses
            .iter_mut()
            .find(|entry| entry["profile"]["id"] == id.as_str())
        {
            entry["profile"] = identity(id, profile);
            continue;
        }
        let added = harnesses
            .iter()
            .find(|entry| entry["id"] == profile.harness.as_str() && entry.get("profile").is_none())
            .map(|own| profile_entry(own, id, profile, unprobed(own["catalogPending"] == true)));
        harnesses.extend(added);
    }
    Ok(())
}

/// The auth of a profile no probe has read: still checking beside a pending entry, else signed out.
fn unprobed(pending: bool) -> Value {
    json!({
        "authenticated": false,
        "supportsSteering": false,
        "loginEligible": true,
        "authState": if pending { "unknown" } else { "needsLogin" },
    })
}

/// Its harness's `own` entry as one profile, with that profile's `auth` in place of the local account's.
fn profile_entry(own: &Value, id: &str, profile: &Profile, auth: Value) -> Value {
    let mut entry = own.clone();
    let fields = entry.as_object_mut().unwrap();
    for key in [
        "label",
        "defaultModel",
        "authMethod",
        "authProvider",
        "account",
        "accountName",
        "org",
        "plan",
        "agentNote",
        "authCheckFailed",
        "needsConfigRepair",
    ] {
        fields.remove(key);
    }
    if let Value::Object(auth) = auth {
        fields.extend(auth);
    }
    let ready = entry["authenticated"] == true
        && entry["installed"] == true
        && entry["installBroken"] != true;
    entry["agentReady"] = json!(ready);
    entry["profile"] = identity(id, profile);
    let configured = entry["defaultModel"].as_str().map(str::to_owned);
    crate::local::harness::resolve_entry(&mut entry, configured.as_deref())
        .expect("profile models");
    entry
}

fn identity(id: &str, profile: &Profile) -> Value {
    let mut out = json!({"id": id, "name": profile.name});
    if let Some(icon) = &profile.icon {
        out["icon"] = icon.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_profile_homes_are_unknown_and_name_the_path() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let id = create("codex", "Unreadable", None).unwrap();
        let root = home(&id, "codex").unwrap();
        let parent = root.parent().unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        let base = vec![json!({"id":"codex","installed":true,"models":[]})];
        let entries = entries(&base).await.unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(entries[0]["authState"], "unknown");
        assert_eq!(entries[0]["agentReady"], false);
        assert!(entries[0]["agentNote"]
            .as_str()
            .unwrap()
            .contains(root.to_str().unwrap()));
        let error = crate::local::harness::selected_model(
            &json!({"harnesses":entries}),
            "codex",
            Some(&id),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains(root.to_str().unwrap()));
        let records = crate::store::data_dir().join("agent-profiles.json");
        std::fs::write(&records, "broken JSON").unwrap();
        assert!(home(&id, "codex")
            .unwrap_err()
            .to_string()
            .contains(records.to_str().unwrap()));
    }

    #[test]
    fn profile_ids_reject_paths_and_other_harnesses() {
        assert_eq!(harness_of("claude-012345abcd"), Some("claude-code"));
        assert_eq!(harness_of("codex-abcdefghij"), Some("codex"));
        for id in [
            "claude-123",
            "codex-ABCDEFGHIJ",
            "codex-0123456789/..",
            "../claude-012345abcd",
            "opencode-012345abcd",
            "claude-012345abc_",
        ] {
            assert_eq!(harness_of(id), None, "{id}");
        }
    }

    #[test]
    fn icon_union_and_size_are_validated() {
        for icon in [
            json!({"brand":true,"color":"orange"}),
            json!({"brand":true}),
            json!({"glyph":"flask-2","color":"teal"}),
            json!({"emoji":"🌱","color":"green"}),
            json!({"image":{"type":"image/png","data":"aGVsbG8="},"color":"gray"}),
        ] {
            validate_icon(&icon).unwrap();
        }
        for icon in [
            json!({"brand":false,"color":"blue"}),
            json!({"brand":true,"glyph":"x","color":"blue"}),
            json!({"glyph":"../x","color":"blue"}),
            json!({"emoji":"a".repeat(17),"color":"blue"}),
            json!({"brand":true,"color":"gold"}),
            json!({"brand":true,"color":null}),
            json!({"color":"blue"}),
            json!({"image":{"type":"image/png","data":base64::engine::general_purpose::STANDARD.encode(vec![0;65537])},"color":"blue"}),
        ] {
            assert!(validate_icon(&icon).is_err(), "{icon}");
        }
    }

    #[test]
    fn profile_homes_share_configuration_and_transcripts_but_keep_auth_private() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-profiles-test").unwrap();
        for harness in ["claude-code", "codex"] {
            let legacy = temp.path().join(harness).join("legacy");
            let isolated = temp.path().join(harness).join("isolated");
            let root = temp.path().join(harness).join("profiles/one");
            std::fs::create_dir_all(&legacy).unwrap();
            let config = legacy.join(".claude.json");
            std::fs::write(
                &config,
                br#"{"oauthAccount":{"emailAddress":"local"},"mcpServers":{"first":{}}}"#,
            )
            .unwrap();
            std::fs::write(legacy.join(".credentials.json"), "local-secret").unwrap();
            std::fs::write(legacy.join("auth.json"), "local-secret").unwrap();
            std::fs::write(legacy.join("settings.json"), "{}").unwrap();
            std::fs::write(legacy.join("config.toml"), "model = 'gpt-6'").unwrap();
            prepare_home_in(&root, harness, &legacy, &isolated, &config).unwrap();
            let (shared, trees, auth) = if harness == "claude-code" {
                (
                    "settings.json",
                    vec![
                        "projects",
                        "todos",
                        "file-history",
                        "plans",
                        "shell-snapshots",
                        "session-env",
                    ],
                    ".credentials.json",
                )
            } else {
                (
                    "config.toml",
                    vec!["sessions", "archived_sessions"],
                    "auth.json",
                )
            };
            assert!(same_file::is_same_file(root.join(shared), legacy.join(shared)).unwrap());
            assert!(!root.join(auth).exists());
            std::fs::write(root.join(auth), "private-secret").unwrap();
            for tree in &trees {
                assert!(same_file::is_same_file(root.join(tree), isolated.join(tree)).unwrap());
            }
            if harness == "claude-code" {
                let seeded: Value =
                    serde_json::from_slice(&std::fs::read(root.join(".claude.json")).unwrap())
                        .unwrap();
                assert_eq!(
                    seeded,
                    json!({"hasCompletedOnboarding":true,"mcpServers":{"first":{}}})
                );
                assert!(!root
                    .join(".claude.json")
                    .symlink_metadata()
                    .unwrap()
                    .file_type()
                    .is_symlink());
            }
            prepare_home_in(&root, harness, &legacy, &isolated, &config).unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join(auth)).unwrap(),
                "private-secret"
            );
            assert_eq!(
                std::fs::read_to_string(legacy.join(auth)).unwrap(),
                "local-secret"
            );
            let transcript = isolated.join(trees[0]).join("saved-session");
            std::fs::write(&transcript, "history").unwrap();
            std::fs::remove_dir_all(&root).unwrap();
            assert_eq!(std::fs::read_to_string(transcript).unwrap(), "history");
        }
    }

    #[test]
    fn claude_mcp_resync_preserves_account_and_other_settings() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-profiles-test").unwrap();
        let legacy = temp.path().join("legacy");
        let isolated = temp.path().join("isolated");
        let root = temp.path().join("profiles/one");
        std::fs::create_dir_all(&legacy).unwrap();
        let config = legacy.join(".claude.json");
        std::fs::write(&config, br#"{"mcpServers":{"old":{}}}"#).unwrap();
        prepare_home_in(&root, "claude-code", &legacy, &isolated, &config).unwrap();
        std::fs::write(
            root.join(".claude.json"),
            br#"{"oauthAccount":{"emailAddress":"private"},"custom":42,"mcpServers":{"old":{}}}"#,
        )
        .unwrap();
        std::fs::write(&config, br#"{"mcpServers":{"new":{}}}"#).unwrap();
        prepare_home_in(&root, "claude-code", &legacy, &isolated, &config).unwrap();
        let read = || {
            serde_json::from_slice::<Value>(&std::fs::read(root.join(".claude.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            read(),
            json!({"oauthAccount":{"emailAddress":"private"},"custom":42,"mcpServers":{"new":{}}})
        );
        std::fs::write(&config, "{}").unwrap();
        prepare_home_in(&root, "claude-code", &legacy, &isolated, &config).unwrap();
        assert_eq!(
            read(),
            json!({"oauthAccount":{"emailAddress":"private"},"custom":42})
        );
    }

    #[test]
    fn preexisting_private_links_are_refused() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-profiles-test").unwrap();
        for (harness, private) in [
            ("claude-code", ".claude.json"),
            ("claude-code", ".credentials.json"),
            ("codex", "auth.json"),
        ] {
            let legacy = temp.path().join("legacy");
            let isolated = temp.path().join("isolated");
            let root = temp.path().join(format!("profiles/{private}"));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&legacy).unwrap();
            let source = legacy.join(private);
            std::fs::write(&source, "{}").unwrap();
            native_store::create_symlink(&source, &root.join(private)).unwrap();
            assert!(prepare_home_in(
                &root,
                harness,
                &legacy,
                &isolated,
                &legacy.join(".claude.json")
            )
            .is_err());
            assert_eq!(std::fs::read_to_string(source).unwrap(), "{}");
        }
    }

    #[test]
    fn removed_profile_record_or_home_never_falls_back() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let gone = "This chat's Claude Code account was removed. Choose another agent.";
        assert_eq!(
            home("claude-012345abcd", "claude-code")
                .unwrap_err()
                .to_string(),
            gone
        );
        let id = create("claude-code", "Work", None).unwrap();
        let root = home(&id, "claude-code").unwrap();
        assert_eq!(
            home(&id, "codex").unwrap_err().to_string(),
            "This chat's OpenAI Codex account was removed. Choose another agent."
        );
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(home(&id, "claude-code").unwrap_err().to_string(), gone);
    }

    #[test]
    fn every_row_takes_the_same_name_and_icon_edit() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let edit = |key: &str, patch: Value| edit(key, &serde_json::from_value(patch).unwrap());
        let labelled = || {
            let mut payload = json!({"harnesses":[{"id":"codex"}]});
            reconcile(&mut payload).unwrap();
            payload["harnesses"][0].get("label").cloned()
        };
        for patch in [
            json!({"name":" "}),
            json!({"name":"x".repeat(81)}),
            json!({"icon":{"brand":false}}),
        ] {
            assert!(edit("codex", patch).is_err());
        }
        assert!(edit("codex-abcdefghij", json!({"name":"Gone"})).is_err());
        assert!(edit("opencode:gone", json!({"name":"Gone"})).is_err());
        edit("codex", json!({"name":" Local ","icon":{"brand":true}})).unwrap();
        assert_eq!(
            labelled(),
            Some(json!({"name":"Local","icon":{"brand":true}}))
        );
        edit("codex", json!({"name":null,"icon":null})).unwrap();
        assert_eq!(labelled(), None);

        let account = create("codex", "Personal", Some(json!({"brand":true}))).unwrap();
        assert!(edit(&account, json!({"name":null})).is_err());
        edit(&account, json!({"name":" Renamed ","icon":null})).unwrap();
        let mut payload = json!({"harnesses":[{"id":"codex","installed":true,"models":[]}]});
        reconcile(&mut payload).unwrap();
        assert_eq!(
            payload["harnesses"][1]["profile"],
            json!({"id": account, "name": "Renamed"})
        );

        let local_models = crate::local::local_models::providers_path();
        crate::local::local_models::record_oauth(
            &local_models,
            "github-copilot",
            "Copilot".into(),
            Some(json!({"brand":true})),
        )
        .unwrap();
        std::fs::write(
            crate::store::data_dir().join("local-models.json"),
            json!({"orx-local-1":{"name":"Box","icon":{"brand":true},"baseUrl":"http://127.0.0.1:1234/v1","models":{}}}).to_string(),
        )
        .unwrap();
        assert!(!edit(
            "opencode:github-copilot",
            json!({"name":"Work Copilot","icon":null})
        )
        .unwrap());
        assert!(edit(
            "opencode:orx-local-1",
            json!({"name":"Lab box","icon":null})
        )
        .unwrap());
        assert_eq!(
            crate::local::local_models::provider_list().unwrap()[0],
            json!({"id":"github-copilot","name":"Work Copilot","kind":"oauth"})
        );
        let endpoint = &crate::local::local_models::list().unwrap()[0];
        assert_eq!(
            (&endpoint["name"], &endpoint["icon"]),
            (&json!("Lab box"), &Value::Null)
        );
    }

    #[cfg(unix)]
    #[test]
    fn accounts_list_their_own_sign_in_from_their_own_home() {
        // The developer's own Claude credential and model would stand in for the fake account's.
        if !crate::config::in_test_child(&[]) {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let data = crate::store::data_dir();
        let claude = create(
            "claude-code",
            "Work",
            Some(json!({"brand":true,"color":"orange"})),
        )
        .unwrap();
        let codex = create("codex", "Personal", None).unwrap();
        let signed_out = create("codex", "Another", None).unwrap();
        let home = prepare(&claude, "claude-code").unwrap();
        assert_eq!(
            login_env(&claude).unwrap(),
            vec![
                ("CLAUDE_CONFIG_DIR", home.clone().into_os_string()),
                (
                    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
                    home.clone().into_os_string()
                )
            ]
        );
        std::fs::write(
            home.join(".claude.json"),
            br#"{"oauthAccount":{"emailAddress":"work@example.test","displayName":"Work"}}"#,
        )
        .unwrap();
        std::fs::write(
            home.join("settings.json"),
            br#"{"model":"configured-model"}"#,
        )
        .unwrap();
        std::fs::write(
            prepare(&codex, "codex").unwrap().join("auth.json"),
            br#"{"OPENAI_API_KEY":"test-key"}"#,
        )
        .unwrap();
        let stub = data.join("fake-claude");
        orx_test_support::write_executable(
            &stub,
            "#!/bin/sh\n[ \"$CLAUDE_CONFIG_DIR\" = \"$CLAUDE_SECURESTORAGE_CONFIG_DIR\" ] || exit 2\nprintf '%s\\n' '{\"loggedIn\":true,\"authMethod\":\"oauth\",\"subscriptionType\":\"max\"}'\n",
            0o700,
        )
        .unwrap();
        let base = vec![
            json!({"id":"claude-code","installed":true,"version":"2.1.211","binPath":stub,"models":[{"id":"sonnet"}],"options":{"test":true},"account":"local@example.test","label":{"name":"Mine"}}),
            json!({"id":"codex","installed":true,"models":[{"id":"gpt-6"}],"options":{}}),
        ];
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(entries(&base)).unwrap();
        let listed = |id: &str| {
            result
                .iter()
                .find(|entry| entry["profile"]["id"] == id)
                .unwrap()
        };
        let work = listed(&claude);
        assert_eq!(work["account"], "work@example.test");
        assert_eq!(work["agentReady"], true);
        assert!(work.get("label").is_none());
        assert_eq!(
            work["profile"]["icon"],
            json!({"brand":true,"color":"orange"})
        );
        assert_eq!(work["defaultModel"], "configured-model");
        assert_eq!(work["options"], base[0]["options"]);
        assert_eq!(listed(&codex)["authenticated"], true);
        assert_eq!(listed(&signed_out)["authState"], "needsLogin");
        assert_eq!(
            std::fs::metadata(data.join("agent-profiles.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let waiting = vec![
            json!({"id":"codex","installed":true,"catalogPending":true,"binPath":"/nonexistent"}),
        ];
        assert!(runtime
            .block_on(entries(&waiting))
            .unwrap()
            .iter()
            .all(|entry| entry["catalogPending"] == true && entry["authState"] == "unknown"));

        remove(&claude).unwrap();
        assert!(login_env(&claude).is_err());
        assert!(data.join("agents/claude/projects").is_dir());
        let added = create("codex", "Lab", None).unwrap();
        let mut payload =
            json!({ "harnesses": base.iter().chain(&result).cloned().collect::<Vec<_>>() });
        reconcile(&mut payload).unwrap();
        let harnesses = payload["harnesses"].as_array().unwrap();
        assert!(!harnesses
            .iter()
            .any(|entry| entry["profile"]["id"] == claude.as_str()));
        let lab = harnesses
            .iter()
            .find(|entry| entry["profile"]["id"] == added.as_str())
            .unwrap();
        assert_eq!(lab["authState"], "needsLogin");
        assert_eq!(lab["models"].as_array().unwrap().len(), 1);
        assert_eq!(lab["models"][0]["id"], "gpt-6");
    }
}
