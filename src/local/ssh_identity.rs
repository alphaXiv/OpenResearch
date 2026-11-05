//! Does *this* machine hold a private key the boxes will accept?
//!
//! Boxes authorize the public keys registered on the account, but `orx` never
//! passes an identity — it shells out to `ssh`, which offers whatever the agent
//! and `~/.ssh` hold. So a box can be online, trusting a key the user registered
//! from another laptop, and every connection still dies with
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
    /// `None` for an agent-only key, which has no `.pub` to register.
    pub path: Option<PathBuf>,
}

/// What the account/machine comparison found.
#[derive(Debug, Clone)]
pub enum KeyStatus {
    Matched,
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

    // On-disk keys first, so a key the agent also holds keeps the `.pub` path
    // `preferred_local` needs.
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".ssh")) {
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("pub") {
                    continue;
                }
                let Ok(body) = tokio::fs::read_to_string(&path).await else {
                    continue;
                };
                for line in body.lines() {
                    let Some(fp) = fingerprint(line.trim()) else {
                        continue;
                    };
                    if seen.insert(fp.clone()) {
                        keys.push(LocalKey {
                            fingerprint: fp,
                            line: line.trim().to_string(),
                            path: Some(path.clone()),
                        });
                    }
                }
            }
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
                if let Some(fp) = fingerprint(line.trim()) {
                    if seen.insert(fp.clone()) {
                        keys.push(LocalKey {
                            fingerprint: fp,
                            line: line.trim().to_string(),
                            path: None,
                        });
                    }
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

    let mut unreadable = false;
    for key in registered {
        if fingerprint(&key.public_key).is_none() {
            unreadable = true;
            continue;
        }
        if is_local(&local, &key.public_key) {
            return KeyStatus::Matched;
        }
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

/// Turn a raw `Permission denied (publickey)` into a short reason. Matches on
/// the method list OpenSSH prints, not a bare "permission denied" — that is far
/// more likely to be the remote command hitting a read-only $HOME.
pub fn explain_launch_failure(sandbox_id: &str, err: &str) -> String {
    if !err.to_lowercase().contains("publickey") {
        // `orx runs` prints each failure on one line; ssh stderr is several.
        let one_line = err.split_whitespace().collect::<Vec<_>>().join(" ");
        return format!("Could not launch the run on box {sandbox_id}: {one_line}");
    }
    // No key path here — this runs in the supervisor with no view of ~/.ssh, and
    // `orx ssh-key list` names the file without us guessing at one.
    format!(
        "Could not launch the run on box {sandbox_id}: it refused this computer's SSH key. \
         Run `orx ssh-key list` to see which key to register, then relaunch."
    )
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
            },
            LocalKey {
                fingerprint: "ssh-ed25519 B".into(),
                line: "ssh-ed25519 B".into(),
                path: None,
            },
            LocalKey {
                fingerprint: "ssh-ed25519 C".into(),
                line: "ssh-ed25519 C".into(),
                path: Some("/h/.ssh/id_ed25519.pub".into()),
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
            }],
        );
        assert!(matches!(status, KeyStatus::Matched));
    }

    #[test]
    fn compare_reports_no_local_match_when_the_material_differs() {
        let status = compare(
            &[registered("work", "ssh-ed25519 AAAA work@laptop")],
            vec![LocalKey {
                fingerprint: "ssh-ed25519 BBBB".into(),
                line: "ssh-ed25519 BBBB home".into(),
                path: None,
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

    /// Real ssh stderr carries a host-key warning before the denial, and
    /// `orx runs` prints each failure on one line.
    #[test]
    fn a_publickey_denial_becomes_one_line_of_actionable_advice() {
        let msg = explain_launch_failure(
            "sb_1",
            "ssh root@h failed (exit 255):\nWarning: Permanently added 'h' to known hosts.\nroot@h: Permission denied (publickey).",
        );
        assert_eq!(msg.lines().count(), 1);
        // Points at the command that *names* the key rather than guessing a path
        // this code can't see.
        assert!(msg.contains("orx ssh-key list"));
        assert!(!msg.contains("id_ed25519"), "no guessed key path");
        assert!(
            !msg.contains("known hosts"),
            "drops the noise before the denial"
        );
    }

    /// The remote command's stderr rides the same string, so a read-only $HOME
    /// must not be diagnosed as an SSH key problem.
    #[test]
    fn other_failures_keep_the_original_error_and_get_no_key_advice() {
        for err in [
            "ssh: connect to host h port 22: Connection refused",
            "ssh root@h failed (exit 1): mkdir: cannot create directory '/root/.orx': Permission denied",
        ] {
            let msg = explain_launch_failure("sb_1", err);
            assert!(msg.contains(err), "keeps the cause: {err}");
            assert!(!msg.contains("ssh-key add"), "no key advice for: {err}");
        }
    }
}
