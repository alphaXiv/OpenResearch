//! How much of the research an agent owns before checking in with the user.
//! Unlike permission modes this is harness-independent: it rides every turn as
//! an `<orx-autonomy>` block, so changing it never restarts the agent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    Copilot,
    #[default]
    Agentic,
}

impl Autonomy {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "copilot" => Some(Self::Copilot),
            "agentic" => Some(Self::Agentic),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Copilot => "copilot",
            Self::Agentic => "agentic",
        }
    }

    /// Stored values from a newer build fall back to the default.
    pub fn from_stored(id: Option<&str>) -> Self {
        id.and_then(Self::from_id).unwrap_or_default()
    }

    /// Agentic is the playbook's native behavior, so it adds nothing to the turn.
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
        assert_eq!(Autonomy::from_stored(Some("copilot")), Autonomy::Copilot);
        assert!(Autonomy::Agentic.turn_context().is_none());
    }
}
