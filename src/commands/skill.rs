use crate::error::{anyhow, Result};
use crate::local::agent_skills::{self, SkillSet};

// Bundled top-level overview, embedded from the repository root.
const SKILL_MD: &str = include_str!("../../SKILL.md");

/// Which bundled skill set this invocation serves.
fn current_skill_set() -> SkillSet {
    if crate::local::chat::in_local_session() {
        SkillSet::Local
    } else {
        SkillSet::Full
    }
}

pub async fn run(args: crate::SkillArgs) -> Result<()> {
    if let Some(path) = args.path {
        if let Some(skill) = agent_skills::find(&path, current_skill_set()) {
            println!("{}", skill.content.trim_end());
            return Ok(());
        }
        if let Some((_, resource)) = agent_skills::find_resource(&path, current_skill_set()) {
            println!("{}", resource.content.trim_end());
            return Ok(());
        }
        let available = agent_skills::skills(current_skill_set())
            .iter()
            .map(|skill| skill.name)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(anyhow!(
            "Unknown bundled skill or resource {path:?}. Available skills: {available}"
        ));
    }

    println!("{}", SKILL_MD);

    println!("\nBundled modules (orx skill <name>):");
    for s in agent_skills::skills(current_skill_set()) {
        println!("  {:<20} {}", s.name, s.description);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_overview_avoids_openresearch_ui_navigation() {
        crate::local::assert_agent_guidance_is_ui_agnostic("orx skill overview", super::SKILL_MD);
    }
}
