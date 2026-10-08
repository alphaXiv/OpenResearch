//! Harness failover: when a turn stops on a usage, rate, or quota limit, the
//! chat continues on the next harness the user lined up instead of failing.
//!
//! The policy is opt-in and lives on the global UI state. The order is the
//! user's; an empty order falls back to registry order, so turning failover on
//! without arranging anything still tries every other chat harness.

use serde::{Deserialize, Serialize};

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
