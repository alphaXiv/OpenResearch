//! `orx ssh-key` — register the public key this computer authenticates with.
//!
//! Boxes trust the keys registered on the account, so a machine whose key isn't
//! registered can't reach any box it provisions. A key is only uploaded when the
//! user asks: this command, `orx compute connect openresearch`, or the dashboard's
//! OpenResearch compute settings, never at sign-in.

use crate::client::{create_ssh_key, list_ssh_keys};
use crate::config::Credentials;
use crate::error::{anyhow, Result};
use crate::local::ssh_identity::{self, fingerprint, key_comment};
use crate::output::print_table;

/// Where `add` looks when no path is given: the key `ssh-keygen -t ed25519`
/// writes, which ssh offers by default.
pub(crate) const DEFAULT_KEY: &str = "~/.ssh/id_ed25519.pub";

/// The shared http client has no timeout, and the settings page waits on this.
const REGISTER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Registers [`DEFAULT_KEY`], creating the key pair if missing. `false` when the
/// account already had it.
pub(crate) async fn ensure_default_registered(creds: &Credentials) -> Result<bool> {
    tokio::time::timeout(REGISTER_TIMEOUT, async {
        let ssh_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Couldn't locate your home directory."))?
            .join(".ssh");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&ssh_dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(ssh_dir.join(".orx-key-setup.lock"))?;
        let mut lock = fd_lock::RwLock::new(file);
        let _guard = lock
            .try_write()
            .map_err(|_| anyhow!("SSH key setup is already running. Try again shortly."))?;
        let existing = list_ssh_keys(creds).await?;
        let line = ensure_default_public_key(&ssh_dir).await?;
        let fp =
            fingerprint(&line).ok_or_else(|| anyhow!("{DEFAULT_KEY} isn't an SSH public key."))?;
        if existing
            .ssh_keys
            .iter()
            .any(|key| fingerprint(&key.public_key).as_deref() == Some(fp.as_str()))
        {
            return Ok(false);
        }
        create_ssh_key(creds, &device_name(&line), &line).await?;
        Ok(true)
    })
    .await
    .map_err(|_| anyhow!("SSH key setup timed out. Try again."))?
}

/// The `SHA256:` fingerprint of [`DEFAULT_KEY`], or `None` while that file is
/// missing or unreadable.
pub(crate) async fn default_key_fingerprint() -> Option<String> {
    let body = tokio::fs::read_to_string(expand(DEFAULT_KEY)).await.ok()?;
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(ssh_identity::sha256_fingerprint)
}

async fn ensure_default_public_key(ssh_dir: &std::path::Path) -> Result<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let private = ssh_dir.join("id_ed25519");
    let public = ssh_dir.join("id_ed25519.pub");
    if !public.try_exists()? {
        let mut command = crate::process::tokio_command("ssh-keygen");
        if private.try_exists()? {
            println!("Restoring the public-key file from the existing private key\u{2026}");
            command.args(["-y", "-P", "", "-f"]).arg(&private);
        } else {
            println!("Creating ~/.ssh/id_ed25519 and its public-key file\u{2026}");
            command
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&private);
        }
        let output = command
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await?;
        if !output.status.success() {
            return Err(anyhow!("Couldn't prepare ~/.ssh/id_ed25519.pub. Existing keys were preserved. If the private key is passphrase-protected, restore its public-key file manually."));
        }
        if !public.try_exists()? {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&public)
                .await?;
            file.write_all(&output.stdout).await?;
            file.flush().await?;
        }
    }
    if !private.is_file() {
        return Err(anyhow!("~/.ssh/id_ed25519.pub exists, but its private key is missing. Restore ~/.ssh/id_ed25519; no keys were overwritten."));
    }
    let body = tokio::fs::read_to_string(&public).await?;
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.starts_with("-----BEGIN") || fingerprint(line).is_none() {
        return Err(anyhow!(
            "~/.ssh/id_ed25519.pub is not an SSH public key. Nothing was uploaded."
        ));
    }
    Ok(line.to_string())
}

fn expand(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| path.into()),
        None => path.into(),
    }
}

/// Name the key after its comment (`alice@laptop`), which is how the user
/// recognizes the device in the dashboard; fall back to the hostname.
fn device_name(line: &str) -> String {
    key_comment(line)
        .or_else(|| {
            crate::process::command("hostname")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "this computer".to_string())
}

pub async fn add(path: Option<String>) -> Result<()> {
    let creds = crate::error::require_credentials().await;
    match path {
        Some(path) => add_with_creds(&creds, path).await,
        None => {
            if ensure_default_registered(&creds).await? {
                println!(
                    "\u{2713} Registered {DEFAULT_KEY}. Your private key stays on this computer."
                );
            } else {
                println!("\u{2713} {DEFAULT_KEY} is already registered.");
            }
            Ok(())
        }
    }
}

async fn add_with_creds(creds: &Credentials, path: String) -> Result<()> {
    let resolved = expand(&path);

    let shown = ssh_identity::tilde(&resolved);
    let body = tokio::fs::read_to_string(&resolved).await.map_err(|_| {
        anyhow!(
            "Couldn't read {shown}. Pass the path to your public key, or create one:\n  \
             ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519\n  orx ssh-key add {DEFAULT_KEY}"
        )
    })?;
    // Only the first key: a file holding several would otherwise be POSTed as
    // one blob and land in authorized_keys malformed.
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.is_empty() {
        return Err(anyhow!("{shown} is empty."));
    }
    // A private key starts with a PEM header and would be rejected server-side
    // anyway — catch it here so the user isn't told "invalid format" about a
    // file they were right to treat as sensitive.
    if line.starts_with("-----BEGIN") {
        // Don't guess the sibling path — for `key.pem` a `.pub` suffix names a
        // file that doesn't exist, and the resulting not-found error tells the
        // user to ssh-keygen over a key they already have.
        return Err(anyhow!(
            "{shown} is a private key — don't share it.\nRegister the matching public \
             key instead (usually the same name with .pub)."
        ));
    }

    // The api has no uniqueness constraint, so without a check every repeat run
    // adds a row and the same key lands in authorized_keys N times.
    let fp = fingerprint(line).ok_or_else(|| anyhow!("{shown} isn't an SSH public key."))?;
    if let Ok(existing) = list_ssh_keys(creds).await {
        if let Some(dup) = existing
            .ssh_keys
            .iter()
            .find(|k| fingerprint(&k.public_key).as_deref() == Some(fp.as_str()))
        {
            println!(
                "This key is already registered as \u{201c}{}\u{201d}.",
                dup.name
            );
            return Ok(());
        }
    }

    let name = device_name(line);
    // A sandbox's own token is refused here by design (it would grant every box
    // in the org), so name that case rather than leaving the generic
    // "token is invalid or revoked".
    // Two causes, both 401: a server older than this command (cookie-only until
    // the api ships), or a box's own token, which is refused by design.
    let key = create_ssh_key(creds, &name, line).await.map_err(|err| {
        if err.is::<crate::client::Unauthorized>() {
            anyhow!(
                "The server wouldn't register a key with this token.\n  \
                 • It may predate `orx ssh-key add` — add the key to your OpenResearch account instead.\n  \
                 • Or you're on a box: a box's token can't add keys, so run this\n    \
                 on your own computer."
            )
        } else {
            err
        }
    })?;
    println!(
        "\u{2713} Registered SSH key \u{201c}{}\u{201d}.",
        key.ssh_key.name
    );
    println!("  Boxes in your orgs now accept this computer, including any already running.");
    Ok(())
}

pub async fn list() -> Result<()> {
    let creds = crate::error::require_credentials().await;
    let keys = list_ssh_keys(&creds).await?.ssh_keys;
    if keys.is_empty() {
        println!("No SSH keys registered. Add this computer's key:");
        println!("  orx ssh-key add {DEFAULT_KEY}");
        return Ok(());
    }
    let local = ssh_identity::local_keys().await;
    let rows: Vec<Vec<String>> = keys
        .iter()
        .map(|key| {
            let here = ssh_identity::is_local(&local, &key.public_key);
            vec![
                key.name.clone(),
                if here {
                    "this computer"
                } else {
                    "another computer"
                }
                .to_string(),
            ]
        })
        .collect();
    print_table(&["NAME", "DEVICE"], &rows);
    Ok(())
}

#[cfg(test)]
mod setup_tests {
    use super::*;
    use crate::local::ssh_identity::KeyStatus;
    use axum::{routing::get, Json, Router};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn default_key_is_created_and_registered_once() {
        // `~/.ssh` and the agent are per-process, so the body runs in a child
        // with an empty home and no agent.
        if std::env::var("ORX_SSH_KEY_TEST_CHILD").is_err() {
            let home = std::env::temp_dir().join(format!("orx-ssh-key-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&home).unwrap();
            let status = crate::process::tokio_command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "commands::ssh_key::setup_tests::default_key_is_created_and_registered_once",
                    "--nocapture",
                ])
                .env("ORX_SSH_KEY_TEST_CHILD", "1")
                .env("HOME", &home)
                .env_remove("SSH_AUTH_SOCK")
                .status()
                .await
                .unwrap();
            let _ = std::fs::remove_dir_all(home);
            assert!(status.success());
            return;
        }

        let registered = Arc::new(Mutex::new(Vec::<Value>::new()));
        let list = registered.clone();
        let add = registered.clone();
        let app = Router::new().route(
            "/ssh-keys",
            get(move || {
                let keys = list.lock().unwrap().clone();
                async move { Json(json!({ "sshKeys": keys })) }
            })
            .post(move |Json(body): Json<Value>| {
                let key =
                    json!({ "id": "key-1", "name": body["name"], "publicKey": body["publicKey"] });
                add.lock().unwrap().push(key.clone());
                async move { Json(json!({ "sshKey": key })) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let creds = Credentials {
            api_url: format!("http://{}", listener.local_addr().unwrap()),
            token: "pat".into(),
        };
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        assert_eq!(default_key_fingerprint().await, None);
        assert!(ensure_default_registered(&creds).await.unwrap());
        assert!(!ensure_default_registered(&creds).await.unwrap());

        let ssh = dirs::home_dir().unwrap().join(".ssh");
        assert!(ssh.join("id_ed25519").is_file());
        let public = std::fs::read_to_string(ssh.join("id_ed25519.pub")).unwrap();
        let keys = registered.lock().unwrap().clone();
        assert_eq!(keys.len(), 1);
        assert_eq!(
            fingerprint(keys[0]["publicKey"].as_str().unwrap()),
            fingerprint(public.trim())
        );
        assert!(matches!(
            ssh_identity::check(&creds).await,
            KeyStatus::Matched
        ));
        assert_eq!(
            default_key_fingerprint().await,
            ssh_identity::sha256_fingerprint(public.trim())
        );
    }

    #[tokio::test]
    async fn default_key_setup_preserves_existing_keys() {
        let dir = std::env::temp_dir().join(format!("orx-key-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let public = ensure_default_public_key(&dir).await.unwrap();
        let private = std::fs::read(dir.join("id_ed25519")).unwrap();
        assert!(public.starts_with("ssh-ed25519 "));
        assert_eq!(ensure_default_public_key(&dir).await.unwrap(), public);
        std::fs::remove_file(dir.join("id_ed25519.pub")).unwrap();
        let restored = ensure_default_public_key(&dir).await.unwrap();
        assert_eq!(fingerprint(&restored), fingerprint(&public));
        assert_eq!(std::fs::read(dir.join("id_ed25519")).unwrap(), private);
        std::fs::remove_file(dir.join("id_ed25519")).unwrap();
        assert!(ensure_default_public_key(&dir).await.is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("id_ed25519.pub"))
                .unwrap()
                .trim(),
            restored
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
