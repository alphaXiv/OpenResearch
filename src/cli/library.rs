use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct SkillArgs {
    pub path: Option<String>,
}

#[derive(Args, Debug)]
pub struct LibraryArgs {
    #[command(subcommand)]
    pub command: LibraryCommand,
}

#[derive(Subcommand, Debug)]
pub enum LibraryCommand {
    /// Save a file or ZIP across projects; replaces an existing entry of the same name.
    Add { path: std::path::PathBuf },
}

#[derive(Args, Debug)]
pub struct InstallSkillsArgs {
    /// Which agent(s) to install into: `claude`, `codex`, `opencode`, `cursor`,
    /// or `all`. Defaults to every agent already set up on this machine.
    #[arg(long)]
    pub agent: Option<String>,

    /// Also install the full set of modular `orx` skills (~8 always-listed
    /// skills) into the agent's global skills dir, not just the thin shim.
    /// Intended for dedicated/orx-only environments. In a general-purpose setup
    /// the always-on skills add noise, so the default is the shim alone.
    #[arg(long)]
    pub full: bool,
}
