//! Operates on a project registered in the local orx store.

use crate::error::Result;
use crate::plane::{resolve_project, ProjectEdit};
use crate::ProjectCommand;

pub async fn run(args: crate::ProjectArgs) -> Result<()> {
    match args.command {
        ProjectCommand::View { project_id } => {
            let store = crate::store::Store::open()?;
            resolve_project(store, &project_id)?.view_project().await
        }
        ProjectCommand::Edit {
            project_id,
            name,
            run_command,
        } => {
            let store = crate::store::Store::open()?;
            resolve_project(store, &project_id)?
                .edit_project(ProjectEdit { name, run_command })
                .await
        }
    }
}
