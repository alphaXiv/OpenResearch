//! Harness-owned turn options — permission choices, plan activation, and the
//! reasoning level a chat session runs under. Each harness advertises its native
//! user-facing vocabulary and maps the chosen value onto its own CLI/API.
//!
//! The two axes are modeled differently on purpose:
//!
//! * Permission policies share an internal enum, but the stored/UI ids, labels,
//!   and descriptions belong to each provider. Plan is part of this enum only
//!   for Claude; Codex and OpenCode carry it on the independent session axis.
//! * Reasoning level is deliberately NOT shared, and is now per *model* as well
//!   as per harness (issue #123). Claude's `--effort` tiers, Codex's
//!   `model_reasoning_effort` and OpenCode's `variant` genuinely differ, and
//!   within a harness they differ by model too: Codex's `ultra` is Sol/Terra
//!   only, OpenCode's variants are declared per model in its catalog, and
//!   Claude's `ultracode` depends on the installed CLI version. So the real
//!   choices ride on [`ModelInfo::reasoning_levels`](super::ModelInfo), and the
//!   list here is only the harness-wide fallback for a model with none.
//!
//! Every reasoning list leads with [`REASONING_DEFAULT_ID`] and defaults to it,
//! so the composer sends no override unless the user picks one — selecting a
//! model must never silently replace the CLI's own configured effort.
//!
//! A harness that doesn't support an axis lists nothing for it, and the composer
//! hides that control.

use serde::{Deserialize, Serialize};

/// Internal permission semantics shared by the adapters. Stored wire ids are
/// provider-owned; [`PermissionMode::from_id`] accepts each provider's native
/// spelling and resolves it to one of these policies.
///
/// Not every harness supports every policy — validation checks the provider's
/// advertised choices before this mapper is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Prompt for every action. (`ask`)
    Ask,
    /// Auto-accept file edits; still prompt for other tools. (`accept-edits`)
    AcceptEdits,
    /// Read/plan only — propose without executing. (`plan`)
    Plan,
    /// Claude Code's default balanced auto mode. (`auto`)
    Auto,
    /// No prompts at all. (`bypass`)
    Bypass,
}

impl PermissionMode {
    /// Parse a provider-owned wire id back to its internal policy. Validation
    /// against the selected harness happens separately, so aliases cannot make
    /// a Claude value valid for a Codex session.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "ask" | "manual" | "default" => Some(PermissionMode::Ask),
            "accept-edits" | "acceptEdits" => Some(PermissionMode::AcceptEdits),
            "plan" => Some(PermissionMode::Plan),
            "auto" | "approve-for-me" | "auto-approve" => Some(PermissionMode::Auto),
            "bypass" | "bypassPermissions" | "full-access" => Some(PermissionMode::Bypass),
            _ => None,
        }
    }
}

/// One selectable value in a composer toggle (id + human label). Ids are owned
/// because the reasoning axis is now *model*-derived: OpenCode's choices come
/// from `opencode models --verbose` at detect time, so they can't be
/// `&'static str`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionChoice {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl OptionChoice {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    pub fn described(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: Some(description.into()),
        }
    }
}

/// How a harness exposes planning to the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanActivation {
    /// Claude Code owns Plan as one of its permission modes.
    Permission,
    /// Codex/OpenCode enter Plan through the `/plan` command.
    Command,
}

/// The wire id meaning "send no explicit effort/variant — let the harness CLI
/// and its own config decide". This is a *sentinel*, never passed to a CLI: the
/// per-harness mappers (`claude_effort`, `codex_reasoning`, `opencode_variant`)
/// all resolve it to `None`.
///
/// It exists because "no override" and "the harness's suggested level" are
/// genuinely different states. Before this, the WebUI always sent an explicit
/// per-turn effort, which silently overrode a user's configured
/// `model_reasoning_effort = "max"` in `~/.codex/config.toml` (issue #123).
pub const REASONING_DEFAULT_ID: &str = "default";

/// Human label for a raw effort/variant id. Native ids are lowercase words
/// (`low`, `xhigh`, `ultracode`), so title-casing covers all of them bar
/// `xhigh`, whose display casing is `XHigh`.
fn reasoning_label(id: &str) -> String {
    match id {
        "xhigh" => "XHigh".to_string(),
        // Unknown native ids still render — an id the CLI genuinely accepts is
        // better shown title-cased than dropped.
        other => super::detect::title_case(other),
    }
}

/// Native ids → labeled choices, no sentinel. For models whose *actual*
/// default tier is known (codex reports `defaultReasoningEffort`), the picker
/// preselects that concrete tier instead of offering a "no override" row —
/// sending the tier the CLI would resolve anyway is equivalent, and the user
/// sees a real value.
pub fn reasoning_tiers(ids: &[&str]) -> Vec<OptionChoice> {
    ids.iter()
        // Skip a native id that collides with the sentinel: it would render
        // as a second row that selects "no override" instead of the tier.
        // The ids come from the CLIs' catalogs verbatim, so this is the
        // catalog's call, not ours.
        .filter(|id| **id != REASONING_DEFAULT_ID)
        .map(|id| OptionChoice::new(*id, reasoning_label(id)))
        .collect()
}

/// Build a reasoning list from native ids, led by the `Default` choice — which
/// sends no override at all. For harnesses where "no override" is genuinely
/// different from every listed tier: Claude's unset effort means *adaptive*
/// (not any fixed level), and opencode reports no per-model default to
/// preselect.
pub fn reasoning_choices(ids: &[&str]) -> Vec<OptionChoice> {
    std::iter::once(OptionChoice::new(REASONING_DEFAULT_ID, "Default"))
        .chain(reasoning_tiers(ids))
        .collect()
}

/// A stored reasoning id → the value to actually send, or `None` for "no
/// override". Shared by all three harnesses so the sentinel rule lives in one
/// place; each passes the `allowed` set it computes for the selected model.
///
/// `None` covers three cases that must all leave the CLI's own configured
/// effort alone: the `default` sentinel, a stale stored level the selected
/// model doesn't accept, and junk.
pub fn resolve_reasoning<'a>(level: Option<&'a str>, allowed: &[&str]) -> Option<&'a str> {
    let level = level?;
    (level != REASONING_DEFAULT_ID && allowed.contains(&level)).then_some(level)
}

/// The toggle vocabulary a harness supports, sent to the UI so it can render
/// only valid choices and pre-select the harness's defaults. An empty list for
/// an axis means "this harness has no such control" and the UI hides it.
///
/// The reasoning axis here is the harness-wide *fallback*: the choices shown
/// when the selected model has no per-model list of its own. Model-specific
/// choices ride on [`ModelInfo::reasoning_levels`](super::ModelInfo) and take
/// precedence in the composer — see `reasoningFor` in `ui/src/api.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessOptions {
    pub permission_modes: Vec<OptionChoice>,
    pub default_permission_mode: Option<&'static str>,
    pub plan_activation: Option<PlanActivation>,
    pub reasoning_levels: Vec<OptionChoice>,
    pub default_reasoning_level: Option<String>,
}

impl HarnessOptions {
    /// A harness with neither control (the trait default).
    pub fn none() -> Self {
        Self {
            permission_modes: Vec::new(),
            default_permission_mode: None,
            plan_activation: None,
            reasoning_levels: Vec::new(),
            default_reasoning_level: None,
        }
    }

    pub fn with_permission_choices(
        mut self,
        modes: Vec<OptionChoice>,
        default: &'static str,
        plan_activation: PlanActivation,
    ) -> Self {
        self.permission_modes = modes;
        self.default_permission_mode = Some(default);
        self.plan_activation = Some(plan_activation);
        self
    }

    /// Set the harness-wide fallback reasoning list from native ids. Unlike
    /// permission modes, reasoning vocabulary isn't shared — Claude's `--effort`
    /// tiers, Codex's `model_reasoning_effort` and OpenCode's `variant` genuinely
    /// differ — so each harness passes its own ids and interprets the chosen one
    /// in its `run_turn`.
    ///
    /// The list is always led by the `Default` choice, and `Default` is always
    /// the default selection: the composer must not send an explicit override
    /// unless the user picks one (issue #123).
    pub fn with_reasoning_levels(mut self, ids: &[&str]) -> Self {
        self.reasoning_levels = reasoning_choices(ids);
        self.default_reasoning_level = Some(REASONING_DEFAULT_ID.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_id_accepts_provider_owned_spellings() {
        assert_eq!(PermissionMode::from_id("manual"), Some(PermissionMode::Ask));
        assert_eq!(
            PermissionMode::from_id("acceptEdits"),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            PermissionMode::from_id("approve-for-me"),
            Some(PermissionMode::Auto)
        );
        assert_eq!(
            PermissionMode::from_id("full-access"),
            Some(PermissionMode::Bypass)
        );
        assert_eq!(
            PermissionMode::from_id("default"),
            Some(PermissionMode::Ask)
        );
        assert_eq!(
            PermissionMode::from_id("auto-approve"),
            Some(PermissionMode::Auto)
        );
        assert_eq!(PermissionMode::from_id("nonsense"), None);
    }
}
