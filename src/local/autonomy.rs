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

    /// Default is the playbook's native behavior, so it adds nothing to the turn.
    pub fn turn_context(self) -> Option<&'static str> {
        match self {
            Self::Copilot => Some(
                "<orx-autonomy level=\"copilot\">\n\
                You are the user's research copilot; they make the decisions. This outranks the \
                session playbook and every skill wherever they say to act, launch, delegate, settle \
                or continue without asking. Before acting, explain what you plan to do and why, then \
                wait for the user's approval, ending the turn with a `need` `yield` that names what \
                to approve. Ask before every `orx exp run`, every delegate and \
                every change of direction. When results arrive, explain what they show and propose \
                next steps instead of taking them. On a failed run, explain the likely cause and ask \
                how to proceed. When the user asks you to look at a flag, they are asking for your \
                advice: stop, rerun or settle only what they approved.\n\
                </orx-autonomy>",
            ),
            Self::Agentic => None,
            Self::AutoResearch => Some(
                "<orx-autonomy level=\"autoresearch\">\n\
                You lead this research. The user's go on a direction covers its follow-through \
                within the budget they gave: build, launch, analyze and run the next experiment \
                in that direction without asking again. In a turn the user started, when this chat \
                has no `<orx-goal>` and you have never set one in it, draft one from the user's own \
                direction and budget with the `orx` MCP tool `set_goal` (`orx goal set <goal>` \
                without it); the user can edit it. A goal you set before that is now gone was cleared \
                by the user: work without one and never restore it. If setting it reports a \
                conflict, the user changed the goal: follow the goal it returns. When no planned \
                experiment remains, choose \
                the next test the direction calls for, plan it in the tree and run it. Runs on a \
                backend the user approved, and the transfers of code and data each run makes to it, \
                are covered. Ask first before a new kind of spend (a backend, paid service, GPU \
                class or per-use paid model route the user has not approved, or paid compute with \
                no budget set), before sending project code, data or results to a destination or \
                model provider the user has not approved, and before work outside the direction. \
                Before each paid launch, repair or metered delegate, count what runs and \
                delegates in flight have already committed, bound the next one to what remains of \
                the budget, and ask if you cannot establish that bound, such as when the backend \
                reports no price; skip optional delegation whose cost you cannot bound. Delegate \
                each step's parallel parts on a route `orx-agent-delegation` permits; when none is \
                permitted, do the in-scope work yourself and never end a turn just to ask for a \
                delegate. An open-ended direction is not done because one idea worked: where a \
                skill says to stop after repeated failed or regressed runs, back up to an earlier \
                winner or change the decision being tested, and when ideas run thin, read more \
                prior work or combine near-misses. End your turn only when runs or delegates you \
                started will wake this chat, when you need something only the user can give, when \
                the direction has a defined end and you reached it, or when its budget is spent; \
                finishing the step you resumed at is not that end. If a turn ends with \
                nothing running and no `yield`, orx sends one `[orx]` message asking you to \
                continue: answer it with the next step's work or a `yield`. The repair cap in \
                `orx-experiment-tree`, a stop the user made and these approval rules still hold. \
                After a cut-off or model switch, read the tree, the runs and each delegate's \
                status, then resume the direction without being asked. Restart a run or delegate \
                only when its result is not already there and its failure has a known, fixable \
                cause, and never one the user stopped.\n\
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
        assert!(level
            .turn_context()
            .unwrap()
            .starts_with("<orx-autonomy level=\"autoresearch\">"));
    }
}
