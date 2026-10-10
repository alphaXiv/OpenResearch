use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use super::{control_dir, sh_quote, sharing::config_quote, SshTarget};
use crate::error::{anyhow, Result};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Query {
    pub args: Vec<String>,
    pub output: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Publication {
    pub host: String,
    pub native_id: String,
    pub socket_name: String,
    pub queries: Vec<Query>,
}

#[derive(Clone)]
pub(super) struct Prepared {
    pub publication: Publication,
    pub snapshot: PathBuf,
    pub path: PathBuf,
}

type Key = (PathBuf, String);
static CONNECTIONS: LazyLock<RwLock<BTreeMap<Key, Prepared>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));
static PREPARATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(super) fn cached(target: &SshTarget) -> Option<Prepared> {
    CONNECTIONS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(control_dir(), target.dest.clone()))
        .cloned()
}

fn value<'a>(output: &'a str, key: &str) -> Result<&'a str> {
    output
        .lines()
        .find_map(|line| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '))
        })
        .ok_or_else(|| anyhow!("OpenSSH did not resolve {key}"))
}

fn unresolved_message(key: &str, ssh: Option<&Path>, version: Option<&str>) -> String {
    let mut message = format!(
        "OpenSSH did not resolve {key} (ssh: {}, version: {})",
        ssh.map_or("not found on PATH".into(), |path| path
            .display()
            .to_string()),
        version.unwrap_or("unknown")
    );
    if !version.is_some_and(|version| version.starts_with("OpenSSH_")) {
        message.push_str("; a non-OpenSSH `ssh` earlier on PATH is the likely cause");
    }
    message
}

async fn unresolved(key: &str) -> crate::error::Error {
    let version = ssh_version("ssh").await;
    anyhow!(unresolved_message(
        key,
        crate::local::shell_env::find_on_process_path("ssh").as_deref(),
        version.as_deref()
    ))
}

async fn ssh_version(program: &str) -> Option<String> {
    let version = Command::new(program)
        .arg("-V")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(std::time::Duration::from_secs(10), version)
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|output| {
            // OpenSSH prints its version to stderr; a wrapper may use stdout instead.
            [output.stderr, output.stdout]
                .into_iter()
                .find_map(|stream| {
                    String::from_utf8_lossy(&stream)
                        .lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                        .map(str::to_owned)
                })
        })
}

pub(super) fn probe_args() -> Vec<String> {
    [
        "-G",
        "-oControlPath=/tmp/orx-ssh-probe-%C",
        "-oControlMaster=no",
        "-oControlPersist=no",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

pub(super) const ROUTE_FIELDS: &str = "hostname|user|port|controlpath|proxyjump|proxycommand";

fn host(query: &Query) -> Result<&str> {
    query
        .args
        .last()
        .and_then(|dest| dest.rsplit('@').next())
        .ok_or_else(|| anyhow!("Missing SSH destination"))
}

pub(super) fn route_configuration(output: &str) -> String {
    // Match final can repeat unrelated list settings such as SendEnv.
    output
        .lines()
        .filter(|line| {
            line.split_once(' ')
                .is_some_and(|(key, _)| ROUTE_FIELDS.split('|').any(|field| field == key))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn configuration(args: &[String], verbose: bool) -> Result<std::process::Output> {
    let mut command = Command::new("ssh");
    if verbose {
        command.arg("-vv");
    }
    command
        .args(probe_args())
        .args(args)
        .env("ORX_SSH_PROBE", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| {
            anyhow!("Timed out resolving SSH configuration; check your Match exec commands")
        })??;
    if !output.status.success() {
        return Err(anyhow!(
            "Could not resolve SSH configuration: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output)
}

async fn query(args: Vec<String>) -> Result<Query> {
    let output = configuration(&args, false).await?;
    Ok(Query {
        args,
        output: route_configuration(&String::from_utf8(output.stdout)?),
    })
}

fn persistent_proxy(command: &str) -> String {
    // Closing the login PTY must not hang up the persistent proxy transport.
    let command = sh_quote(&format!("exec {command}"));
    format!(
        "/bin/sh -c {}",
        sh_quote(&format!(
            "trap '' HUP; exec \"${{SHELL:-/bin/sh}}\" -c {command}"
        ))
    )
}

fn snapshot_contents(
    queries: &[Query],
    proxies: &[Option<String>],
    config: &std::path::Path,
    system_include: bool,
) -> Result<String> {
    let mut contents = String::new();
    for (query, proxy) in queries.iter().zip(proxies) {
        contents.push_str(&format!("Host {}\n", config_quote(host(query)?)));
        for key in ["hostname", "user", "port"] {
            contents.push_str(&format!(
                "  {key} {}\n",
                config_quote(value(&query.output, key)?)
            ));
        }
        let jump = value(&query.output, "proxyjump").unwrap_or("none");
        if let Some(command) = proxy {
            contents.push_str(&format!("  ProxyCommand {}\n", persistent_proxy(command)));
        } else if jump != "none" {
            contents.push_str(&format!("  ProxyJump {}\n", config_quote(jump)));
        } else {
            contents.push_str("  ProxyCommand none\n");
        }
    }
    // The system config is re-included so `-F <snapshot>` keeps its settings,
    // but only when it was readable during resolution: inside a UID-mapped
    // user namespace its files look nobody-owned and OpenSSH rejects them.
    let system = if system_include {
        " /etc/ssh/ssh_config"
    } else {
        ""
    };
    contents.push_str(&format!(
        "Host *\nInclude {}{system}\n",
        config_quote(&config.to_string_lossy())
    ));
    Ok(contents)
}

fn jump_args(jump: &str) -> Result<Vec<String>> {
    let url = reqwest::Url::parse(&format!("ssh://{jump}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("Invalid ProxyJump destination: {jump}"))?;
    let mut args = Vec::new();
    if !url.username().is_empty() {
        args.extend(["-l".into(), url.username().into()]);
    }
    if let Some(port) = url.port() {
        args.extend(["-p".into(), port.to_string()]);
    }
    args.extend(["--".into(), host.into()]);
    Ok(args)
}

pub(super) async fn prepare(target: &SshTarget, refresh: bool) -> Result<Prepared> {
    if !refresh {
        if let Some(connection) = cached(target) {
            return Ok(connection);
        }
    }
    let _guard = PREPARATION.lock().await;
    if !refresh {
        if let Some(connection) = cached(target) {
            return Ok(connection);
        }
    }
    let connection = resolve(&target.dest, None).await?;
    CONNECTIONS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((control_dir(), target.dest.clone()), connection.clone());
    Ok(connection)
}

// OpenSSH requires the `-F` file to exist, so when the user has no config
// the retry falls back to an empty one under the owner-checked control dir.
fn user_or_empty_config(home: &std::path::Path) -> Result<PathBuf> {
    let config = home.join(".ssh/config");
    if config.exists() {
        return Ok(config);
    }
    super::prepare_control_dir()?;
    let empty = control_dir().join("user.config");
    if !empty.exists() {
        crate::local::git::atomic_write_with_mode(&empty, b"", Some(0o600))?;
    }
    Ok(empty)
}

async fn resolve(dest: &str, config: Option<&std::path::Path>) -> Result<Prepared> {
    resolve_with(dest, config, query).await
}

async fn resolve_with<F, Fut>(
    dest: &str,
    config: Option<&std::path::Path>,
    query: F,
) -> Result<Prepared>
where
    F: Fn(Vec<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Query>>,
{
    super::prepare_control_dir()?;
    let mut prefix: Vec<String> = config
        .map(|path| vec!["-F".into(), path.to_string_lossy().into_owned()])
        .unwrap_or_default();
    let mut args = prefix.clone();
    args.extend(["--".into(), dest.into()]);
    let mut snapshot_config = None;
    // When the default chain fails OpenSSH's owner check — a user namespace
    // (bwrap, managed agent sandboxes) shows root-owned /etc/ssh files as
    // nobody — retry against the user config alone, skipping the system
    // include chain; the same workaround as `ssh -F ~/.ssh/config`.
    let (first, skip_system_include) = match query(args).await {
        Ok(first) => (first, false),
        Err(error)
            if prefix.is_empty() && error.to_string().contains("Bad owner or permissions") =>
        {
            let home =
                dirs::home_dir().ok_or_else(|| anyhow!("Could not locate your home directory"))?;
            let fallback = user_or_empty_config(&home)?;
            prefix = vec!["-F".into(), fallback.to_string_lossy().into_owned()];
            let mut retry = prefix.clone();
            retry.extend(["--".into(), dest.into()]);
            match query(retry).await {
                Ok(first) => {
                    snapshot_config = Some(fallback);
                    (first, true)
                }
                Err(_) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    let host = host(&first)?.to_owned();
    if host.is_empty()
        || host.chars().any(|character| {
            character.is_control() || character.is_whitespace() || "*?,!".contains(character)
        })
    {
        return Err(anyhow!(
            "Cannot safely prepare SSH alias {host}; use a plain host alias"
        ));
    }
    let Ok(native_id) = value(&first.output, "controlpath") else {
        return Err(unresolved("controlpath").await);
    };
    let native_id = native_id
        .strip_prefix("/tmp/orx-ssh-probe-")
        .filter(|name| name.len() == 40 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| anyhow!("OpenSSH did not expand the connection identifier for {host}"))?
        .to_owned();
    let routed = value(&first.output, "proxyjump").unwrap_or("none") != "none"
        || value(&first.output, "proxycommand").unwrap_or("none") != "none";
    let mut queries = vec![first];
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < queries.len() {
        let jumps = value(&queries[index].output, "proxyjump")
            .unwrap_or("none")
            .to_owned();
        if jumps != "none" {
            for jump in jumps.split(',') {
                let mut args = prefix.clone();
                args.extend(jump_args(jump)?);
                if seen.insert(args.clone()) {
                    queries.push(query(args).await?);
                }
            }
        }
        index += 1;
    }
    let encoded = serde_json::to_vec(&queries)?;
    let digest = format!("{:x}", Sha256::digest(&encoded));
    let socket_name = if routed {
        digest[..40].to_owned()
    } else {
        "%C".into()
    };
    let snapshot = control_dir().join(format!("{digest}.config"));
    let config = match (config, snapshot_config) {
        (Some(path), _) => path.to_path_buf(),
        (None, Some(fallback)) => fallback,
        (None, None) => dirs::home_dir()
            .ok_or_else(|| anyhow!("Could not locate your home directory"))?
            .join(".ssh/config"),
    };
    let mut proxies: Vec<Option<String>> = queries
        .iter()
        .map(|query| {
            value(&query.output, "proxycommand")
                .ok()
                .filter(|command| *command != "none")
                .map(str::to_owned)
        })
        .collect();
    let contents = snapshot_contents(&queries, &proxies, &config, !skip_system_include)?;
    crate::local::git::atomic_write_with_mode(&snapshot, contents.as_bytes(), Some(0o600))?;
    for (index, query) in queries.iter().enumerate() {
        if value(&query.output, "proxyjump").unwrap_or("none") == "none" {
            continue;
        }
        let args = vec![
            "-F".into(),
            snapshot.to_string_lossy().into_owned(),
            "-l".into(),
            value(&query.output, "user")?.into(),
            "-p".into(),
            value(&query.output, "port")?.into(),
            self::host(query)?.into(),
        ];
        let output = configuration(&args, true).await?;
        let debug = String::from_utf8_lossy(&output.stderr);
        let command = debug
            .lines()
            .find_map(|line| {
                line.strip_prefix("debug1: Setting implicit ProxyCommand from ProxyJump: ")
            })
            .ok_or_else(|| {
                anyhow!(
                    "OpenSSH did not expose the ProxyJump transport for {}",
                    self::host(query).unwrap_or("target")
                )
            })?;
        proxies[index] = Some(command.to_owned());
    }
    let contents = snapshot_contents(&queries, &proxies, &config, !skip_system_include)?;
    crate::local::git::atomic_write_with_mode(&snapshot, contents.as_bytes(), Some(0o600))?;
    let path = control_dir().join(if routed { &socket_name } else { &native_id });
    let connection = Prepared {
        publication: Publication {
            host,
            native_id,
            socket_name,
            queries,
        },
        snapshot,
        path,
    };
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn ssh_version_prefers_stderr_and_falls_back_to_stdout() {
        use std::os::unix::fs::PermissionsExt;
        let temp = crate::local::git::TemporaryDirectory::new("orx-ssh-version").unwrap();
        let stub = |name: &str, body: &str| {
            let path = temp.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_string_lossy().into_owned()
        };
        let both = stub("both", "echo usage; echo; echo 'OpenSSH_9.6p1' >&2");
        let stdout_only = stub("stdout", "echo; echo 'ssh-wrapper 1.0'; exit 2");
        let silent = stub("silent", "exit 1");
        assert_eq!(ssh_version(&both).await.as_deref(), Some("OpenSSH_9.6p1"));
        assert_eq!(
            ssh_version(&stdout_only).await.as_deref(),
            Some("ssh-wrapper 1.0")
        );
        assert_eq!(ssh_version(&silent).await, None);
        assert_eq!(ssh_version("/nonexistent/orx-test-ssh").await, None);
    }

    #[test]
    fn snapshot_drops_system_include_when_it_failed_the_owner_check() {
        let query = Query {
            args: vec!["--".into(), "lab".into()],
            output:
                "hostname lab.invalid\nuser alice\nport 22\nproxyjump none\nproxycommand none\n"
                    .into(),
        };
        let proxies = vec![None];
        let config = Path::new("/home/me/.ssh/config");
        let with_system =
            snapshot_contents(std::slice::from_ref(&query), &proxies, config, true).unwrap();
        assert!(with_system.contains("Include \"/home/me/.ssh/config\" /etc/ssh/ssh_config"));
        let without_system =
            snapshot_contents(std::slice::from_ref(&query), &proxies, config, false).unwrap();
        assert!(without_system.ends_with("Include \"/home/me/.ssh/config\"\n"));
        assert!(!without_system.contains("/etc/ssh/ssh_config"));
    }

    fn owner_check_error() -> crate::error::Error {
        anyhow!("Could not resolve SSH configuration: Bad owner or permissions on /etc/ssh/ssh_config.d/50-drop.conf")
    }

    fn canned_output() -> String {
        format!(
            "hostname lab.invalid\nuser alice\nport 22\ncontrolpath /tmp/orx-ssh-probe-{}\nproxyjump none\nproxycommand none\n",
            "a".repeat(40)
        )
    }

    fn scripted_query<'a>(
        calls: &'a std::sync::Mutex<Vec<Vec<String>>>,
        outcomes: &'a std::sync::Mutex<std::collections::VecDeque<Result<String>>>,
    ) -> impl Fn(Vec<String>) -> std::future::Ready<Result<Query>> + 'a {
        move |args| {
            calls.lock().unwrap().push(args.clone());
            let outcome = outcomes.lock().unwrap().pop_front().unwrap();
            let output = outcome.map(|output| Query { args, output });
            std::future::ready(output)
        }
    }

    #[tokio::test]
    async fn owner_check_retries_with_the_user_config_and_drops_the_system_include() {
        let calls = std::sync::Mutex::new(Vec::new());
        let outcomes = std::sync::Mutex::new(
            [Err(owner_check_error()), Ok(canned_output())]
                .into_iter()
                .collect(),
        );
        let prepared = resolve_with("lab", None, scripted_query(&calls, &outcomes))
            .await
            .unwrap();
        let calls = calls.into_inner().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], vec!["--", "lab"]);
        let home = dirs::home_dir().unwrap();
        let fallback = user_or_empty_config(&home).unwrap();
        assert_eq!(
            calls[1],
            vec![
                "-F".to_owned(),
                fallback.to_string_lossy().into_owned(),
                "--".to_owned(),
                "lab".to_owned()
            ]
        );
        let snapshot = std::fs::read_to_string(&prepared.snapshot).unwrap();
        assert!(!snapshot.contains("/etc/ssh/ssh_config"));
        assert!(snapshot.contains(&format!(
            "Include {}",
            config_quote(&fallback.to_string_lossy())
        )));
    }

    #[tokio::test]
    async fn other_errors_do_not_retry() {
        let calls = std::sync::Mutex::new(Vec::new());
        let outcomes = std::sync::Mutex::new(
            [Err(anyhow!(
                "Could not resolve SSH configuration: ssh: Could not resolve hostname lab"
            ))]
            .into_iter()
            .collect(),
        );
        let error = resolve_with("lab", None, scripted_query(&calls, &outcomes))
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("Could not resolve hostname"));
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_failed_retry_returns_the_original_error() {
        let calls = std::sync::Mutex::new(Vec::new());
        let outcomes = std::sync::Mutex::new(
            [
                Err(owner_check_error()),
                Err(anyhow!("Could not resolve SSH configuration: retry broke")),
            ]
            .into_iter()
            .collect(),
        );
        let error = resolve_with("lab", None, scripted_query(&calls, &outcomes))
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("Bad owner or permissions"));
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_explicit_config_does_not_retry() {
        let calls = std::sync::Mutex::new(Vec::new());
        let outcomes = std::sync::Mutex::new([Err(owner_check_error())].into_iter().collect());
        let config = Path::new("/tmp/orx-test-explicit-config");
        let error = resolve_with("lab", Some(config), scripted_query(&calls, &outcomes))
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("Bad owner or permissions"));
        let calls = calls.into_inner().unwrap();
        assert_eq!(
            calls,
            vec![vec![
                "-F".to_owned(),
                "/tmp/orx-test-explicit-config".to_owned(),
                "--".to_owned(),
                "lab".to_owned()
            ]]
        );
    }

    #[test]
    fn user_or_empty_config_substitutes_an_empty_file_for_a_missing_user_config() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-fallback-config").unwrap();
        let fallback = user_or_empty_config(temp.path()).unwrap();
        assert_eq!(fallback, control_dir().join("user.config"));
        assert_eq!(std::fs::read_to_string(&fallback).unwrap(), "");
        std::fs::create_dir_all(temp.path().join(".ssh")).unwrap();
        let config = temp.path().join(".ssh/config");
        std::fs::write(&config, "Host somewhere\n").unwrap();
        assert_eq!(user_or_empty_config(temp.path()).unwrap(), config);
    }

    #[test]
    fn unresolved_key_names_the_ssh_binary_and_version() {
        assert_eq!(
            unresolved_message(
                "controlpath",
                Some(Path::new("/home/me/bin/ssh")),
                Some("ssh-wrapper 1.0")
            ),
            "OpenSSH did not resolve controlpath (ssh: /home/me/bin/ssh, version: ssh-wrapper 1.0); a non-OpenSSH `ssh` earlier on PATH is the likely cause"
        );
        assert_eq!(
            unresolved_message("controlpath", None, None),
            "OpenSSH did not resolve controlpath (ssh: not found on PATH, version: unknown); a non-OpenSSH `ssh` earlier on PATH is the likely cause"
        );
        assert_eq!(
            unresolved_message(
                "controlpath",
                Some(Path::new("/usr/bin/ssh")),
                Some("OpenSSH_9.6p1 Ubuntu-3ubuntu13")
            ),
            "OpenSSH did not resolve controlpath (ssh: /usr/bin/ssh, version: OpenSSH_9.6p1 Ubuntu-3ubuntu13)"
        );
    }

    #[tokio::test]
    async fn configured_routes_have_distinct_identities_and_remain_pinned_after_edits() {
        let temp = crate::local::git::TemporaryDirectory::new("orx-route").unwrap();
        let config = temp.path().join("config with spaces");
        let original = "Host direct equivalent gpu+cluster command-a command-b jump-a jump-b\n HostName example.invalid\n User alice\n Port 2222\n IdentityFile \"/tmp/key with spaces\"\nHost command-a\n ProxyCommand printf a\nHost command-b\n ProxyCommand printf b\nHost jump-a\n ProxyJump gateway-a\nHost jump-b\n ProxyJump gateway-b\nHost gateway-a\n HostName first.invalid\nHost gateway-b\n HostName second.invalid\n";
        std::fs::write(&config, original).unwrap();
        let direct = resolve("direct", Some(&config)).await.unwrap();
        let equivalent = resolve("equivalent", Some(&config)).await.unwrap();
        let literal = resolve("gpu+cluster", Some(&config)).await.unwrap();
        for destination in ["[::1]", "root@[::1]"] {
            let ipv6 = resolve(destination, Some(&config)).await.unwrap();
            assert_eq!(ipv6.publication.host, "[::1]");
        }
        let command_a = resolve("command-a", Some(&config)).await.unwrap();
        let command_b = resolve("command-b", Some(&config)).await.unwrap();
        let jump_a = resolve("jump-a", Some(&config)).await.unwrap();
        let jump_b = resolve("jump-b", Some(&config)).await.unwrap();
        assert_eq!(direct.path, equivalent.path);
        assert_eq!(direct.path, literal.path);
        assert_ne!(command_a.path, command_b.path);
        assert_ne!(jump_a.path, jump_b.path);
        std::fs::write(
            &config,
            format!("{original}Host *\nSendEnv ORX_ROUTE_TEST\nMatch final\n"),
        )
        .unwrap();
        assert_eq!(
            command_a.path,
            resolve("command-a", Some(&config)).await.unwrap().path
        );
        std::fs::write(
            &config,
            original
                .replace("first.invalid", "changed.invalid")
                .replace("printf a", "printf changed"),
        )
        .unwrap();
        assert_ne!(
            jump_a.path,
            resolve("jump-a", Some(&config)).await.unwrap().path
        );
        assert_ne!(
            command_a.path,
            resolve("command-a", Some(&config)).await.unwrap().path
        );
        let target = SshTarget::alias("command-a");
        let key = (control_dir(), target.dest.clone());
        let refreshed = resolve(&target.dest, Some(&config)).await.unwrap();
        CONNECTIONS.write().unwrap().insert(key.clone(), refreshed);
        let mut args = super::super::ssh_opts_prepared(&target, true, &command_a);
        args.extend(["--".into(), target.dest.clone()]);
        let pinned = Command::new("ssh")
            .arg("-G")
            .args(args)
            .env("ORX_SSH_PROBE", "1")
            .output()
            .await
            .unwrap();
        CONNECTIONS.write().unwrap().remove(&key);
        assert!(pinned.status.success());
        let pinned = String::from_utf8(pinned.stdout).unwrap();
        assert_eq!(
            value(&pinned, "controlpath").unwrap(),
            command_a.path.to_string_lossy()
        );
        assert!(value(&pinned, "proxycommand").unwrap().contains("printf a"));
        for (connection, dest, field, expected) in [
            (&command_a, "command-a", "proxycommand", "printf a"),
            (&jump_a, "gateway-a", "hostname", "first.invalid"),
        ] {
            let result = query(vec![
                "-F".into(),
                connection.snapshot.to_string_lossy().into_owned(),
                "--".into(),
                dest.into(),
            ])
            .await
            .unwrap();
            let selected = value(&result.output, field).unwrap();
            if field == "proxycommand" {
                assert!(selected.contains(expected));
                assert!(!selected.contains("printf changed"));
            } else {
                assert_eq!(selected, expected);
            }
            if dest == "command-a" {
                let native = configuration(&result.args, false).await.unwrap();
                assert!(String::from_utf8(native.stdout)
                    .unwrap()
                    .contains("identityfile /tmp/key with spaces\n"));
            }
        }
    }
}
