//! Operates on a project registered in the local orx store.

use crate::error::{anyhow, Result};
use crate::local::projects::expand_path;
use crate::local::share::{self, ChatSelection, ShareOptions};
use crate::plane::{resolve_project, ProjectEdit};
use crate::ProjectCommand;

pub async fn run(args: crate::ProjectArgs) -> Result<()> {
    match args.command {
        ProjectCommand::View { project_id, all } => {
            let store = crate::store::Store::open()?;
            resolve_project(store, &project_id)?.view_project(all).await
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
        ProjectCommand::Share {
            project_id,
            out,
            chats,
            no_chats,
            no_logs,
            no_artifacts,
        } => {
            let store = crate::store::Store::open()?;
            let project = crate::local::resolve::resolve_project(&store, &project_id)?;
            let options = ShareOptions {
                chats: if no_chats {
                    ChatSelection::None
                } else if chats.is_empty() {
                    ChatSelection::All
                } else {
                    ChatSelection::Only(chats)
                },
                logs: !no_logs,
                artifacts: !no_artifacts,
            };
            let out = expand_path(&out.unwrap_or_else(|| share::file_name(&project)))?;
            let manifest = share::export(&store, &project, &options, &out)?;
            println!(
                "Saved {} to {}: {} experiments, {} runs, {} chats.",
                project.name,
                out.display(),
                manifest.experiments.len(),
                manifest.runs.len(),
                manifest.chats.len()
            );
            for skipped in &manifest.skipped {
                println!(
                    "Left out (over {}): {skipped}",
                    crate::store::human_bytes(share::MAX_FILE_BYTES)
                );
            }
            Ok(())
        }
        ProjectCommand::Import { file, path } => {
            let file = expand_path(&file)?;
            let folder = match path {
                Some(path) => path,
                None => file
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .ok_or_else(|| anyhow!("{} is not a file.", file.display()))?,
            };
            let store = crate::store::Store::open()?;
            let imported = share::import(&store, &file, &expand_path(&folder)?)?;
            println!(
                "Opened {} ({}) at {}: {} experiments, {} runs, {} chats.",
                imported.project.name,
                imported.project.id,
                imported.project.repo_path,
                imported.manifest.experiments.len(),
                imported.manifest.runs.len(),
                imported.manifest.chats.len()
            );
            for branch in &imported.missing_branches {
                println!("Not in the file, so its experiment has no code: {branch}");
            }
            Ok(())
        }
    }
}
