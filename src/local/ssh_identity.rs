//! Does *this* machine hold a private key the boxes will accept?
//!
//! Boxes authorize the public keys registered on the account, but `orx` never
//! passes an identity — it shells out to `ssh`, which offers whatever the agent
//! holds and the user's SSH config names. So a box can be online, trusting a key
//! the user registered from another laptop, and every connection still dies with
//! `Permission denied (publickey)`. Comparing the registered public keys against
//! the locally-available ones catches that before a box is ever billed.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::client::{list_ssh_keys, SshKey};
use crate::config::Credentials;

/// A public key reduced to the two fields that identify it: `type` + base64
/// blob. The trailing comment is user-editable and differs between the copy on
/// disk and the copy registered on the account, so it can't take part. Also
/// rejects the `environment="…" ssh-ed25519 …` form `sync-keys.ts` writes into
/// authorized_keys — the api must return unprefixed lines for matching to work.
pub fn fingerprint(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    let key_type = parts.next()?;
    let blob = parts.next()?;
    if !key_type.starts_with("ssh-")
        && !key_type.starts_with("ecdsa-")
        && !key_type.starts_with("sk-")
    {
        return None;
    }
    Some(format!("{key_type} {blob}"))
}

/// The `SHA256:…` form `ssh-keygen -l` prints, which is how people recognize a key.
pub fn sha256_fingerprint(line: &str) -> Option<String> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    use base64::Engine as _;
    use sha2::Digest as _;

    let key = fingerprint(line)?;
    let (_, blob) = key.split_once(' ')?;
    let digest = sha2::Sha256::digest(STANDARD.decode(blob).ok()?);
    Some(format!("SHA256:{}", STANDARD_NO_PAD.encode(digest)))
}

/// The trailing comment of a public key line, used to name the device.
pub fn key_comment(line: &str) -> Option<String> {
    let rest = line
        .split_whitespace()
        .skip(2)
        .collect::<Vec<_>>()
        .join(" ");
    (!rest.is_empty()).then_some(rest)
}

#[derive(Debug, Clone)]
pub struct LocalKey {
    pub fingerprint: String,
    pub line: String,
    /// `None` when no `.pub` sits beside it: an agent-only key, or a configured identity file ssh
    /// reads without a passphrase.
    pub path: Option<PathBuf>,
    /// The SSH agent holds it, so ssh signs with it without a prompt.
    pub agent: bool,
}

/// What the account/machine comparison found.
#[derive(Debug, Clone)]
pub enum KeyStatus {
    /// `matched` holds this computer's keys that are registered on the account.
    Matched {
        matched: Vec<LocalKey>,
    },
    NoLocalMatch {
        registered: Vec<String>,
        local: Vec<LocalKey>,
    },
    NoneRegistered {
        local: Vec<LocalKey>,
    },
    Unknown {
        reason: String,
    },
}

/// A key present as a `.pub` with no loaded private half can still
/// authenticate (ssh reads the private file directly), so both sources count.
pub async fn local_keys() -> Vec<LocalKey> {
    let mut seen = HashSet::new();
    let mut keys = Vec::new();
    let mut add = |body: &str, path: Option<PathBuf>| {
        for line in body.lines() {
            let Some(fp) = fingerprint(line.trim()) else {
                continue;
            };
            if seen.insert(fp.clone()) {
                keys.push(LocalKey {
                    fingerprint: fp,
                    line: line.trim().to_string(),
                    path: path.clone(),
                    agent: false,
                });
            }
        }
    };

    // On-disk keys first, so a key the agent also holds keeps the `.pub` path
    // `preferred_local` needs.
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".ssh")) {
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("pub") {
                    continue;
                }
                if let Ok(body) = tokio::fs::read_to_string(&path).await {
                    add(&body, Some(path));
                }
            }
        }
    }
    for private in offered_identities().await.unwrap_or_default() {
        let mut public = private.clone().into_os_string();
        public.push(".pub");
        let public = PathBuf::from(public);
        if let Ok(body) = tokio::fs::read_to_string(&public).await {
            add(&body, Some(public));
        } else if let Some(body) = unlocked_public_key(&private).await {
            add(&body, None);
        }
    }

    // A wedged SSH_AUTH_SOCK makes `ssh-add -L` hang; the on-disk keys above
    // are enough to advise with.
    let agent = tokio::time::timeout(
        Duration::from_secs(5),
        crate::process::tokio_command("ssh-add")
            .arg("-L")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    if let Ok(Ok(out)) = agent {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let Some(fp) = fingerprint(line.trim()) else {
                    continue;
                };
                if seen.insert(fp.clone()) {
                    keys.push(LocalKey {
                        fingerprint: fp,
                        line: line.trim().to_string(),
                        path: None,
                        agent: true,
                    });
                } else if let Some(key) = keys.iter_mut().find(|key| key.fingerprint == fp) {
                    key.agent = true;
                }
            }
        }
    }

    keys
}

/// This check only ever produces advice, so it must never outlast the thing
/// it's advising about — the shared http client has no timeout of its own.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Compare the account's registered keys against what this machine can offer.
/// Best-effort by construction: every failure path yields [`KeyStatus::Unknown`]
/// so a launch is never blocked by our inability to check.
pub async fn check(creds: &Credentials) -> KeyStatus {
    match tokio::time::timeout(CHECK_TIMEOUT, list_ssh_keys(creds)).await {
        Ok(Ok(k)) => compare(&k.ssh_keys, local_keys().await),
        Ok(Err(err)) => KeyStatus::Unknown {
            reason: err.to_string(),
        },
        Err(_) => KeyStatus::Unknown {
            reason: "timed out".to_string(),
        },
    }
}

/// Whether this machine can offer the private half of `public_key`. The one
/// place the match rule lives, so `orx ssh-key list` and the preflight can't
/// disagree about which key is here.
pub fn is_local(local: &[LocalKey], public_key: &str) -> bool {
    fingerprint(public_key).is_some_and(|fp| local.iter().any(|k| k.fingerprint == fp))
}

fn compare(registered: &[SshKey], local: Vec<LocalKey>) -> KeyStatus {
    if registered.is_empty() {
        return KeyStatus::NoneRegistered { local };
    }

    let unreadable = registered
        .iter()
        .any(|key| fingerprint(&key.public_key).is_none());
    let matched: Vec<LocalKey> = local
        .iter()
        .filter(|key| {
            registered
                .iter()
                .any(|registered| is_local(std::slice::from_ref(key), &registered.public_key))
        })
        .cloned()
        .collect();
    if !matched.is_empty() {
        return KeyStatus::Matched { matched };
    }

    // A key we couldn't parse might be the one that works, so "none of yours are
    // here" would be a guess — and it drives a prompt to register a redundant key.
    if unreadable {
        return KeyStatus::Unknown {
            reason: "a registered key is in an unrecognized format".to_string(),
        };
    }

    KeyStatus::NoLocalMatch {
        registered: registered.iter().map(|k| k.name.clone()).collect(),
        local,
    }
}

/// The key `orx ssh-key add` would register by default: prefer a modern ed25519
/// on disk, else any on-disk key (an agent-only key has no `.pub` to read).
pub fn preferred_local(local: &[LocalKey]) -> Option<&LocalKey> {
    local
        .iter()
        .find(|k| k.path.is_some() && k.fingerprint.starts_with("ssh-ed25519"))
        .or_else(|| local.iter().find(|k| k.path.is_some()))
}

/// What's wrong, for the launch preflight.
pub fn diagnosis(registered: &[String]) -> String {
    if registered.is_empty() {
        return "No SSH key is registered on your account.".to_string();
    }
    format!(
        "None of your registered SSH keys are on this computer.\n  Registered: {}",
        registered.join(", ")
    )
}

/// How to fix it, for the launch preflight.
pub fn remediation(registered: &[String], local: &[LocalKey]) -> String {
    // A lone action doesn't need a header and a bullet to introduce it.
    if let (Some(key), true) = (preferred_local(local), registered.is_empty()) {
        let pub_path = key.path.as_deref().map(tilde).unwrap_or_default();
        return format!("Register this computer:  orx ssh-key add {pub_path}\n");
    }
    let mut out = String::from("To fix:\n");
    match preferred_local(local) {
        Some(key) => {
            let pub_path = key.path.as_deref().map(tilde).unwrap_or_default();
            out.push_str(&format!(
                "  • Register this computer:   orx ssh-key add {pub_path}\n"
            ));
            if !registered.is_empty() {
                out.push_str("  • Or load a registered key: ssh-add <its private key>\n");
            }
        }
        // Nothing on disk to register. With keys already on the account, loading
        // one is likelier to be the fix than generating yet another.
        None if !registered.is_empty() => out.push_str(
            "  • Load a registered key:    ssh-add <its private key>\n  • Or make a new one:        ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519\n                              orx ssh-key add ~/.ssh/id_ed25519.pub\n",
        ),
        None => out.push_str(
            "  • Create a key:             ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519\n  • Then register it:         orx ssh-key add ~/.ssh/id_ed25519.pub\n",
        ),
    }
    out
}

/// `~/…` for a path under the home dir, so a command stays copy-pasteable
/// without overflowing the line.
pub fn tilde(path: &std::path::Path) -> String {
    dirs::home_dir()
        .and_then(|home| path.strip_prefix(&home).ok())
        .map(|rel| format!("~/{}", rel.display()))
        .unwrap_or_else(|| path.display().to_string())
}

/// The private key files ssh offers an `orx-box-*` host under the user's SSH config, as
/// `ssh -G` resolves them. `None` when ssh can't say, or when what it offers depends on the box
/// (`%h`, `%r`, …) or comes from an agent or token `ssh-add` doesn't reach.
pub async fn offered_identities() -> Option<Vec<PathBuf>> {
    let home = dirs::home_dir()?;
    let config = home.join(".ssh").join("config");
    let resolved = tokio::time::timeout(
        Duration::from_secs(5),
        crate::process::tokio_command("ssh")
            .arg("-G")
            .arg("-F")
            .arg(if config.is_file() {
                config.as_os_str()
            } else {
                "none".as_ref()
            })
            .arg("orx-box-preflight")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(output)) = resolved else {
        return None;
    };
    if !output.status.success() {
        return None;
    }

    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((option, value)) = line.split_once(' ') else {
            continue;
        };
        match option {
            "identityfile" => {
                let mut path = String::new();
                let mut chars = value.chars();
                while let Some(char) = chars.next() {
                    if char != '%' {
                        path.push(char);
                        continue;
                    }
                    match chars.next() {
                        Some('%') => path.push('%'),
                        Some('d') => path.push_str(&home.to_string_lossy()),
                        _ => return None,
                    }
                }
                files.push(match path.strip_prefix("~/") {
                    Some(relative) => home.join(relative),
                    None => PathBuf::from(path),
                });
            }
            "identityagent" if value != "SSH_AUTH_SOCK" => return None,
            "pkcs11provider" if value != "none" => return None,
            _ => {}
        }
    }
    Some(files)
}

/// The public half of `private` when ssh can read it without a passphrase.
async fn unlocked_public_key(private: &std::path::Path) -> Option<String> {
    let output = probe_private_key(private).await?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn probe_private_key(private: &std::path::Path) -> Option<std::process::Output> {
    tokio::time::timeout(
        Duration::from_secs(5),
        crate::process::tokio_command("ssh-keygen")
            .args(["-y", "-P", "", "-f"])
            .arg(private)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()
}

/// A private key file from `matched` when ssh, which never prompts for orx, can use none of
/// them: none is in the agent and each file needs a passphrase. `None` when one is usable or a
/// check fails.
pub async fn locked(matched: &[LocalKey]) -> Option<PathBuf> {
    let mut locked = None;
    for key in matched {
        let public = key.path.as_deref().filter(|_| !key.agent)?;
        let private = public.with_extension("");
        let output = probe_private_key(&private).await?;
        if output.status.success()
            || !String::from_utf8_lossy(&output.stderr).contains("incorrect passphrase")
        {
            return None;
        }
        locked.get_or_insert(private);
    }
    locked
}

/// How to let ssh use the passphrase-protected `private` key without a prompt.
pub fn unlock_hint(private: &std::path::Path) -> String {
    if cfg!(windows) {
        format!(
            "Start the ssh-agent service (in an administrator PowerShell: `Get-Service ssh-agent | Set-Service -StartupType Automatic; Start-Service ssh-agent`), then load the key with `ssh-add {}`",
            private.display()
        )
    } else if cfg!(target_os = "macos") {
        format!(
            "Load it into your SSH agent with `ssh-add --apple-use-keychain {}`",
            tilde(private)
        )
    } else {
        format!(
            "Load it into your SSH agent with `ssh-add {}`",
            tilde(private)
        )
    }
}

/// Why box `sandbox_id` refused this computer's SSH key, and the fix, on one line.
pub async fn explain_refusal(creds: &Credentials, sandbox_id: &str) -> String {
    let cause = match check(creds).await {
        KeyStatus::Matched { matched } => match locked(&matched).await {
            Some(private) => format!(
                "{} needs its passphrase, and no SSH agent holds it. {}, then relaunch.",
                tilde(&private),
                unlock_hint(&private)
            ),
            None => format!(
                "the box never accepted {}, which is registered on your account. Relaunch the run; if a box refuses it again, pass a different `--provider`.",
                matched
                    .iter()
                    .filter_map(|key| {
                        key.path
                            .as_deref()
                            .map(tilde)
                            .or_else(|| sha256_fingerprint(&key.line))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
        KeyStatus::NoLocalMatch { registered, .. } => format!(
            "none of your registered keys ({}) are on this computer. Register this computer's key with `orx ssh-key add`, then relaunch.",
            registered.join(", ")
        ),
        KeyStatus::NoneRegistered { .. } => "no SSH key is registered on your account. Register this computer's key with `orx ssh-key add`, then relaunch.".into(),
        KeyStatus::Unknown { .. } => "run `orx ssh-key list` to see which key to register, then relaunch.".into(),
    };
    format!("Box {sandbox_id} refused this computer's SSH key: {cause}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_ignores_the_comment() {
        let a = fingerprint("ssh-ed25519 AAAAC3NzaC1 alice@laptop").unwrap();
        let b = fingerprint("ssh-ed25519 AAAAC3NzaC1 bob@desktop").unwrap();
        assert_eq!(a, b, "same key material, different comment");
        assert_eq!(a, "ssh-ed25519 AAAAC3NzaC1");
    }

    #[test]
    fn fingerprint_accepts_the_formats_the_api_allows() {
        for line in [
            "ssh-rsa AAAAB3Nza x",
            "ssh-ed25519 AAAAC3 x",
            "ecdsa-sha2-nistp256 AAAAE2 x",
            "sk-ssh-ed25519@openssh.com AAAAG x",
        ] {
            assert!(fingerprint(line).is_some(), "should parse: {line}");
        }
    }

    #[test]
    fn fingerprint_rejects_junk() {
        assert!(fingerprint("").is_none());
        assert!(fingerprint("ssh-ed25519").is_none(), "no blob");
        assert!(fingerprint("not-a-key AAAA x").is_none());
    }

    #[test]
    fn sha256_fingerprint_matches_ssh_keygen() {
        assert_eq!(
            sha256_fingerprint(
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE5jEj+4KP1+kf1xBWmjNx96KLFUkjreQxCOrA08NQEJ t"
            )
            .as_deref(),
            Some("SHA256:pU0aHZujtAnVqRHnpcOVxPe07Yft4bANE4UgHDCaYL4")
        );
        assert_eq!(sha256_fingerprint("ssh-ed25519 !!!"), None);
    }

    #[test]
    fn comment_is_the_third_field_onward() {
        assert_eq!(
            key_comment("ssh-ed25519 AAAA alice@laptop").as_deref(),
            Some("alice@laptop")
        );
        assert_eq!(key_comment("ssh-ed25519 AAAA").as_deref(), None);
    }

    #[test]
    fn preferred_local_prefers_an_on_disk_ed25519() {
        let keys = vec![
            LocalKey {
                fingerprint: "ssh-rsa A".into(),
                line: "ssh-rsa A".into(),
                path: Some("/h/.ssh/id_rsa.pub".into()),
                agent: false,
            },
            LocalKey {
                fingerprint: "ssh-ed25519 B".into(),
                line: "ssh-ed25519 B".into(),
                path: None,
                agent: true,
            },
            LocalKey {
                fingerprint: "ssh-ed25519 C".into(),
                line: "ssh-ed25519 C".into(),
                path: Some("/h/.ssh/id_ed25519.pub".into()),
                agent: false,
            },
        ];
        assert_eq!(preferred_local(&keys).unwrap().fingerprint, "ssh-ed25519 C");
    }

    #[test]
    fn preferred_local_skips_agent_only_keys() {
        let keys = vec![LocalKey {
            fingerprint: "ssh-ed25519 B".into(),
            line: "ssh-ed25519 B".into(),
            path: None,
            agent: true,
        }];
        assert!(preferred_local(&keys).is_none(), "no .pub to register");
    }

    fn registered(name: &str, public_key: &str) -> SshKey {
        SshKey {
            id: "k1".into(),
            name: name.into(),
            public_key: public_key.into(),
        }
    }

    #[test]
    fn compare_matches_on_key_material_despite_a_different_comment() {
        let status = compare(
            &[registered("sam@Mac.lan", "ssh-ed25519 AAAA sam@Mac.lan")],
            vec![LocalKey {
                fingerprint: "ssh-ed25519 AAAA".into(),
                line: "ssh-ed25519 AAAA someone-else@host".into(),
                path: None,
                agent: true,
            }],
        );
        assert!(matches!(status, KeyStatus::Matched { .. }));
    }

    #[test]
    fn compare_reports_no_local_match_when_the_material_differs() {
        let status = compare(
            &[registered("work", "ssh-ed25519 AAAA work@laptop")],
            vec![LocalKey {
                fingerprint: "ssh-ed25519 BBBB".into(),
                line: "ssh-ed25519 BBBB home".into(),
                path: None,
                agent: true,
            }],
        );
        match status {
            KeyStatus::NoLocalMatch { registered, .. } => assert_eq!(registered, ["work"]),
            other => panic!("expected NoLocalMatch, got {other:?}"),
        }
    }

    /// An unreadable key could be the one that works, so claiming "none of
    /// yours are here" would prompt the user to register a redundant key.
    #[test]
    fn compare_is_unsure_when_a_registered_key_cannot_be_parsed() {
        assert!(matches!(
            compare(
                &[registered("legacy", "-----BEGIN PUBLIC KEY-----")],
                Vec::new()
            ),
            KeyStatus::Unknown { .. }
        ));
    }

    #[test]
    fn compare_reports_none_registered_on_an_empty_account() {
        assert!(matches!(
            compare(&[], Vec::new()),
            KeyStatus::NoneRegistered { .. }
        ));
    }

    fn on_disk_key() -> Vec<LocalKey> {
        vec![LocalKey {
            fingerprint: "ssh-ed25519 C".into(),
            line: "ssh-ed25519 C".into(),
            path: Some("/h/.ssh/id_ed25519.pub".into()),
            agent: false,
        }]
    }

    #[test]
    fn remediation_offers_registration_when_a_local_key_exists() {
        let msg = remediation(&["sam@Mac.lan".to_string()], &on_disk_key());
        assert!(msg.contains("orx ssh-key add /h/.ssh/id_ed25519.pub"));
        assert!(msg.contains("ssh-add"), "offers loading the registered key");
    }

    /// With nothing registered there is no key to `ssh-add`, so offering it
    /// would send the user after something that doesn't exist — and a lone
    /// action doesn't need a "To fix:" header to introduce it.
    #[test]
    fn remediation_is_a_single_line_when_only_one_action_applies() {
        let msg = remediation(&[], &on_disk_key());
        assert!(msg.contains("orx ssh-key add /h/.ssh/id_ed25519.pub"));
        assert!(!msg.contains("ssh-add <"), "nothing registered to load");
        assert!(!msg.contains("To fix:"), "no header for one action");
        assert_eq!(msg.lines().count(), 1);
    }

    /// Keys registered but none on disk: loading one beats generating a third.
    #[test]
    fn remediation_prefers_loading_over_generating_when_keys_are_registered() {
        let msg = remediation(&["work".to_string()], &[]);
        let load = msg.find("ssh-add").expect("offers ssh-add");
        let generate = msg.find("ssh-keygen").expect("still offers keygen");
        assert!(load < generate, "load comes first");
    }

    #[test]
    fn remediation_falls_back_to_keygen_with_no_local_key() {
        let msg = remediation(&[], &[]);
        assert!(msg.contains("ssh-keygen -t ed25519"));
        assert!(msg.contains("orx ssh-key add ~/.ssh/id_ed25519.pub"));
    }

    /// The agent is how a passphrase-protected key becomes usable without a prompt.
    #[cfg(unix)]
    #[tokio::test]
    async fn keys_the_ssh_config_offers_boxes_count_as_local() {
        if !crate::config::in_test_child(&["SSH_AUTH_SOCK"]) {
            return;
        }
        let home = crate::config::test_home();
        std::env::set_var("HOME", &home);
        std::env::remove_var("SSH_AUTH_SOCK");
        let (ssh, keys) = (home.join(".ssh"), home.join("keys"));
        let key = |path: PathBuf, passphrase: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            assert!(crate::process::command("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", passphrase, "-f"])
                .arg(&path)
                .status()
                .unwrap()
                .success());
            std::fs::read_to_string(path.with_extension("pub")).unwrap()
        };
        let old = key(ssh.join("id_ed25519"), "secret");
        let work = key(keys.join("work"), "");
        let bare = key(keys.join("bare"), "");
        std::fs::remove_file(keys.join("bare.pub")).unwrap();
        let config = ssh.join("config");
        std::fs::write(
            &config,
            "Host *\n  IdentityFile ~/keys/work\n  IdentityFile %d/keys/bare\n",
        )
        .unwrap();

        let local = local_keys().await;
        for public in [&old, &work, &bare] {
            assert!(is_local(&local, public), "{local:?}");
        }
        assert_eq!(
            locked(&local).await,
            None,
            "a configured key needs no passphrase"
        );
        assert!(offered_identities()
            .await
            .unwrap()
            .contains(&keys.join("work")));

        for unresolved in [
            "Host *\n  IdentityFile ~/keys/%r\n",
            "Host *\n  IdentityAgent ~/another-agent.sock\n",
        ] {
            std::fs::write(&config, unresolved).unwrap();
            assert_eq!(offered_identities().await, None, "{unresolved}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_passphrase_key_is_locked_until_an_agent_holds_it() {
        if !crate::config::in_test_child(&["SSH_AUTH_SOCK"]) {
            return;
        }
        let home = crate::config::test_home();
        std::env::set_var("HOME", &home);
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let key = |name: &str, passphrase: &str| {
            assert!(crate::process::command("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", passphrase, "-f"])
                .arg(ssh.join(name))
                .status()
                .unwrap()
                .success());
        };
        key("id_ed25519", "secret");
        key("spare", "");
        let mut local = local_keys().await;
        local.sort_by_key(|key| key.path.clone());
        let (locked_key, spare) = (&local[..1], &local[1..]);
        assert_eq!(
            locked(locked_key).await,
            Some(ssh.join("id_ed25519")),
            "{local:?}"
        );
        assert_eq!(locked(spare).await, None);
        assert_eq!(locked(&local).await, None, "one usable key is enough");

        // Unix socket paths are capped near 100 bytes.
        let socket = PathBuf::from(format!(
            "/tmp/orx-agent-{}",
            &uuid::Uuid::new_v4().to_string()[..8]
        ));
        let _agent = crate::process::tokio_command("ssh-agent")
            .arg("-D")
            .arg("-a")
            .arg(&socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !socket.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("ssh-agent listens");
        let askpass = home.join("askpass");
        std::fs::write(&askpass, "#!/bin/sh\necho secret\n").unwrap();
        std::fs::set_permissions(
            &askpass,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::env::set_var("SSH_AUTH_SOCK", &socket);
        assert!(crate::process::command("ssh-add")
            .arg(ssh.join("id_ed25519"))
            .env("SSH_ASKPASS", &askpass)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", ":0")
            .stdin(Stdio::null())
            .status()
            .unwrap()
            .success());
        let local = local_keys().await;
        let held: Vec<_> = local.iter().filter(|key| key.agent).collect();
        assert_eq!(held.len(), 1, "{local:?}");
        assert_eq!(locked(&local[..]).await, None);
        assert_eq!(locked(&[held[0].clone()]).await, None);
        let _ = std::fs::remove_file(socket);
    }
}
