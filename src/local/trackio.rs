//! Optional Trackio experiment tracking against a server the user already runs.
//!
//! Orx never starts, tunnels or supervises a Trackio server — this is purely a
//! connection contract. Configuration is three variables in the synced env file
//! (see [`crate::config`]), so it already reaches every compute backend and the
//! research agent; this module adds the parts that need to know what Trackio
//! *is*: a reachability probe and the dashboard link.
//!
//! The write token is a secret. Nothing here returns it, and every string that
//! could have picked it up out of a user-supplied URL goes through
//! [`redact_write_token`] first.

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;
use serde_json::json;

/// The configured connection, or `None` when Trackio is switched off. The
/// token is deliberately reduced to a bool — callers never need the value.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub server_url: String,
    pub project: Option<String>,
    pub has_token: bool,
}

impl Config {
    /// The dashboard URL for the configured project (project-scoped: Trackio
    /// has no per-run query parameter).
    pub fn dashboard_url(&self) -> String {
        crate::config::trackio_dashboard_url(&self.server_url, self.project.as_deref())
    }
}

/// Read the connection from the process environment and the synced env file.
pub fn config() -> Option<Config> {
    let server_url = crate::config::trackio_server_url()?;
    Some(Config {
        server_url,
        project: crate::config::trackio_project(),
        has_token: crate::config::trackio_write_token().is_some(),
    })
}

/// What a probe found. `write_access` is `None` when the question could not be
/// asked (server unreachable, or not a Trackio server).
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub reachable: bool,
    /// The Trackio version the server reports. `None` from a server that
    /// answered but is not Trackio.
    pub version: Option<String>,
    pub write_access: Option<bool>,
    pub error: Option<String>,
}

/// One consistent verdict for settings, launch previews and CLI output.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// A reachable endpoint that identifies itself as Trackio.
    pub configured: bool,
    /// Ready to receive this run's metrics, not merely reachable.
    pub usable: bool,
    pub reason: Option<String>,
}

pub fn verdict(config: Option<&Config>, probe: Option<&Probe>, remote: bool) -> Verdict {
    let Some(config) = config else {
        return Verdict {
            configured: false,
            usable: false,
            reason: Some("Trackio is not configured — set TRACKIO_SERVER_URL.".to_string()),
        };
    };
    let Some(probe) = probe else {
        return Verdict {
            configured: false,
            usable: false,
            reason: Some("Trackio has not been checked yet.".to_string()),
        };
    };
    if !probe.reachable || probe.version.is_none() {
        return Verdict {
            configured: false,
            usable: false,
            reason: probe
                .error
                .clone()
                .or_else(|| Some("The configured endpoint is not a Trackio server.".to_string())),
        };
    }
    if remote && crate::config::is_local_only_trackio_url(&config.server_url) {
        return Verdict {
            configured: true,
            usable: false,
            reason: Some(format!(
                "{} is local-only and cannot be reached by a remote backend; the run will launch without Trackio.",
                config.server_url
            )),
        };
    }
    if config
        .project
        .as_deref()
        .is_none_or(|project| project.trim().is_empty())
    {
        return Verdict {
            configured: true,
            usable: false,
            reason: Some("Set TRACKIO_PROJECT before launching a tracked run.".to_string()),
        };
    }
    match probe.write_access {
        Some(true) => {}
        Some(false) => {
            return Verdict {
                configured: true,
                usable: false,
                reason: Some(
                    "The Trackio write token was rejected; runs will not be able to log."
                        .to_string(),
                ),
            };
        }
        None => {
            return Verdict {
                configured: true,
                usable: false,
                reason: Some(if config.has_token {
                    "Trackio write access could not be confirmed; the connection is not usable until the token can be verified."
                        .to_string()
                } else {
                    "TRACKIO_WRITE_TOKEN is not set and write access could not be confirmed."
                        .to_string()
                }),
            };
        }
    }
    Verdict {
        configured: true,
        usable: true,
        reason: None,
    }
}

/// Replace the value of any `write_token` query parameter with `***`.
///
/// Trackio's own write-access URL embeds the token, so it can arrive inside a
/// URL a user typed, and from there into a `reqwest` error message. Orx splits
/// the token out when the URL is saved through the dashboard, but a variable
/// set directly in the environment never passes through that path.
pub fn redact_write_token(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("write_token=") {
        let (before, after) = rest.split_at(at + "write_token=".len());
        out.push_str(before);
        out.push_str("***");
        // The value runs to the next delimiter; keep whatever follows it.
        let end = after
            .find(['&', '#', ' ', '"', '\''])
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// A Trackio server on this machine bypasses `HTTP(S)_PROXY`, which reqwest would
/// otherwise apply to 127.0.0.1 too; a remote one goes through the proxy.
fn http(base: &str) -> &'static Client {
    static LOOPBACK: OnceLock<Client> = OnceLock::new();
    static REMOTE: OnceLock<Client> = OnceLock::new();
    let (client, builder): (_, fn() -> reqwest::ClientBuilder) =
        if crate::config::is_local_only_trackio_url(base) {
            (&LOOPBACK, crate::net::loopback_client)
        } else {
            (&REMOTE, crate::net::remote_client)
        };
    client.get_or_init(|| {
        builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client")
    })
}

/// Ask a Trackio server whether it is there and whether the token can write.
///
/// Two calls, both read-only:
/// * `GET /version` — Trackio's own identity endpoint. A 200 with a `version`
///   field is the difference between "a Trackio server" and "something is
///   listening on that port".
/// * `POST /api/get_run_mutation_status` — reports whether the caller may
///   mutate runs, which is exactly the write-token check, and changes nothing.
pub async fn probe(server_url: &str, write_token: Option<&str>) -> Probe {
    let base = server_url.trim().trim_end_matches('/');
    let version = match http(base).get(format!("{base}/version")).send().await {
        Ok(res) if res.status().is_success() => {
            let version = res
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|b| b.get("version").and_then(|v| v.as_str()).map(String::from));
            // A 200 with no `version` is some other service on that port.
            // Reporting it as "reachable, unknown version" would send the user
            // looking for a Trackio problem that isn't there.
            let Some(version) = version else {
                return Probe {
                    reachable: true,
                    version: None,
                    write_access: None,
                    error: Some(format!(
                        "{} answered /version without a Trackio version — is this a Trackio server?",
                        redact_write_token(base)
                    )),
                };
            };
            Some(version)
        }
        Ok(res) => {
            return Probe {
                reachable: true,
                version: None,
                write_access: None,
                error: Some(format!(
                    "{} answered {} on /version — is this a Trackio server?",
                    redact_write_token(base),
                    res.status().as_u16()
                )),
            };
        }
        Err(e) => {
            return Probe {
                reachable: false,
                version: None,
                write_access: None,
                error: Some(format!(
                    "Could not reach Trackio at {}: {}",
                    redact_write_token(base),
                    redact_write_token(&e.to_string())
                )),
            };
        }
    };

    let mut req = http(base)
        .post(format!("{base}/api/get_run_mutation_status"))
        .json(&json!({}));
    if let Some(token) = write_token {
        req = req.header("X-Trackio-Write-Token", token);
    }
    let write_access = match req.send().await {
        Ok(res) if res.status().is_success() => res
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|b| b.pointer("/data/allowed").and_then(|v| v.as_bool())),
        _ => None,
    };

    Probe {
        reachable: true,
        version,
        write_access,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_write_token_hides_the_value_and_keeps_the_rest() {
        assert_eq!(
            redact_write_token("http://h:7860/?write_token=s3cret&project=demo"),
            "http://h:7860/?write_token=***&project=demo"
        );
    }

    #[test]
    fn redact_write_token_handles_a_trailing_token_and_repeats() {
        assert_eq!(
            redact_write_token("error at http://h/?write_token=abc"),
            "error at http://h/?write_token=***"
        );
        assert_eq!(
            redact_write_token("?write_token=a&x=1 and ?write_token=b&y=2"),
            "?write_token=***&x=1 and ?write_token=***&y=2"
        );
    }

    #[test]
    fn redact_write_token_leaves_clean_text_alone() {
        let clean = "Could not reach Trackio at http://127.0.0.1:7860";
        assert_eq!(redact_write_token(clean), clean);
    }

    #[test]
    fn trackio_verdict_rejects_unreachable_and_non_trackio_endpoints() {
        let config = Config {
            server_url: "http://127.0.0.1:7860".to_string(),
            project: Some("demo".to_string()),
            has_token: true,
        };
        for probe in [
            Probe {
                reachable: false,
                version: None,
                write_access: None,
                error: Some("unreachable".to_string()),
            },
            Probe {
                reachable: true,
                version: None,
                write_access: None,
                error: Some("not Trackio".to_string()),
            },
        ] {
            let verdict = verdict(Some(&config), Some(&probe), false);
            assert!(!verdict.configured);
            assert!(!verdict.usable);
            assert_eq!(verdict.reason, probe.error);
        }
    }

    #[test]
    fn trackio_verdict_distinguishes_reachable_from_usable() {
        let probe = Probe {
            reachable: true,
            version: Some("0.35.0".to_string()),
            write_access: Some(true),
            error: None,
        };
        let missing_project = Config {
            server_url: "https://trackio.example".to_string(),
            project: None,
            has_token: true,
        };
        let missing_project_verdict = verdict(Some(&missing_project), Some(&probe), false);
        assert!(missing_project_verdict.configured);
        assert!(!missing_project_verdict.usable);
        assert!(missing_project_verdict
            .reason
            .unwrap()
            .contains("TRACKIO_PROJECT"));

        let configured = Config {
            server_url: "https://trackio.example".to_string(),
            project: Some("demo".to_string()),
            has_token: true,
        };
        let unknown_write_access = Probe {
            reachable: true,
            version: Some("0.35.0".to_string()),
            write_access: None,
            error: None,
        };
        let verdict = verdict(Some(&configured), Some(&unknown_write_access), false);
        assert!(verdict.configured);
        assert!(!verdict.usable);
        assert!(verdict.reason.unwrap().contains("could not be confirmed"));
    }
}
