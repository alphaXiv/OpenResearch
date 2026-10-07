//! The only place reqwest clients are built; `clippy.toml` bans building them elsewhere.

/// For servers on this machine. reqwest proxies 127.0.0.1 through `HTTP(S)_PROXY` too (OR-352).
#[allow(clippy::disallowed_methods)]
pub fn loopback_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder().no_proxy()
}

/// For internet hosts; honors `HTTP(S)_PROXY` and `NO_PROXY`.
#[allow(clippy::disallowed_methods)]
pub fn remote_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
}
