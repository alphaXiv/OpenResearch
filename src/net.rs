//! The only place reqwest clients are built; `clippy.toml` bans building them elsewhere.

use std::sync::OnceLock;

use crate::error::{anyhow, Result};

struct NetworkProxy {
    url: String,
    proxy: reqwest::Proxy,
    bypass: String,
}

// Clients and long-lived agents retain their network configuration until restart.
static NETWORK_PROXY: OnceLock<Option<NetworkProxy>> = OnceLock::new();

pub(crate) fn normalize_proxy_url(value: Option<String>) -> Result<Option<String>> {
    let Some(value) = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let url = url::Url::parse(&value)
        .map_err(|_| anyhow!("Enter a full proxy address, such as http://127.0.0.1:18080."))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.port_or_known_default() == Some(0)
    {
        return Err(anyhow!(
            "Use an HTTP or HTTPS proxy address with a host and a valid port."
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(anyhow!(
            "Use a proxy address without a username or password."
        ));
    }
    if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
        return Err(anyhow!(
            "Use a proxy address without a path, query or fragment."
        ));
    }
    Ok(Some(url.to_string()))
}

pub(crate) fn initialize_proxy() -> Result<()> {
    let url = normalize_proxy_url(
        crate::telemetry::load_settings().and_then(|settings| settings.network_proxy_url),
    )?;
    let configured = url
        .map(|url| {
            let bypass = [
                "localhost,127.0.0.1,::1",
                &std::env::var("NO_PROXY")
                    .or_else(|_| std::env::var("no_proxy"))
                    .unwrap_or_default(),
            ]
            .join(",");
            let proxy = reqwest::Proxy::all(&url)?.no_proxy(reqwest::NoProxy::from_string(&bypass));
            Ok::<_, crate::error::Error>(NetworkProxy { url, proxy, bypass })
        })
        .transpose()?;
    NETWORK_PROXY
        .set(configured)
        .map_err(|_| anyhow!("Network proxy was already initialized."))
}

pub(crate) fn active_proxy_url() -> Option<&'static str> {
    NETWORK_PROXY
        .get()
        .and_then(|proxy| proxy.as_ref())
        .map(|proxy| proxy.url.as_str())
}

pub(crate) fn apply_proxy(command: &mut std::process::Command) {
    if let Some(Some(proxy)) = NETWORK_PROXY.get() {
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, &proxy.url);
        }
        for key in ["NO_PROXY", "no_proxy"] {
            command.env(key, &proxy.bypass);
        }
    }
}

/// For servers on this machine. reqwest proxies 127.0.0.1 through `HTTP(S)_PROXY` too (OR-352).
#[allow(clippy::disallowed_methods)]
pub fn loopback_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder().no_proxy()
}

/// For internet hosts; the saved proxy overrides inherited proxy variables.
#[allow(clippy::disallowed_methods)]
pub fn remote_client() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    match NETWORK_PROXY.get().and_then(|proxy| proxy.as_ref()) {
        Some(proxy) => builder.proxy(proxy.proxy.clone()),
        None => builder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_addresses_are_validated_without_exposing_credentials() {
        assert_eq!(normalize_proxy_url(None).unwrap(), None);
        assert_eq!(normalize_proxy_url(Some("  ".into())).unwrap(), None);
        assert_eq!(
            normalize_proxy_url(Some(" http://127.0.0.1:18080 ".into()))
                .unwrap()
                .as_deref(),
            Some("http://127.0.0.1:18080/")
        );
        for value in [
            "127.0.0.1:18080",
            "socks5://localhost:1080",
            "http://localhost:0",
            "http://localhost:99999",
            "http://localhost/proxy",
            "https://localhost?token=secret",
            "http://user:secret@localhost:8080",
            "file:///tmp/proxy",
        ] {
            let error = normalize_proxy_url(Some(value.into()))
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret"));
        }
    }

    #[tokio::test]
    async fn saved_proxy_routes_http_preserves_loopback_and_reaches_children() {
        if !crate::config::in_test_child(&[
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ]) {
            return;
        }
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("http://{address}/");
        crate::telemetry::mutate_settings(|settings| {
            settings.network_proxy_url = Some(url.clone());
            settings.auto_update = Some(false);
        })
        .unwrap();
        initialize_proxy().unwrap();

        let server = tokio::spawn(async move {
            for expected in ["http://proxy-test.invalid/paper", "/local"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let count = stream.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..count])
                    .starts_with(&format!("GET {expected} HTTP/1.1")));
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
            }
        });
        let response = remote_client()
            .build()
            .unwrap()
            .get("http://proxy-test.invalid/paper")
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
        let response = remote_client()
            .build()
            .unwrap()
            .get(format!("http://{address}/local"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
        server.await.unwrap();

        let child = crate::process::command("unused-proxy-test");
        let variables: std::collections::HashMap<_, _> = child.get_envs().collect();
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
            assert_eq!(
                variables.get(std::ffi::OsStr::new(key)).copied().flatten(),
                Some(std::ffi::OsStr::new(&url))
            );
        }
        assert!(variables
            .get(std::ffi::OsStr::new("NO_PROXY"))
            .copied()
            .flatten()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("127.0.0.1"));

        crate::telemetry::mutate_settings(|settings| settings.network_proxy_url = None).unwrap();
        assert_eq!(active_proxy_url(), Some(url.as_str()));
        assert_eq!(
            crate::telemetry::load_settings().unwrap().auto_update,
            Some(false)
        );
    }
}
