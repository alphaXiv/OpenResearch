//! Reads the saved Trackio variables for the Settings check, run links and launch warning.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

pub const SERVER_URL: &str = "TRACKIO_SERVER_URL";
const PROJECT: &str = "TRACKIO_PROJECT";
pub const WRITE_TOKEN: &str = "TRACKIO_WRITE_TOKEN";

/// The saved server URL, stripped to scheme, host, port and path.
pub fn server_url() -> Option<String> {
    crate::config::synced_env_var(SERVER_URL).and_then(|raw| base_url(&raw))
}

pub fn project() -> Option<String> {
    crate::config::synced_env_var(PROJECT)
}

/// The value the Environment page may show and edit in the clear; `None` keeps it masked.
pub fn plain_value(key: &str, value: &str) -> Option<String> {
    match key {
        // An unparseable URL is shown as typed unless it might carry a token.
        SERVER_URL => {
            base_url(value).or_else(|| (!value.contains(['?', '@'])).then(|| value.to_string()))
        }
        PROJECT => Some(value.to_string()),
        _ => None,
    }
}

/// Drops the query (`trackio.show()` URLs carry `?write_token=`), fragment and
/// credentials, so the result is safe to show, link to, and log.
fn base_url(raw: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    Some(url.as_str().trim_end_matches('/').to_string())
}

/// True for an address only this machine can reach, including Gradio's `0.0.0.0` bind address.
pub fn is_local_only(url: &str) -> bool {
    crate::local::local_models::is_loopback_url(url)
        || reqwest::Url::parse(url).is_ok_and(|url| {
            url.host_str()
                .and_then(|host| {
                    host.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .ok()
                })
                .is_some_and(|ip| ip.is_unspecified())
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub reachable: bool,
    pub version: Option<String>,
    /// `None` when no write token is saved or the check failed (see `error`).
    pub write_access: Option<bool>,
    pub error: Option<String>,
}

/// `GET /version`, then (with a token) Trackio's read-only write-access check.
pub async fn preflight(base: &str, token: Option<&str>) -> Preflight {
    let failed = |error: String| Preflight {
        reachable: false,
        version: None,
        write_access: None,
        error: Some(error),
    };
    let builder = if is_local_only(base) {
        crate::net::loopback_client()
    } else {
        crate::net::remote_client()
    };
    // No redirects: the write token must only ever reach the saved host.
    let client = match builder
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => return failed(error.to_string()),
    };
    let version = match client.get(format!("{base}/version")).send().await {
        Ok(res) if res.status().is_success() => res
            .json::<Value>()
            .await
            .ok()
            .and_then(|body| body["version"].as_str().map(str::to_string)),
        Ok(res) => {
            return failed(format!(
                "{base}/version answered {}; is this a Trackio server?",
                res.status()
            ))
        }
        Err(error) => return failed(format!("Could not reach {base}: {}", error.without_url())),
    };
    let Some(version) = version else {
        return failed(format!("{base}/version did not identify a Trackio server"));
    };
    let reachable = |write_access, error| Preflight {
        reachable: true,
        version: Some(version.clone()),
        write_access,
        error,
    };
    let Some(token) = token else {
        return reachable(None, None);
    };
    let check = async {
        let res = client
            .post(format!("{base}/api/get_run_mutation_status"))
            .header("X-Trackio-Write-Token", token)
            .json(&json!({}))
            .send()
            .await
            .map_err(|error| error.without_url().to_string())?;
        if !res.status().is_success() {
            return Err(format!("it answered {}", res.status()));
        }
        res.json::<Value>()
            .await
            .ok()
            .and_then(|body| body["data"]["allowed"].as_bool())
            .ok_or_else(|| "its answer had no `allowed` field".to_string())
    };
    match check.await {
        Ok(allowed) => reachable(Some(allowed), None),
        Err(error) => reachable(
            None,
            Some(format!("Could not check the write token: {error}")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_drops_tokens_and_credentials() {
        assert_eq!(
            base_url(" https://user:pw@trackio.example:7860/?write_token=secret#x ").as_deref(),
            Some("https://trackio.example:7860")
        );
        assert_eq!(
            base_url("http://10.0.0.5:7860/trackio/").as_deref(),
            Some("http://10.0.0.5:7860/trackio")
        );
        assert_eq!(base_url("localhost:7860"), None);
        assert_eq!(
            plain_value(SERVER_URL, "http://h:7860/?write_token=secret").as_deref(),
            Some("http://h:7860")
        );
        assert_eq!(plain_value(WRITE_TOKEN, "secret"), None);
        assert_eq!(
            plain_value(SERVER_URL, "localhost:7860").as_deref(),
            Some("localhost:7860")
        );
        assert_eq!(plain_value(SERVER_URL, "h:7860?write_token=secret"), None);
        assert_eq!(base_url("ftp://trackio.example"), None);
    }

    #[test]
    fn local_only_addresses() {
        for url in [
            "http://localhost:7860",
            "http://127.0.0.1:7860",
            "http://0.0.0.0:7860",
            "http://[::]:7860",
            "http://[::1]:7860",
        ] {
            assert!(is_local_only(url), "{url}");
        }
        for url in ["http://10.0.0.5:7860", "https://trackio.example"] {
            assert!(!is_local_only(url), "{url}");
        }
    }

    async fn stub(write_check: axum::Router) -> String {
        use axum::routing::get;
        let app = write_check.route(
            "/version",
            get(|| async { axum::Json(json!({ "version": "0.42.0" })) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    #[tokio::test]
    async fn preflight_reports_the_write_check() {
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::post;
        let allowed = |headers: HeaderMap| async move {
            let ok = headers
                .get("x-trackio-write-token")
                .is_some_and(|t| t == "right");
            axum::Json(json!({ "data": { "allowed": ok } }))
        };
        let base =
            stub(axum::Router::new().route("/api/get_run_mutation_status", post(allowed))).await;
        let accepted = preflight(&base, Some("right")).await;
        assert_eq!(
            (accepted.reachable, accepted.write_access),
            (true, Some(true))
        );
        assert_eq!(accepted.version.as_deref(), Some("0.42.0"));
        assert_eq!(
            preflight(&base, Some("wrong")).await.write_access,
            Some(false)
        );
        let unchecked = preflight(&base, None).await;
        assert_eq!((unchecked.write_access, unchecked.error), (None, None));

        let silent = || async { axum::Json(json!({})) };
        let base =
            stub(axum::Router::new().route("/api/get_run_mutation_status", post(silent))).await;
        let silent = preflight(&base, Some("right")).await;
        assert!(silent.error.is_some_and(|e| e.contains("allowed")));

        let failing = || async { StatusCode::INTERNAL_SERVER_ERROR };
        let base =
            stub(axum::Router::new().route("/api/get_run_mutation_status", post(failing))).await;
        let failed = preflight(&base, Some("right")).await;
        assert!(failed.reachable);
        assert_eq!(failed.write_access, None);
        assert!(failed.error.is_some_and(|e| e.contains("500")));
    }
}
