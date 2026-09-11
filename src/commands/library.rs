use crate::error::{anyhow, Result};
use crate::local::{latex_templates, user_skills};
use crate::{LibraryArgs, LibraryCommand};

pub fn skills(args: LibraryArgs) -> Result<()> {
    let LibraryCommand::Add { path } = args.command;
    let bytes = std::fs::read(&path)?;
    let previous = user_skills::list_uploaded();
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("provide a SKILL.md file or a skill ZIP"))?;
    let saved = user_skills::save_upload(filename, &bytes)?;
    println!(
        "{}",
        serde_json::json!({
            "name": saved.name,
            "scope": "all projects in the active OpenResearch data directory",
            "replaced": previous.iter().any(|s| s.name == saved.name),
        })
    );
    Ok(())
}

pub fn templates(args: LibraryArgs) -> Result<()> {
    let LibraryCommand::Add { path } = args.command;
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("provide a .tex file or a template ZIP"))?;
    let bytes = std::fs::read(&path)?;
    let previous = latex_templates::list();
    let saved = latex_templates::save_upload(filename, &bytes)?;
    println!(
        "{}",
        serde_json::json!({
            "name": saved.name,
            "scope": "all projects in the active OpenResearch data directory",
            "replaced": previous.iter().any(|t| t.name == saved.name),
        })
    );
    Ok(())
}
