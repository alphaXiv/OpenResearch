//! Operates on a project registered in the local orx store.

use crate::client;
use crate::config::Credentials;
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
            publish,
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
            if publish {
                let creds = signed_in().await?;
                let scratch = crate::local::git::TemporaryDirectory::new("orx-publish")?;
                let out = scratch.path().join("project.orx");
                let manifest = share::export(&store, &project, &options, &out)?;
                let link = client::publish_project(&creds, &project.id, &manifest, &out).await?;
                println!(
                    "Published {}: {} ({} experiments, {} runs, {} chats). Anyone with the link can open it.",
                    project.name,
                    link.url,
                    manifest.experiments.len(),
                    manifest.runs.len(),
                    manifest.chats.len()
                );
                print_skipped(&manifest.skipped);
                return Ok(());
            }
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
            print_skipped(&manifest.skipped);
            Ok(())
        }
        ProjectCommand::Unpublish { project_id } => {
            let store = crate::store::Store::open()?;
            let project = crate::local::resolve::resolve_project(&store, &project_id)?;
            let creds = signed_in().await?;
            let link = client::find_published_project(&creds, &project.id)
                .await?
                .ok_or_else(|| anyhow!("{} has no published link.", project.name))?;
            client::delete_published_project(&creds, &link.id).await?;
            println!(
                "Unpublished {}: {} no longer works.",
                project.name, link.url
            );
            Ok(())
        }
        ProjectCommand::Import { file, path } => {
            let local = expand_path(&file)?;
            // A link, or a bare slug, that isn't also a file here.
            let slug = if local.exists() {
                None
            } else {
                client::published_project_slug(&file)
            };
            let (file, default_folder, _download) = match slug {
                Some(slug) => {
                    let api = crate::config::openresearch_api_url().await;
                    let public = client::get_public_project(&api, slug).await?;
                    let download = crate::local::git::TemporaryDirectory::new("orx-import")?;
                    let downloaded = download.path().join("download.orx");
                    client::download_public_project(&api, slug, &downloaded).await?;
                    // The publisher picks the name, so it must not steer the folder elsewhere.
                    let folder = public
                        .name
                        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-");
                    let folder = folder.trim().trim_matches(['.', '~']);
                    let folder = if folder.is_empty() { slug } else { folder };
                    (downloaded, folder.to_string(), Some(download))
                }
                None => {
                    let stem = local
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .ok_or_else(|| anyhow!("{} is not a file.", local.display()))?;
                    (local, stem, None)
                }
            };
            let folder = path.unwrap_or(default_folder);
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

async fn signed_in() -> Result<Credentials> {
    crate::config::load_credentials()
        .await?
        .ok_or_else(|| anyhow!("Not signed in to OpenResearch. Run `orx login` first."))
}

fn print_skipped(skipped: &[String]) {
    for skipped in skipped {
        println!(
            "Left out (over {}): {skipped}",
            crate::store::human_bytes(share::MAX_FILE_BYTES)
        );
    }
}
