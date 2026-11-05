use super::prepared::{probe_args, Publication};
use super::ROUTE_FIELDS;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::Stdio;

use super::{control_dir, control_dir_for, resolved_control_path, sh_quote, SshTarget};
use crate::error::{anyhow, Result};

const INCLUDE: &str = "Include openresearch_config\n";
const FOOTER: &str = "\nHost *\nInclude openresearch_config\n";

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

pub(super) fn config_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn entry_id(entry: &Publication) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(entry).expect("SSH publication contains only strings"))
    )
}

fn fragment(default_dir: &Path, entries: &BTreeMap<String, Publication>) -> String {
    let mut contents = "# Managed by OpenResearch. Reuse existing connections only.\n".to_owned();
    for entry in entries.values() {
        let id = entry_id(entry);
        let scoped = format!("${{ORX_SSH_CONTROL_DIR}}/{}", entry.socket_name);
        let ordinary = default_dir.join(&entry.socket_name);
        for (mode, path) in [
            ("scoped", scoped),
            ("default", ordinary.to_string_lossy().into_owned()),
        ] {
            contents.push_str(&format!(
                "Match final originalhost {} exec \"/bin/sh ~/.ssh/openresearch_match {id} {mode} %C\"\n  ControlPath {}\n",
                config_quote(&entry.host), config_quote(&path)
            ));
        }
    }
    contents.push_str("Host *\n");
    contents
}

fn predicate(default_dir: &Path, entries: &BTreeMap<String, Publication>) -> String {
    let mut contents = format!(
        "test \"${{ORX_SSH_PROBE:-}}\" != 1 || exit 1\ncase \"$2\" in\n  scoped) test -n \"${{ORX_SSH_CONTROL_DIR:-}}\" || exit 1; directory=$ORX_SSH_CONTROL_DIR ;;\n  default) test -z \"${{ORX_SSH_CONTROL_DIR:-}}\" || exit 1; directory={} ;;\n  *) exit 1 ;;\nesac\ncase \"$1\" in\n",
        sh_quote(&default_dir.to_string_lossy())
    );
    for entry in entries.values() {
        contents.push_str(&format!(
            "  {})\n    test \"$3\" = {} || exit 1\n    test -S \"$directory/{}\" || exit 1\n",
            entry_id(entry),
            sh_quote(&entry.native_id),
            if entry.socket_name == "%C" {
                "$3"
            } else {
                &entry.socket_name
            }
        ));
        for query in &entry.queries {
            let args = probe_args()
                .into_iter()
                .chain(query.args.iter().cloned())
                .map(|arg| sh_quote(&arg))
                .collect::<Vec<_>>()
                .join(" ");
            contents.push_str(&format!("    actual=$(ORX_SSH_PROBE=1 ssh {args} 2>/dev/null) || exit 1\n    actual=$(printf '%s\\n' \"$actual\" | /usr/bin/awk '/^({ROUTE_FIELDS}) /')\n    test \"$actual\" = {} || exit 1\n", sh_quote(&query.output)));
        }
        contents.push_str("    ;;\n");
    }
    contents.push_str("  *) exit 1 ;;\nesac\n");
    contents
}

fn update(
    ssh_dir: &Path,
    default_dir: &Path,
    include: bool,
    change: impl FnOnce(&mut BTreeMap<String, Publication>) -> Result<()>,
) -> Result<()> {
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
    let started = std::time::Instant::now();
    let _guard = loop {
        match lock.try_write() {
            Ok(guard) => break guard,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && started.elapsed() < std::time::Duration::from_secs(5) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => {
                return Err(anyhow!(
                    "Could not acquire the SSH configuration lock: {error}"
                ))
            }
        }
    };
    let user_config = ssh_dir.join("config");
    let managed_config = ssh_dir.join("openresearch_config");
    let managed_predicate = ssh_dir.join("openresearch_match");
    let registry = ssh_dir.join("openresearch_connections.json");
    for path in [&managed_config, &managed_predicate, &registry] {
        if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
            return Err(anyhow!(
                "{} is a symlink; automatic setup will not replace it",
                path.display()
            ));
        }
    }
    let mut entries: BTreeMap<String, Publication> = match std::fs::read(&registry) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error.into()),
    };
    change(&mut entries)?;
    let contents = fragment(default_dir, &entries);
    let predicate = predicate(default_dir, &entries);
    let registry_contents = serde_json::to_vec(&entries)?;
    for (path, contents) in [
        (&managed_predicate, predicate.as_bytes()),
        (&managed_config, contents.as_bytes()),
        (&registry, registry_contents.as_slice()),
    ] {
        if !std::fs::read(path).is_ok_and(|bytes| bytes == contents) {
            crate::local::git::atomic_write_with_mode(path, contents, Some(0o600))?;
        }
    }
    if !include {
        return Ok(());
    }
    let current = match std::fs::read(&user_config) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    if current.split(|&byte| byte == b'\n').any(|line| {
        let mut fields = line
            .split(|byte| byte.is_ascii_whitespace() || *byte == b'=')
            .filter(|field| !field.is_empty());
        fields
            .next()
            .is_some_and(|key| key.eq_ignore_ascii_case(b"include"))
            && fields
                .take_while(|field| !field.starts_with(b"#"))
                .any(|path| {
                    matches!(
                        path,
                        b"openresearch_config"
                            | b"\"openresearch_config\""
                            | b"'openresearch_config'"
                    )
                })
    }) {
        return Ok(());
    }
    if std::fs::symlink_metadata(&user_config).is_ok_and(|metadata| metadata.is_symlink()) {
        return Err(anyhow!(
            "{} is a symlink; leave it intact and append `Host *` followed by `{}` to your SSH config manually",
            user_config.display(),
            INCLUDE.trim()
        ));
    }
    let mut updated = current;
    updated.extend_from_slice(FOOTER.as_bytes());
    crate::local::git::atomic_write_with_mode(&user_config, &updated, Some(0o600))?;
    Ok(())
}

async fn require_sharing() -> Result<()> {
    let version = crate::process::tokio_command("ssh")
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
    Ok(())
}

pub(crate) fn retire_legacy(home: &Path) -> Result<()> {
    let ssh_dir = home.join(".ssh");
    if !ssh_dir.join("openresearch_connections.json").exists() {
        return Ok(());
    }
    let default_dir = control_dir_for(&home.join(".config/openresearch"));
    update(&ssh_dir, &default_dir, false, |entries| {
        entries.retain(|key, _| {
            key.strip_prefix("engine:").is_none_or(|path| {
                let path = Path::new(path);
                !path.is_absolute() || path.exists()
            })
        });
        Ok(())
    })
}

pub(crate) async fn publish_engine(
    home: &Path,
    alias: &str,
    config: Option<&Path>,
    path: &Path,
    route: &str,
) -> Result<()> {
    require_sharing().await?;
    let connection = super::prepared::resolve(alias, config).await?;
    let actual = connection.publication.queries[0]
        .output
        .lines()
        .filter(|line| !line.starts_with("controlpath "))
        .collect::<Vec<_>>();
    let pinned = route
        .lines()
        .filter(|line| !line.starts_with("canonicalizehostname "))
        .collect::<Vec<_>>();
    if actual != pinned {
        return Err(anyhow!(
            "SSH route changed while publishing connection sharing"
        ));
    }
    let _master_lock = super::lock_master(&connection.path).await?;
    let publication = connection.publication;
    let path = path.to_path_buf();
    super::prepare_control_dir()?;
    let home = home.to_path_buf();
    let default_dir = control_dir_for(&home.join(".config/openresearch"));
    tokio::task::spawn_blocking(move || {
        update(&home.join(".ssh"), &default_dir, true, |entries| {
            let link = control_dir().join(&publication.socket_name);
            if link
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_symlink())
                && !link.exists()
            {
                std::fs::remove_file(&link)?;
            }
            match std::os::unix::fs::symlink(&path, &link) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !std::fs::metadata(&link)?.file_type().is_socket() {
                        return Err(anyhow!(
                            "SSH sharing path {} is not a socket",
                            link.display()
                        ));
                    }
                }
                Err(error) => return Err(error.into()),
            }
            entries.insert(format!("engine:{}", path.display()), publication);
            Ok(())
        })
    })
    .await??;
    Ok(())
}

pub(crate) async fn withdraw_engine(home: &Path, path: &Path) -> Result<()> {
    let key = format!("engine:{}", path.display());
    let path = path.to_path_buf();
    let home = home.to_path_buf();
    let default_dir = control_dir_for(&home.join(".config/openresearch"));
    tokio::task::spawn_blocking(move || {
        let ssh_dir = home.join(".ssh");
        if !ssh_dir.join("openresearch_connections.json").exists() {
            return Ok(());
        }
        update(&ssh_dir, &default_dir, false, |entries| {
            let Some(publication) = entries.remove(&key) else {
                return Ok(());
            };
            let link = control_dir().join(&publication.socket_name);
            if std::fs::read_link(&link).ok().as_ref() == Some(&path) {
                std::fs::remove_file(&link)?;
                if let Some(other) = entries.iter().find_map(|(key, entry)| {
                    let path = Path::new(key.strip_prefix("engine:")?);
                    (entry.socket_name == publication.socket_name
                        && std::fs::metadata(path)
                            .is_ok_and(|metadata| metadata.file_type().is_socket()))
                    .then_some(path)
                }) {
                    std::os::unix::fs::symlink(other, &link)?;
                }
            }
            Ok(())
        })
    })
    .await?
}

pub(super) async fn setup(target: &SshTarget) -> Result<()> {
    require_sharing().await?;
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate your home directory"))?;
    let ssh_dir = home.join(".ssh");
    let default_dir = control_dir_for(&home.join(".config/openresearch"));
    let publication = super::prepared::prepare(target, false).await?.publication;
    tokio::task::spawn_blocking(move || {
        update(&ssh_dir, &default_dir, true, |entries| {
            entries.insert(
                format!("{}:{}", publication.host, publication.native_id),
                publication,
            );
            Ok(())
        })
    })
    .await??;

    let mut command = crate::process::tokio_command("ssh");
    command
        .args(["-G", "--"])
        .arg(&target.dest)
        .env("ORX_SSH_CONTROL_DIR", control_dir())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| anyhow!("Timed out verifying SSH connection sharing"))??;
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
    use super::super::route_configuration;
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn shared_master_withdrawal_keeps_other_stores_and_legacy_sockets() {
        use std::os::unix::net::UnixListener;
        let home = crate::local::git::TemporaryDirectory::new("orx-sharing-withdraw").unwrap();
        if std::env::var("ORX_TEST_SHARED_MASTER").as_deref() != Ok("1") {
            let output = crate::process::command(std::env::current_exe().unwrap())
                .env("ORX_TEST_SHARED_MASTER", "1")
                .env("XDG_CONFIG_HOME", home.path().join("config"))
                .args([
                    "--exact",
                    concat!(
                        module_path!(),
                        "::shared_master_withdrawal_keeps_other_stores_and_legacy_sockets"
                    ),
                    "--nocapture",
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        super::super::prepare_control_dir().unwrap();
        let sockets = Path::new("/tmp").join(format!("orx-sharing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&sockets).unwrap();
        let first = sockets.join("first");
        let second = sockets.join("second");
        let _first = UnixListener::bind(&first).unwrap();
        let _second = UnixListener::bind(&second).unwrap();
        let publication = Publication {
            host: "lab".into(),
            native_id: "a".repeat(40),
            socket_name: "same-route-and-agent".into(),
            queries: Vec::new(),
        };
        let link = control_dir().join(&publication.socket_name);
        std::os::unix::fs::symlink(&first, &link).unwrap();
        update(&home.path().join(".ssh"), &control_dir(), true, |entries| {
            entries.insert(format!("engine:{}", first.display()), publication.clone());
            entries.insert(format!("engine:{}", second.display()), publication.clone());
            Ok(())
        })
        .unwrap();
        withdraw_engine(home.path(), &first).await.unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), second);
        withdraw_engine(home.path(), &second).await.unwrap();
        assert!(link.symlink_metadata().is_err());
        let _legacy = UnixListener::bind(&link).unwrap();
        update(&home.path().join(".ssh"), &control_dir(), true, |entries| {
            entries.insert(format!("engine:{}", first.display()), publication.clone());
            Ok(())
        })
        .unwrap();
        withdraw_engine(home.path(), &first).await.unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_socket());
        std::fs::remove_file(link).unwrap();
        std::fs::remove_dir(control_dir()).unwrap();
        std::fs::remove_dir_all(sockets).unwrap();
    }

    #[test]
    fn native_config_shares_only_matching_live_sockets_and_preserves_user_settings() {
        use std::os::unix::net::UnixListener;
        let version = crate::process::command("ssh").arg("-V").output().unwrap();
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
            predicate(&prod, &BTreeMap::new()),
        )
        .unwrap();
        std::fs::write(&managed, fragment(&prod, &BTreeMap::new())).unwrap();
        let user = "ServerAliveInterval 7\nHost lab alias\n  HostName example.invalid\n  User alice\n  Port 2222\n";
        let include = format!(
            "Host *\nInclude {}\n",
            config_quote(&managed.to_string_lossy())
        );
        let original = format!("{user}{include}");
        std::fs::write(&config, &original).unwrap();
        let mut entries = BTreeMap::new();
        for host in ["lab", "alias"] {
            let args = vec![
                "-F".into(),
                config.to_string_lossy().into_owned(),
                "--".into(),
                host.into(),
            ];
            let output = crate::process::command("ssh")
                .args(probe_args())
                .args(&args)
                .env("HOME", temp.path())
                .env("ORX_SSH_PROBE", "1")
                .output()
                .unwrap();
            assert!(output.status.success());
            let output = route_configuration(&String::from_utf8(output.stdout).unwrap());
            let native_id = output
                .lines()
                .find_map(|line| line.strip_prefix("controlpath /tmp/orx-ssh-probe-"))
                .unwrap()
                .to_owned();
            entries.insert(
                host.to_owned(),
                Publication {
                    host: host.into(),
                    native_id,
                    socket_name: "%C".into(),
                    queries: vec![super::super::prepared::Query { args, output }],
                },
            );
        }
        std::fs::write(&managed, fragment(&prod, &entries)).unwrap();
        std::fs::write(
            temp.path().join(".ssh/openresearch_match"),
            predicate(&prod, &entries),
        )
        .unwrap();
        let query = |args: &[&str], marker: Option<&Path>| {
            let mut cmd = crate::process::command("ssh");
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
        for section in ["Host *", "Match final"] {
            for user_path in ["none", "/tmp/user-chosen"] {
                std::fs::write(
                    &config,
                    format!("{user}{section}\n  ControlPath {user_path}\n{include}"),
                )
                .unwrap();
                let expected = (user_path != "none").then(|| PathBuf::from(user_path));
                assert_eq!(selected(&query(&["lab"], Some(&dev))), expected);
            }
        }
        std::fs::write(
            &config,
            format!("{user}Host lab\n  ProxyCommand printf changed\n{include}"),
        )
        .unwrap();
        assert_eq!(selected(&query(&["lab"], None)), None);
        std::fs::write(&config, &original).unwrap();
        drop(prod_socket);
        std::fs::remove_file(&expected).unwrap();
        assert_eq!(selected(&query(&["lab"], None)), None);
        drop(dev_socket);
        std::fs::remove_dir_all(&prod).unwrap();
        std::fs::remove_dir_all(&dev).unwrap();
    }

    #[test]
    fn engine_and_legacy_publications_share_the_lock_and_registry() {
        use std::os::unix::net::UnixListener;
        let temp = crate::local::git::TemporaryDirectory::new("orx-sharing-coexist").unwrap();
        let ssh_dir = temp.path().join(".ssh");
        let default_dir = temp.path().join("sockets");
        let socket = Path::new("/tmp").join(format!("orx-sharing-{}", uuid::Uuid::new_v4()));
        let listener = UnixListener::bind(&socket).unwrap();
        let publication = Publication {
            host: "lab".into(),
            native_id: "a".repeat(40),
            socket_name: "0123456789abcdef".into(),
            queries: Vec::new(),
        };
        let key = format!("engine:{}", socket.display());
        update(&ssh_dir, &default_dir, true, |entries| {
            entries.insert(key.clone(), publication.clone());
            Ok(())
        })
        .unwrap();
        let user = std::fs::read(ssh_dir.join("config")).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(ssh_dir.join(".orx-config.lock"))
            .unwrap();
        let mut lock = fd_lock::RwLock::new(file);
        let guard = lock.try_write().unwrap();
        let (attempting, attempted) = std::sync::mpsc::channel();
        let writer = std::thread::spawn({
            let ssh_dir = ssh_dir.clone();
            let default_dir = default_dir.clone();
            move || {
                attempting.send(()).unwrap();
                update(&ssh_dir, &default_dir, true, |entries| {
                    entries.insert(
                        format!("{}:{}", publication.host, publication.native_id),
                        publication,
                    );
                    Ok(())
                })
                .unwrap();
            }
        });
        attempted.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!writer.is_finished());
        drop(guard);
        writer.join().unwrap();
        let read = || {
            serde_json::from_slice::<BTreeMap<String, Publication>>(
                &std::fs::read(ssh_dir.join("openresearch_connections.json")).unwrap(),
            )
            .unwrap()
        };
        assert_eq!(read().len(), 2);
        retire_legacy(temp.path()).unwrap();
        assert_eq!(read().len(), 2);
        assert_eq!(std::fs::read(ssh_dir.join("config")).unwrap(), user);
        drop(listener);
        std::fs::remove_file(socket).unwrap();
        retire_legacy(temp.path()).unwrap();
        assert_eq!(read().len(), 1);
        assert!(!read().contains_key(&key));
        assert_eq!(std::fs::read(ssh_dir.join("config")).unwrap(), user);
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
        update(&ssh_dir, Path::new("/tmp/default"), true, |_| Ok(())).unwrap();
        let once = std::fs::read(&user_config).unwrap();
        assert_eq!(&once[..original.len()], original);
        update(&ssh_dir, Path::new("/tmp/default"), true, |_| Ok(())).unwrap();
        assert_eq!(std::fs::read(&user_config).unwrap(), once);
        let later = b"Match final\n  ControlPath none\n";
        for contents in [
            [once.as_slice(), later].concat(),
            [b"# legacy\n", INCLUDE.as_bytes(), once.as_slice()].concat(),
            b"Host *\r\n  include openresearch_config\r\nHost office\r\n  Port 2222\r\n".to_vec(),
            b"Host *\nInclude\t  \"openresearch_config\" # sharing\nHost office\n".to_vec(),
            b"Host *\nInclude=openresearch_config".to_vec(),
        ] {
            use std::os::unix::fs::MetadataExt;
            std::fs::write(&user_config, &contents).unwrap();
            let before = std::fs::metadata(&user_config).unwrap();
            update(&ssh_dir, Path::new("/tmp/default"), true, |_| Ok(())).unwrap();
            let after = std::fs::metadata(&user_config).unwrap();
            assert_eq!(std::fs::read(&user_config).unwrap(), contents);
            assert_eq!(after.modified().unwrap(), before.modified().unwrap());
            assert_eq!((after.ino(), after.mode()), (before.ino(), before.mode()));
        }
        std::fs::write(
            &user_config,
            [b"Host office", FOOTER.as_bytes(), b"  Port 2222\n"].concat(),
        )
        .unwrap();
        update(&ssh_dir, Path::new("/tmp/default"), true, |_| Ok(())).unwrap();
        let resolved = crate::process::command("ssh")
            .args(["-G", "-F"])
            .arg(&user_config)
            .arg("another-host")
            .output()
            .unwrap();
        assert!(resolved.status.success());
        assert!(String::from_utf8_lossy(&resolved.stdout).contains("port 2222\n"));
        std::fs::remove_file(&user_config).unwrap();
        let real = temp.path().join("real");
        std::fs::write(&real, original).unwrap();
        symlink(&real, &user_config).unwrap();
        assert!(update(&ssh_dir, Path::new("/tmp/default"), true, |_| Ok(())).is_err());
        assert_eq!(std::fs::read(&real).unwrap(), original);
        assert_eq!(
            std::fs::read(ssh_dir.join("openresearch_config")).unwrap(),
            fragment(Path::new("/tmp/default"), &BTreeMap::new()).as_bytes()
        );
    }
    #[test]
    fn another_stores_live_legacy_publication_is_preserved() {
        use crate::engine::clock::TestClock;
        use crate::engine::Engine;
        use crate::store::{Store, StoredRun};
        use std::fs::{self, OpenOptions};
        use std::os::unix::net::UnixListener;
        use std::sync::Arc;
        let home = crate::local::git::TemporaryDirectory::new("orx-sharing-stores").unwrap();
        let ssh_dir = home.path().join(".ssh");
        fs::create_dir(&ssh_dir).unwrap();
        fs::write(ssh_dir.join("config"), FOOTER).unwrap();
        let engine = Engine::new(
            Store::open_at(home.path().join("data")).unwrap(),
            Arc::new(TestClock::new()),
        );
        let other = Store::open_at(home.path().join("other-data")).unwrap();
        let run = StoredRun {
            id: "other-legacy".into(),
            experiment_id: "exp".into(),
            project_id: "proj".into(),
            status: "running".into(),
            backend_json: r#"{"kind":"ssh_job","jobId":"/other/run"}"#.into(),
            command: "train".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        };
        other.upsert_run(&run).unwrap();
        let [path, _] = other.supervisor_lock_paths(&run.id).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap();
        let mut lock = fd_lock::RwLock::new(file);
        let _guard = lock.try_write().unwrap();
        let sockets = control_dir_for(&home.path().join(".config/openresearch"));
        fs::create_dir(&sockets).unwrap();
        let socket = sockets.join("other-route");
        let _listener = UnixListener::bind(&socket).unwrap();
        let registry = serde_json::json!({
            "engine:other-route": {
                "host": "engine",
                "native_id": "other-route",
                "socket_name": "%C",
                "queries": []
            }
        });
        let path = ssh_dir.join("openresearch_connections.json");
        fs::write(&path, serde_json::to_vec(&registry).unwrap()).unwrap();
        assert!(engine.retire_ssh_config(home.path()).unwrap());
        assert!(socket.exists());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap(),
            registry
        );
        assert_eq!(fs::read(ssh_dir.join("config")).unwrap(), FOOTER.as_bytes());
        assert!(engine.retire_ssh_config(home.path()).unwrap());
        drop(other);
        fs::remove_file(socket).unwrap();
        fs::remove_dir(sockets).unwrap();
    }
}
