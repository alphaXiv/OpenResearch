//! Directory browsing for the loopback dashboard.

use crate::error::{anyhow, Result};

#[cfg(windows)]
fn drive_roots(mask: u32) -> Vec<String> {
    (0..26)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| format!("{}:\\", char::from(b'A' + bit)))
        .collect()
}

fn roots() -> Result<Vec<String>> {
    #[cfg(windows)]
    {
        // Only enumerate drive letters; do not probe removable/network media.
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        if mask == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(drive_roots(mask))
    }
    #[cfg(not(windows))]
    {
        Ok(vec!["/".into()])
    }
}

/// List directories without invoking Windows shell extensions or modal dialogs.
pub fn browse(path: &str) -> Result<serde_json::Value> {
    let path = path.trim();
    if path.is_empty() {
        let folders: Vec<_> = roots()?
            .into_iter()
            .map(|path| serde_json::json!({ "name": path, "path": path }))
            .collect();
        return Ok(serde_json::json!({ "path": null, "parent": null, "folders": folders }));
    }
    // The browser's contract is full paths; a relative path would silently
    // resolve against the server's working directory.
    if !(path == "~" || path.starts_with("~/") || std::path::Path::new(path).is_absolute()) {
        return Err(anyhow!("Enter a full folder path"));
    }
    let path = crate::local::projects::expand_path(path)?;
    // Resolve .. and links, and strip Windows' verbatim prefix for display.
    let path = crate::paths::canonicalize(path)?;
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(&path)? {
        // Entries may disappear while the user is browsing.
        let Ok(entry) = entry else { continue };
        // Do not follow links while listing: disconnected network targets must
        // not block browsing their parent. Windows already reports directory
        // links as is_dir(), so a non-dir link points at a file or a dead
        // target; on Unix links cannot be classified without following, so a
        // bad one is listed and fails cleanly when opened.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !(kind.is_dir() || (!cfg!(windows) && kind.is_symlink())) {
            continue;
        }
        // Paths that cannot round-trip through JSON cannot be selected.
        let entry_path = entry.path();
        if entry_path.to_str().is_some() {
            folders.push(entry_path);
        }
    }
    folders.sort_by_cached_key(|p| p.file_name().unwrap_or_default().to_ascii_lowercase());
    let display_path = path
        .to_str()
        .ok_or_else(|| anyhow!("The folder path is not valid UTF-8"))?;
    Ok(serde_json::json!({
        "path": display_path,
        "parent": path.parent().and_then(|p| p.to_str()),
        "folders": folders.iter().map(|p| serde_json::json!({
            "name": p.file_name().unwrap_or_default().to_string_lossy(),
            "path": p.to_string_lossy(),
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_path_lists_filesystem_roots() {
        let listing = super::browse("").unwrap();
        assert!(listing["path"].is_null());
        assert!(listing["parent"].is_null());
        assert!(!listing["folders"].as_array().unwrap().is_empty());
        #[cfg(not(windows))]
        assert_eq!(listing["folders"][0]["path"], "/");
    }

    #[cfg(windows)]
    #[test]
    fn drive_selection_is_not_restricted_to_system_drive() {
        assert_eq!(
            super::drive_roots((1 << 2) | (1 << 8) | (1 << 25)),
            vec!["C:\\", "I:\\", "Z:\\"]
        );
    }
    #[test]
    fn browser_lists_directories_preserves_unicode_and_rejects_files() {
        let root = std::env::temp_dir().join(format!("orx-browse-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("existing 中文 folder")).unwrap();
        std::fs::write(root.join("file.txt"), "test").unwrap();
        let result = super::browse(root.to_str().unwrap()).unwrap();
        assert_eq!(result["folders"].as_array().unwrap().len(), 1);
        assert_eq!(result["folders"][0]["name"], "existing 中文 folder");
        let child = super::browse(result["folders"][0]["path"].as_str().unwrap()).unwrap();
        assert_eq!(child["parent"], result["path"]);
        let back = super::browse(
            root.join("existing 中文 folder")
                .join("..")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(back["path"], result["path"]);
        // The root's parent must return the virtual locations list, not itself.
        let root_path = root.ancestors().last().unwrap();
        assert!(super::browse(root_path.to_str().unwrap()).unwrap()["parent"].is_null());
        assert!(super::browse(root.join("file.txt").to_str().unwrap()).is_err());
        assert!(super::browse(root.join("missing").to_str().unwrap()).is_err());
        // Relative paths must not resolve against the server's working directory.
        assert!(super::browse("orx-browse-relative").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
