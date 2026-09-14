use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct VersionArgs {
    /// Print the embedded telemetry build channel.
    #[arg(long, hide = true, conflicts_with_all = ["check", "json"])]
    pub build_channel: bool,
    /// Also check the latest released version on GitHub.
    #[arg(long)]
    pub check: bool,
    /// Emit a JSON object instead of text (implies --check).
    #[arg(long)]
    pub json: bool,
    /// Print the dashboard protocol understood by this binary.
    #[arg(long, hide = true, conflicts_with_all = ["check", "json", "build_channel"])]
    pub dashboard_protocol: bool,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Report whether an update is available without installing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Update even when the binary doesn't match the install receipt
    /// (multiple copies, or a `cargo install` overwrote it).
    #[arg(long)]
    pub force: bool,
    /// Internal: the detached auto-updater. Silent, and records its outcome so
    /// repeated failures back off.
    #[arg(long, hide = true)]
    pub background: bool,
}

#[derive(Args, Debug)]
pub struct InstallCliArgs {
    /// Replace an existing `orx` on your PATH.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct DeleteArgs {
    #[command(subcommand)]
    pub command: DeleteCommand,
}

#[derive(Subcommand, Debug, Clone, Copy)]
pub enum DeleteCommand {
    /// Delete only orx.db and its SQLite sidecars. Project folders are untouched.
    #[command(alias = "db")]
    Database,
    /// Delete the running orx executable and its matching installer receipt.
    Cli,
    /// Delete both the database and CLI executable.
    All,
}

#[derive(Args, Debug)]
pub struct TelemetryArgs {
    #[command(subcommand)]
    pub command: TelemetryCommand,
}

#[derive(Subcommand, Debug)]
pub enum TelemetryCommand {
    /// Show whether analytics is on, why, and the anonymous install id.
    Status,
    /// Enable anonymous usage analytics.
    On,
    /// Disable anonymous usage analytics on this machine.
    Off,
}
