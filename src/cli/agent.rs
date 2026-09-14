use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct AgentArgs {
    #[command(subcommand)]
    pub command: AgentCommand,
}

#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    /// Hand a task to a helper agent running in its own top-level session.
    Spawn {
        /// What the helper agent should do. Write it as a self-contained brief:
        /// the helper starts with an empty transcript and cannot see this chat.
        task: Option<String>,
        /// Read the task from stdin instead, for long multi-paragraph briefs.
        #[arg(long)]
        stdin: bool,
        /// Name the session in the sidebar. Defaults to an auto-generated title.
        #[arg(long)]
        title: Option<String>,
        /// Harness for the helper (defaults to this session's).
        #[arg(long)]
        harness: Option<String>,
        /// Model for the helper (defaults to this session's).
        #[arg(long)]
        model: Option<String>,
        /// Do not resume this chat when the helper finishes.
        #[arg(long)]
        no_wake: bool,
    },
}
