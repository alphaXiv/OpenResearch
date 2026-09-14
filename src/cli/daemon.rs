use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Port to bind on 127.0.0.1 (default 4790 — what the api proxies to).
    #[arg(long)]
    pub port: Option<u16>,
    /// Require `Authorization: Bearer <token>` on every request. Falls back
    /// to the `ORX_SERVE_TOKEN` env var when omitted. Unset by default for
    /// backward compatibility, but strongly recommended whenever this runs
    /// on a host other local users can reach — see SECURITY.md.
    #[arg(long)]
    pub token: Option<String>,
}

#[derive(Args, Debug)]
pub struct SuperviseArgs {
    /// The run to supervise (must exist in the local store).
    pub run_id: String,
}

#[derive(Args, Debug)]
pub struct UpArgs {
    /// Port to bind on 127.0.0.1. With `--remote`, the local presentation port.
    #[arg(long, default_value_t = 4791)]
    pub port: u16,
    /// Run `orx up` on a remote box over SSH and forward it here. The value is
    /// an `~/.ssh/config` host alias, or `user@host` (append `:PORT` for a
    /// non-standard SSH port, e.g. `root@1.2.3.4:38455`). Only user@host + port
    /// are reconstructed; a custom key or jump host must come from `~/.ssh/config`.
    /// Starts an authenticated server there, tunnels it through a hidden local
    /// port, and opens a dedicated local presentation gateway in your browser.
    #[arg(long, value_name = "HOST")]
    pub remote: Option<String>,
    /// Don't open the dashboard in the browser on startup.
    #[arg(long)]
    pub no_browser: bool,
    /// Don't spawn the opencode agent on startup (for tests).
    #[arg(long)]
    pub no_agent: bool,
    /// opencode model override, e.g. `anthropic/claude-sonnet-4-5`.
    #[arg(long)]
    pub model: Option<String>,
    /// Internal persistent dashboard/agent-host mode.
    #[arg(long, hide = true)]
    pub remote_host: bool,
}

#[derive(Args, Clone, Debug)]
pub struct RemoteHostArgs {
    #[command(subcommand)]
    pub command: RemoteHostCommand,
}

#[derive(Clone, Subcommand, Debug)]
pub enum RemoteHostCommand {
    Ensure {
        #[arg(long)]
        expected_instance: Option<String>,
    },
    Status,
    Attach {
        #[arg(long)]
        expected_instance: String,
    },
    Stop,
}
