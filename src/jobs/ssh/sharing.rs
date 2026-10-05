use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use super::{control_dir, control_dir_for, resolved_control_path, sh_quote, SshTarget};
use crate::error::{anyhow, Result};

const INCLUDE: &str = "Include openresearch_config\n";

fn supports_sharing(version: &str) -> bool {
    let Some(version) = version.trim().strip_prefix("OpenSSH_") else {
        return false;
    };
    let Some((major, rest)) = version.split_once('.') else {
        return false;
    };
    let minor: String = rest.chars().take_while(char::is_ascii_digit).collect();
    match (major.parse::<u32>(), minor.parse::<u32>()) {
        (Ok(major), Ok(minor)) => (major, minor) >= (8, 4),
        _ => false,
    }
}

fn config_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn fragment(default_dir: &Path) -> String {
    let default_path = default_dir.join("%C");
    let default_path = default_path.to_string_lossy();
    format!(
        "# Managed by OpenResearch. Reuse existing connections only.\n\
         Match final exec \"/bin/sh ~/.ssh/openresearch_match scoped %C\"\n  ControlPath \"${{ORX_SSH_CONTROL_DIR}}/%C\"\n\
         Match final exec \"/bin/sh ~/.ssh/openresearch_match default %C\"\n  ControlPath {}\n\
         Host *\n",
        config_quote(&default_path)
    )
}

fn predicate(default_dir: &Path) -> String {
    format!(
        "case \"$1\" in\n  scoped) test -n \"${{ORX_SSH_CONTROL_DIR:-}}\" && test -S \"$ORX_SSH_CONTROL_DIR/$2\" ;;\n  default) test -z \"${{ORX_SSH_CONTROL_DIR:-}}\" && test -S {}/\"$2\" ;;\n  *) exit 1 ;;\nesac\n",
        sh_quote(&default_dir.to_string_lossy())
    )
}

fn install(ssh_dir: &Path, contents: &str, predicate: &str) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(ssh_dir)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(ssh_dir.join(".orx-config.lock"))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write()?;
    let user_config = ssh_dir.join("config");
    let managed_config = ssh_dir.join("openresearch_config");
    let managed_predicate = ssh_dir.join("openresearch_match");
    for path in [&managed_config, &managed_predicate] {
        if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
            return Err(anyhow!(
                "{} is a symlink; automatic setup will not replace it",
                path.display()
            ));
        }
    }
    let current = match std::fs::read(&user_config) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    for (path, contents) in [(&managed_predicate, predicate), (&managed_config, contents)] {
        if !std::fs::read(path).is_ok_and(|bytes| bytes == contents.as_bytes()) {
            crate::local::git::atomic_write_with_mode(path, contents.as_bytes(), Some(0o600))?;
        }
    }
    if std::fs::symlink_metadata(&user_config).is_ok_and(|metadata| metadata.is_symlink()) {
        return Err(anyhow!(
            "{} is a symlink; leave it intact and add `{}` at the top of your SSH config manually",
            user_config.display(),
            INCLUDE.trim()
        ));
    }
    if !current.starts_with(INCLUDE.as_bytes()) {
        let mut updated = INCLUDE.as_bytes().to_vec();
        updated.extend_from_slice(&current);
        crate::local::git::atomic_write_with_mode(&user_config, &updated, Some(0o600))?;
    }
    Ok(())
}

pub(super) async fn setup(target: &SshTarget) -> Result<()> {
    let version = Command::new("ssh")
        .arg("-V")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !version.status.success() || !supports_sharing(&String::from_utf8_lossy(&version.stderr)) {
        return Err(anyhow!(
            "automatic sharing requires OpenSSH 8.4 or newer; your SSH config was not changed"
        ));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate your home directory"))?;
    let ssh_dir = home.join(".ssh");
    let default_dir = control_dir_for(&home.join(".config/openresearch"));
    let contents = fragment(&default_dir);
    let predicate = predicate(&default_dir);
    tokio::task::spawn_blocking(move || install(&ssh_dir, &contents, &predicate)).await??;

    let output = Command::new("ssh")
        .args(["-G", "--"])
        .arg(&target.dest)
        .env("ORX_SSH_CONTROL_DIR", control_dir())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!(
            "could not verify connection sharing: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let expected = resolved_control_path(target).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let selected = stdout
        .lines()
        .find_map(|line| line.strip_prefix("controlpath "));
    if selected != expected.to_str() {
        return Err(anyhow!("your existing SSH settings do not select OpenResearch's connection for {}; reconnect if the master expired, or adjust your ControlPath manually", target.dest));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn native_config_shares_only_matching_live_sockets_and_preserves_user_settings() {
        use std::os::unix::net::UnixListener;
        let version = std::process::Command::new("ssh")
            .arg("-V")
            .output()
            .unwrap();
        if !supports_sharing(&String::from_utf8_lossy(&version.stderr)) {
            return;
        }
        let temp = crate::local::git::TemporaryDirectory::new("orx-sharing-native").unwrap();
        let prod = control_dir_for(&temp.path().join("prod"));
        let dev = control_dir_for(&temp.path().join("dev"));
        std::fs::create_dir(&prod).unwrap();
        std::fs::create_dir(&dev).unwrap();
        let managed = temp.path().join("fragment with spaces");
        let config = temp.path().join("config");
        std::fs::create_dir(temp.path().join(".ssh")).unwrap();
        std::fs::write(
            temp.path().join(".ssh/openresearch_match"),
            predicate(&prod),
        )
        .unwrap();
        std::fs::write(&managed, fragment(&prod)).unwrap();
        let original = format!("Include {}\nServerAliveInterval 7\nHost lab alias\n  HostName example.invalid\n  User alice\n  Port 2222\n", config_quote(&managed.to_string_lossy()));
        std::fs::write(&config, &original).unwrap();
        let query = |args: &[&str], marker: Option<&Path>| {
            let mut cmd = std::process::Command::new("ssh");
            cmd.arg("-F")
                .arg(&config)
                .arg("-G")
                .args(args)
                .env("HOME", temp.path())
                .env_remove("ORX_SSH_CONTROL_DIR");
            if let Some(marker) = marker {
                cmd.env("ORX_SSH_CONTROL_DIR", marker);
            }
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!String::from_utf8_lossy(&output.stderr).contains("expand"));
            String::from_utf8(output.stdout).unwrap()
        };
        let selected = |output: &str| {
            output
                .lines()
                .find_map(|line| line.strip_prefix("controlpath ").map(PathBuf::from))
        };
        assert_eq!(selected(&query(&["lab"], None)), None);
        let expected = selected(&query(
            &["-o", &format!("ControlPath={}/%C", prod.display()), "lab"],
            None,
        ))
        .unwrap();
        let dev_expected = dev.join(expected.file_name().unwrap());
        let prod_socket = UnixListener::bind(&expected).unwrap();
        let dev_socket = UnixListener::bind(&dev_expected).unwrap();
        assert_eq!(selected(&query(&["lab"], None)), Some(expected.clone()));
        assert_eq!(selected(&query(&["alias"], None)), Some(expected.clone()));
        assert_eq!(selected(&query(&["lab"], Some(&dev))), Some(dev_expected));
        for args in [
            &["-p", "2223", "lab"][..],
            &["-l", "bob", "lab"],
            &["unrelated"],
        ] {
            assert_eq!(selected(&query(args, None)), None);
        }
        assert!(query(&["lab"], None).contains("serveraliveinterval 7\n"));
        for user_path in ["none", "/tmp/user-chosen"] {
            std::fs::write(
                &config,
                format!("{original}Host *\n  ControlPath {user_path}\n"),
            )
            .unwrap();
            let expected = (user_path != "none").then(|| PathBuf::from(user_path));
            assert_eq!(selected(&query(&["lab"], Some(&dev))), expected);
        }
        std::fs::write(&config, &original).unwrap();
        drop(prod_socket);
        std::fs::remove_file(&expected).unwrap();
        assert_eq!(selected(&query(&["lab"], None)), None);
        drop(dev_socket);
        std::fs::remove_dir_all(&prod).unwrap();
        std::fs::remove_dir_all(&dev).unwrap();
    }

    #[test]
    fn version_requires_supported_openssh() {
        for version in ["OpenSSH_8.4p1, LibreSSL", "OpenSSH_9.9p2", "OpenSSH_10.3p1"] {
            assert!(supports_sharing(version));
        }
        for version in [
            "OpenSSH_8.3p1",
            "OpenSSH_7.9",
            "Dropbear_2024",
            "OpenSSH_unknown",
        ] {
            assert!(!supports_sharing(version));
        }
    }

    #[test]
    fn installation_preserves_bytes_is_idempotent_and_refuses_symlinks() {
        use std::os::unix::fs::symlink;
        let temp = crate::local::git::TemporaryDirectory::new("orx-sharing-install").unwrap();
        let ssh_dir = temp.path().join("ssh");
        std::fs::create_dir(&ssh_dir).unwrap();
        let user_config = ssh_dir.join("config");
        let original = b"# user's config\r\nHost lab\r\n  User me";
        std::fs::write(&user_config, original).unwrap();
        install(&ssh_dir, "managed\n", "predicate\n").unwrap();
        let once = std::fs::read(&user_config).unwrap();
        assert_eq!(&once[INCLUDE.len()..], original);
        install(&ssh_dir, "managed\n", "predicate\n").unwrap();
        assert_eq!(std::fs::read(&user_config).unwrap(), once);
        std::fs::remove_file(&user_config).unwrap();
        let real = temp.path().join("real");
        std::fs::write(&real, original).unwrap();
        symlink(&real, &user_config).unwrap();
        assert!(install(&ssh_dir, "changed\n", "predicate\n").is_err());
        assert_eq!(std::fs::read(&real).unwrap(), original);
        assert_eq!(
            std::fs::read(ssh_dir.join("openresearch_config")).unwrap(),
            b"changed\n"
        );
    }
}
