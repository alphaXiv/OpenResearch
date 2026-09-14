use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct ProjectsArgs {
    /// Emit local project records as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct OrgsArgs {
    /// Emit organization records as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    /// Show a local project's details and experiment tree.
    View { project_id: String },

    /// Edit a local project's name or run command.
    Edit {
        project_id: String,
        /// Rename the project.
        #[arg(long)]
        name: Option<String>,
        /// Set the project's default run command.
        /// New experiments inherit it; pass '' to clear.
        #[arg(long = "run-command")]
        run_command: Option<String>,
    },
}
