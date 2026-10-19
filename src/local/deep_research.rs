//! Fork-local package registration; the workflow lives entirely in its skill files.
use super::agent_skills::{AgentSkill, AgentSkillResource};

pub const CONTENT: &str = include_str!("../../agent-skills/orx-deep-research/SKILL.md");
pub const SKILL: AgentSkill = AgentSkill {
    name: "orx-deep-research",
    description: "Coordinate multi-source research and a substantial cited report with independent researchers, cross-harness review, and a report writer. Use for deep research, broad comparisons, or literature synthesis requiring a durable report; use orx-lit-review for focused scientific explanations and quick lookups.",
    content: CONTENT,
    resources: &[
        AgentSkillResource {
            path: "references/researcher.md",
            content: include_str!("../../agent-skills/orx-deep-research/references/researcher.md"),
        },
        AgentSkillResource {
            path: "references/report-writer.md",
            content: include_str!("../../agent-skills/orx-deep-research/references/report-writer.md"),
        },
    ],
};
