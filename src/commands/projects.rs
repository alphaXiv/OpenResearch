//! Lists projects registered by `orx up` in the local store.

use crate::error::Result;
use crate::store::Store;

pub async fn run(args: crate::ProjectsArgs) -> Result<()> {
    let projects = Store::open()?.list_local_projects()?;

    if args.json {
        let rows = projects
            .iter()
            .map(|project| {
                serde_json::json!({
                    "id": project.id,
                    "name": project.name,
                    "paperId": project.paper_id,
                    "path": project.repo_path,
                    "baselineBranch": project.baseline_branch,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if projects.is_empty() {
        println!("No local projects found. Run `orx up` in a repository to add one.");
        return Ok(());
    }

    let id_width = projects
        .iter()
        .map(|project| project.id.chars().count())
        .max()
        .unwrap_or(0);
    for project in projects {
        let pad = id_width.saturating_sub(project.id.chars().count());
        println!(
            "{}{}  {}  {} · baseline {}",
            project.id,
            " ".repeat(pad),
            project.name,
            project.repo_path,
            project.baseline_branch,
        );
    }

    Ok(())
}
