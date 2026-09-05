//! `orx ssh-key` — register the public key this computer authenticates with.
//!
//! Boxes trust the keys registered on the account, so a machine whose key isn't
//! registered can't reach any box it provisions. The dashboard has always
//! pointed at this command; it lives here so the fix never requires a browser.

use crate::client::{create_ssh_key, list_ssh_keys};
use crate::config::Credentials;
use crate::error::{anyhow, Result};
use crate::local::ssh_identity::{self, fingerprint, key_comment, KeyStatus};
use crate::output::print_table;

/// Where `add` looks when no path is given — the key `ssh-keygen -t ed25519`
/// writes, which is also what the dashboard's copy-paste hint reads.
const DEFAULT_KEY: &str = "~/.ssh/id_ed25519.pub";

/// The shared http client has no timeout, and `orx login` must not hang on the
/// courtesy registration after it has already printed success.
const REGISTER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

async fn ensure_default_registered(creds: &Credentials) -> Result<()> {
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
            println!("\u{2713} {DEFAULT_KEY} is already registered.");
            return Ok(());
        }
        create_ssh_key(creds, &device_name(&line), &line).await?;
        println!("\u{2713} Registered {DEFAULT_KEY}. Your private key stays on this computer.");
        Ok(())
    })
    .await
    .map_err(|_| anyhow!("SSH key setup timed out. Try again."))?
}

async fn ensure_default_public_key(ssh_dir: &std::path::Path) -> Result<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let private = ssh_dir.join("id_ed25519");
    let public = ssh_dir.join("id_ed25519.pub");
    if !public.try_exists()? {
        let mut command = tokio::process::Command::new("ssh-keygen");
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
            std::process::Command::new("hostname")
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
        None => ensure_default_registered(&creds).await,
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

    // The api has no uniqueness constraint and the login prompt offers this on
    // every login, so without a check a repeat user accumulates a row per login
    // and the same key lands in authorized_keys N times.
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
        if err.to_string().contains("Unauthorized") {
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

/// Post-login check: tell the user now, while they're at the keyboard, if the
/// boxes they provision won't accept this computer. Consent-gated and
/// best-effort, mirroring `install_skills::offer_install_after_login`.
pub async fn verify_after_login(creds: &Credentials) {
    use std::io::IsTerminal;

    // Before any network work: a piped login must not block on a hung api. All
    // three streams, since the prompt reads stdin, writes stdout, and reports
    // failures on stderr — redirecting any of them means nobody sees this.
    if !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || !std::io::stderr().is_terminal()
    {
        return;
    }

    let registered = match ssh_identity::check(creds).await {
        KeyStatus::Matched => return,
        KeyStatus::Unknown { reason } => {
            eprintln!("Couldn't check SSH keys: {reason}\nRetry with: orx ssh-key add");
            return;
        }
        KeyStatus::NoLocalMatch { registered, .. } => registered,
        KeyStatus::NoneRegistered { .. } => Vec::new(),
    };

    println!("\n{}", ssh_identity::diagnosis(&registered));

    println!("Registering this computer's public SSH key lets it connect to OpenResearch");
    println!("compute instances in your organizations, including ones already running.");
    println!("We'll use {DEFAULT_KEY}, creating a key pair if needed.");
    print!("Register this computer's public key with your OpenResearch account? [Y/n] ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
        return;
    }
    if !matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes") {
        println!("Skipped. You can set it up any time with:\n  orx ssh-key add");
        return;
    }
    if let Err(err) = ensure_default_registered(creds).await {
        eprintln!("Couldn't set up SSH access: {err}\nRetry with: orx ssh-key add");
    }
}

#[cfg(test)]
mod setup_tests {
    use super::*;

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
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
