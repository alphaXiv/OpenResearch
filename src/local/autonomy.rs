//! How much of the research an agent owns before checking in with the user.
//! Unlike permission modes this is harness-independent and can add an
//! `<orx-autonomy>` block to the turn without restarting the agent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    Copilot,
    #[default]
    Agentic,
    AutoResearch,
}

impl Autonomy {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "copilot" => Some(Self::Copilot),
            "agentic" => Some(Self::Agentic),
            "autoresearch" => Some(Self::AutoResearch),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Copilot => "copilot",
            Self::Agentic => "agentic",
            Self::AutoResearch => "autoresearch",
        }
    }

    /// Stored values from a newer build fall back to the default.
    pub fn from_stored(id: Option<&str>) -> Self {
        id.and_then(Self::from_id).unwrap_or_default()
    }

    pub fn is_autonomous(self) -> bool {
        matches!(self, Self::Agentic | Self::AutoResearch)
    }

    /// Default is the playbook's native behavior, so it adds nothing to the turn.
    pub fn turn_context(self) -> Option<&'static str> {
        match self {
            Self::Copilot => Some(
                "<orx-autonomy level=\"copilot\">\n\
                You are the user's research copilot; they make the decisions. Before acting, \
                explain what you plan to do and why, then wait for their approval. Ask before \
                every `orx exp run` and before changing direction. When results arrive, explain \
                what they show and propose next steps instead of taking them. On a failed run, \
                explain the likely cause and ask how to proceed. This overrides any skill \
                guidance to continue on your own.\n\
                </orx-autonomy>",
            ),
            Self::Agentic => None,
            Self::AutoResearch => Some(
                "<orx-autonomy level=\"autoresearch\">\n\
                You lead this research. The user's go on a direction covers its follow-through \
                within the budget they gave: build, launch, analyze, and start the next planned \
                experiment without asking again. Ask before a new kind of spend (a backend, paid \
                service or GPU class the user has not approved, or paid compute with no budget \
                set), before sending project code, data or results off this machine, or before work \
                outside the approved direction. Keep delegates on each step's parallel parts. An \
                open-ended direction is not done because one idea worked: when ideas run thin, read \
                more prior work, combine near-misses or try a bolder change. Stop only when runs or \
                delegates you started will wake this chat, when you need something only the user can \
                give, or when the budget is spent. \
                After a cut-off or model switch, read the tree, the runs and each delegate's status, \
                then resume the approved direction without being asked. This overrides any skill \
                guidance to stop for confirmation.\n\
                </orx-autonomy>",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_levels_default_to_agentic_which_adds_no_context() {
        assert_eq!(Autonomy::from_stored(None), Autonomy::Agentic);
        assert_eq!(Autonomy::from_stored(Some("reckless")), Autonomy::Agentic);
        assert_eq!(Autonomy::from_stored(Some("agentic")), Autonomy::Agentic);
        assert_eq!(Autonomy::from_stored(Some("copilot")), Autonomy::Copilot);
        assert!(Autonomy::Agentic.turn_context().is_none());
    }

    #[test]
    fn auto_research_uses_its_stored_and_wire_id_and_adds_turn_context() {
        let level = Autonomy::AutoResearch;
        let id = "autoresearch";
        assert_eq!(level.id(), id);
        assert_eq!(Autonomy::from_stored(Some(id)), level);
        assert_eq!(serde_json::to_value(level).unwrap(), id);
        assert_eq!(
            serde_json::from_value::<Autonomy>(id.into()).unwrap(),
            level
        );
        assert!(level.is_autonomous());
        assert!(level
            .turn_context()
            .unwrap()
            .starts_with("<orx-autonomy level=\"autoresearch\">"));
    }
}
