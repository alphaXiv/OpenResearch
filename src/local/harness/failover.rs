//! Harness failover: when a turn stops on a usage, rate, or quota limit, the
//! chat continues on the next harness the user lined up instead of failing.
//!
//! The policy is opt-in and lives on the global UI state. The order is the
//! user's; an empty order falls back to registry order, so turning failover on
//! without arranging anything still tries every other chat harness.

use serde::{Deserialize, Serialize};

use super::PermissionMode;
use crate::error::{anyhow, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessFailover {
    #[serde(default)]
    pub enabled: bool,
    /// Harness ids, most preferred first.
    #[serde(default)]
    pub order: Vec<String>,
}

impl HarnessFailover {
    /// The stored form: known chat harnesses only, each once.
    pub fn validated(self) -> Result<Self> {
        let mut order = Vec::new();
        for id in self.order {
            if !super::is_chat_harness(&id) {
                return Err(anyhow!("unknown harness: {id}"));
            }
            if !order.contains(&id) {
                order.push(id);
            }
        }
        Ok(Self {
            enabled: self.enabled,
            order,
        })
    }

    pub fn from_stored(json: Option<&str>) -> Self {
        json.and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default()
    }

    /// Harnesses to try after `current`, in order, skipping any already tried.
    pub fn candidates(&self, current: &str, tried: &[String]) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let order = if self.order.is_empty() {
            super::registry()
                .iter()
                .filter(|harness| harness.supports_chat())
                .map(|harness| harness.id().to_string())
                .collect()
        } else {
            self.order.clone()
        };
        order
            .into_iter()
            .filter(|id| id != current && !tried.contains(id) && super::is_chat_harness(id))
            .collect()
    }
}

/// The permission settings a chat carries onto another harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedPermission {
    /// Wire id on the new harness.
    pub permission_id: Option<String>,
    /// Plan on the independent axis (harnesses that enter Plan by command).
    pub plan_mode: bool,
}

/// How much a permission mode lets the agent do without asking.
fn reach(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Plan => 0,
        PermissionMode::Ask => 1,
        PermissionMode::AcceptEdits => 2,
        PermissionMode::Auto => 3,
        PermissionMode::Bypass => 4,
    }
}

/// The settings on `to` that allow no more than `mode` and `plan` allowed on
/// `from`: Plan stays Plan, and otherwise the widest choice `to` offers that
/// does not go past the current one. `None` when `to` cannot run that narrowly,
/// so failing over there would quietly widen what the agent may do.
pub fn carry_permission(
    from: &str,
    mode: Option<PermissionMode>,
    plan: bool,
    to: &str,
) -> Option<CarriedPermission> {
    let current = mode.or_else(|| {
        super::effective_permission_id(from, None)
            .as_deref()
            .and_then(PermissionMode::from_id)
    });
    let was_plan = plan || current == Some(PermissionMode::Plan);
    // Under Claude's Plan the mode to return to is unknown; assume the narrow one.
    let limit = current
        .filter(|mode| *mode != PermissionMode::Plan)
        .map_or(reach(PermissionMode::Ask), reach);
    let best = super::chat_harness(to)?
        .options()
        .permission_modes
        .into_iter()
        .filter_map(|choice| Some((PermissionMode::from_id(&choice.id)?, choice.id)))
        .filter(|(mode, _)| *mode != PermissionMode::Plan && reach(*mode) <= limit)
        .max_by_key(|(mode, _)| reach(*mode))
        .map(|(_, id)| id);
    if !was_plan {
        return Some(CarriedPermission {
            permission_id: Some(best?),
            plan_mode: false,
        });
    }
    if super::supports_command_plan(to) {
        Some(CarriedPermission {
            permission_id: Some(best?),
            plan_mode: true,
        })
    } else {
        Some(CarriedPermission {
            permission_id: Some(super::permission_id_for_mode(to, PermissionMode::Plan)?),
            plan_mode: false,
        })
    }
}

/// Whether a turn failure means the harness ran out of usage, rate, or quota
/// headroom — the failures another harness can pick up. Transient overloads
/// are not included: those retry on the same harness.
pub fn is_usage_limit_failure(kind: &str, message: &str) -> bool {
    if kind == "claude_usage_limit" {
        return true;
    }
    if kind == "server_overloaded" {
        return false;
    }
    let message = message.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "usagelimitexceeded",
        "ratelimitexceeded",
        "usage_limit",
        "rate_limit",
        "insufficient_quota",
        "resource_exhausted",
        "usage limit",
        "rate limit",
        "session limit",
        "too many requests",
        "exceeded your current quota",
        "quota exceeded",
        "quota exhausted",
        "exceeded quota",
        "insufficient credits",
        "insufficient balance",
        "credit balance is too low",
        "out of credits",
        "named models unavailable",
    ];
    MARKERS.iter().any(|marker| message.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_limit_failures() {
        assert!(is_usage_limit_failure("claude_usage_limit", ""));
        assert!(is_usage_limit_failure(
            "codex_terminal",
            "You've hit your usage limit. Try again in 3 hours."
        ));
        assert!(is_usage_limit_failure(
            "opencode_terminal",
            "429 Too Many Requests"
        ));
        assert!(is_usage_limit_failure(
            "adapter_error",
            "You exceeded your current quota, please check your plan"
        ));
        assert!(is_usage_limit_failure(
            "cursor_terminal",
            "ActionRequiredError: Named models unavailable"
        ));
        assert!(!is_usage_limit_failure(
            "server_overloaded",
            "rate limit: overloaded"
        ));
        assert!(!is_usage_limit_failure(
            "codex_terminal",
            "permission denied"
        ));
    }

    #[test]
    fn carried_permissions_never_widen() {
        // Antigravity offers only ask or bypass, so auto narrows to ask.
        let carried = carry_permission(
            "claude-code",
            Some(PermissionMode::Auto),
            false,
            "antigravity",
        )
        .unwrap();
        assert_eq!(carried.permission_id.as_deref(), Some("default"));
        assert!(!carried.plan_mode);
        // Bypass stays bypass where it exists.
        let carried =
            carry_permission("codex", Some(PermissionMode::Bypass), false, "antigravity").unwrap();
        assert_eq!(carried.permission_id.as_deref(), Some("bypass"));
        // Claude's Plan becomes the command-plan flag on Codex.
        let carried =
            carry_permission("claude-code", Some(PermissionMode::Plan), false, "codex").unwrap();
        assert!(carried.plan_mode);
        let carried = carried
            .permission_id
            .and_then(|id| PermissionMode::from_id(&id));
        assert!(carried.is_some_and(|mode| reach(mode) <= reach(PermissionMode::Ask)));
        // Codex Plan becomes Claude's plan permission.
        let carried =
            carry_permission("codex", Some(PermissionMode::Auto), true, "claude-code").unwrap();
        assert_eq!(carried.permission_id.as_deref(), Some("plan"));
        assert!(!carried.plan_mode);
    }

    #[test]
    fn candidates_follow_the_users_order() {
        let policy = HarnessFailover {
            enabled: true,
            order: vec!["codex".into(), "claude-code".into(), "opencode".into()],
        };
        assert_eq!(
            policy.candidates("claude-code", &[]),
            vec!["codex".to_string(), "opencode".to_string()]
        );
        assert_eq!(
            policy.candidates("claude-code", &["codex".into()]),
            vec!["opencode".to_string()]
        );
        let disabled = HarnessFailover {
            enabled: false,
            ..policy
        };
        assert!(disabled.candidates("claude-code", &[]).is_empty());
    }

    #[test]
    fn empty_order_tries_every_other_chat_harness() {
        let policy = HarnessFailover {
            enabled: true,
            order: Vec::new(),
        };
        let candidates = policy.candidates("claude-code", &[]);
        assert!(!candidates.contains(&"claude-code".to_string()));
        assert!(candidates.contains(&"codex".to_string()));
    }

    #[test]
    fn validation_dedupes_and_rejects_unknown_ids() {
        let policy = HarnessFailover {
            enabled: true,
            order: vec!["codex".into(), "codex".into()],
        }
        .validated()
        .unwrap();
        assert_eq!(policy.order, vec!["codex".to_string()]);
        assert!(HarnessFailover {
            enabled: true,
            order: vec!["nope".into()],
        }
        .validated()
        .is_err());
        assert_eq!(
            HarnessFailover::from_stored(Some("garbage")),
            HarnessFailover::default()
        );
    }
}
