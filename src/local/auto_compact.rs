//! Auto-compact: when a turn ends with the context window past a threshold,
//! orx compacts the chat before the next turn starts, the way Claude Code's
//! autocompact does. It reuses the manual `/compact` machinery
//! (`ChatHost::compact_session`): native compaction where the harness has one,
//! a summary reseeded into a fresh session where it does not.
//!
//! The check runs between turns, so the threshold leaves room for the next
//! turn; a harness's own in-turn autocompact stays the backstop for a single
//! turn that fills the window. The goal and the sleep-mode plan (objective,
//! budget, deadline) ride every turn as context, so they survive compaction.
//!
//! Two limits keep it from spending more than it saves. A token limit
//! compacts a large window long before a percentage of it would: every turn
//! re-reads the whole context, so a 1M-token window left to fill to 80% costs
//! several times as much per turn as one kept near the limit. And a
//! compaction that would free little — the chat has barely grown since the
//! last one — is skipped, since each one re-reads the whole context too.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::chat::ContextUsage;

/// Default threshold, in percent of the context window.
pub const DEFAULT_PERCENT: u8 = 80;
/// Lowest threshold accepted: compacting a half-empty window loses detail for
/// little gain.
pub const MIN_PERCENT: u8 = 50;
/// Highest threshold accepted: past this the next turn may not fit.
pub const MAX_PERCENT: u8 = 95;
/// Default token limit. Below the percentage threshold of a 200K window, so it
/// only changes anything for larger windows.
pub const DEFAULT_MAX_TOKENS: u64 = 200_000;
/// Lowest token limit accepted: below this the summary and the system prompt
/// alone fill most of it.
pub const MIN_MAX_TOKENS: u64 = 50_000;

/// After a failed auto-compact, wait this long before trying again, so a
/// harness that cannot compact does not get asked after every turn.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(15 * 60);

/// Share of the trigger level the chat must have grown by since its last
/// compaction before another one is worth its cost.
const MIN_GROWTH_DIVISOR: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    pub percent: u8,
    /// Compact once this many tokens are in use, whatever the window. `None`
    /// leaves the percentage alone to decide.
    pub max_tokens: Option<u64>,
}

impl Settings {
    fn from_stored(enabled: Option<bool>, percent: Option<u8>, max_tokens: Option<u64>) -> Self {
        Self {
            enabled: enabled.unwrap_or(true),
            percent: percent
                .unwrap_or(DEFAULT_PERCENT)
                .clamp(MIN_PERCENT, MAX_PERCENT),
            max_tokens: match max_tokens.unwrap_or(DEFAULT_MAX_TOKENS) {
                0 => None,
                tokens => Some(tokens.max(MIN_MAX_TOKENS)),
            },
        }
    }

    /// Tokens in use at which a window of `window` tokens gets compacted.
    fn limit(&self, window: u64) -> u64 {
        let by_percent = window.saturating_mul(u64::from(self.percent)) / 100;
        self.max_tokens
            .map_or(by_percent, |tokens| by_percent.min(tokens))
    }
}

pub fn settings() -> Settings {
    let (enabled, percent, max_tokens) = crate::telemetry::auto_compact();
    Settings::from_stored(enabled, percent, max_tokens)
}

/// `max_tokens` of `0` turns the token limit off.
pub fn set_settings(
    enabled: Option<bool>,
    percent: Option<u8>,
    max_tokens: Option<u64>,
) -> anyhow::Result<Settings> {
    if let Some(percent) = percent {
        if !(MIN_PERCENT..=MAX_PERCENT).contains(&percent) {
            anyhow::bail!("the threshold must be between {MIN_PERCENT}% and {MAX_PERCENT}%");
        }
    }
    if let Some(tokens) = max_tokens {
        if tokens != 0 && tokens < MIN_MAX_TOKENS {
            anyhow::bail!("the token limit must be at least {MIN_MAX_TOKENS} tokens");
        }
    }
    crate::telemetry::set_auto_compact(enabled, percent, max_tokens)?;
    Ok(settings())
}

/// How full the window is, in whole percent, when the window is known.
pub fn used_percent(usage: &ContextUsage) -> Option<u8> {
    let window = usage.context_window.filter(|window| *window > 0)?;
    Some((usage.used_tokens.saturating_mul(100) / window).min(100) as u8)
}

/// Why an auto-compact is due: how full the window is, and the token count
/// that triggered it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Trigger {
    percent: u8,
    limit: u64,
}

/// `Some` when this usage is past the threshold, `None` otherwise. A harness
/// that does not report its window is left to its own compaction.
fn trigger(settings: Settings, usage: &ContextUsage) -> Option<Trigger> {
    if !settings.enabled {
        return None;
    }
    let window = usage.context_window.filter(|window| *window > 0)?;
    let limit = settings.limit(window);
    (usage.used_tokens >= limit).then(|| Trigger {
        percent: used_percent(usage).unwrap_or(100),
        limit,
    })
}

/// Whether compacting again would free enough to pay for itself: the chat
/// must have grown by a share of the trigger level since the context it
/// restarted from after the last compaction.
fn worth_compacting(trigger: Trigger, used: u64, baseline: Option<u64>) -> bool {
    baseline
        .is_none_or(|baseline| used.saturating_sub(baseline) >= trigger.limit / MIN_GROWTH_DIVISOR)
}

#[derive(Default)]
struct SessionState {
    failed_at: Option<Instant>,
    /// A compaction finished and the next usage report is the context the
    /// chat restarted from.
    awaiting_baseline: bool,
    /// Tokens in use at the first request after the last compaction.
    baseline: Option<u64>,
}

static STATE: Mutex<Option<HashMap<String, SessionState>>> = Mutex::new(None);

fn with_state<T>(session_id: &str, f: impl FnOnce(&mut SessionState) -> T) -> T {
    let mut state = STATE.lock().unwrap();
    f(state
        .get_or_insert_with(HashMap::new)
        .entry(session_id.to_string())
        .or_default())
}

/// Whether the turn that just ended in `session_id` with `usage` should be
/// followed by an auto-compact; `Some(percent full)` when it should.
pub fn due(session_id: &str, usage: &ContextUsage) -> Option<u8> {
    let trigger = trigger(settings(), usage)?;
    let state = STATE.lock().unwrap();
    let state = state.as_ref().and_then(|state| state.get(session_id));
    let recently_failed = state
        .and_then(|state| state.failed_at)
        .is_some_and(|at| at.elapsed() < RETRY_AFTER_FAILURE);
    let baseline = state.and_then(|state| state.baseline);
    (!recently_failed && worth_compacting(trigger, usage.used_tokens, baseline))
        .then_some(trigger.percent)
}

/// A turn in `session_id` reported `used_tokens` in use. The first report
/// after a compaction is the size the chat restarted from.
pub fn observe(session_id: &str, used_tokens: u64) {
    if used_tokens == 0 {
        return;
    }
    let mut state = STATE.lock().unwrap();
    if let Some(state) = state
        .as_mut()
        .and_then(|state| state.get_mut(session_id))
        .filter(|state| state.awaiting_baseline)
    {
        state.awaiting_baseline = false;
        state.baseline = Some(used_tokens);
    }
}

/// A compaction of `session_id`, manual or automatic, finished.
pub fn record_compacted(session_id: &str) {
    with_state(session_id, |state| {
        state.failed_at = None;
        state.awaiting_baseline = true;
        state.baseline = None;
    });
}

/// An auto-compact of `session_id` failed; back off before the next one.
pub fn record_failure(session_id: &str) {
    with_state(session_id, |state| state.failed_at = Some(Instant::now()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(used: u64, window: Option<u64>) -> ContextUsage {
        ContextUsage {
            used_tokens: used,
            context_window: window,
        }
    }

    fn percent_only(percent: u8) -> Settings {
        Settings {
            enabled: true,
            percent,
            max_tokens: None,
        }
    }

    #[test]
    fn stored_settings_default_on_at_eighty_with_a_token_limit_and_clamp() {
        assert_eq!(
            Settings::from_stored(None, None, None),
            Settings {
                enabled: true,
                percent: 80,
                max_tokens: Some(DEFAULT_MAX_TOKENS),
            }
        );
        assert_eq!(
            Settings::from_stored(Some(false), Some(10), None).percent,
            50
        );
        assert_eq!(Settings::from_stored(None, Some(99), None).percent, 95);
        assert_eq!(Settings::from_stored(None, None, Some(0)).max_tokens, None);
        assert_eq!(
            Settings::from_stored(None, None, Some(10)).max_tokens,
            Some(MIN_MAX_TOKENS)
        );
    }

    #[test]
    fn triggers_only_past_the_threshold_of_a_known_window() {
        let on = percent_only(80);
        let fired = |used, window| trigger(on, &usage(used, window)).map(|t| t.percent);
        assert_eq!(fired(150_000, Some(200_000)), None);
        assert_eq!(fired(160_000, Some(200_000)), Some(80));
        assert_eq!(fired(250_000, Some(200_000)), Some(100));
        assert_eq!(fired(190_000, None), None);
        assert_eq!(fired(190_000, Some(0)), None);
        let off = Settings {
            enabled: false,
            ..on
        };
        assert_eq!(trigger(off, &usage(199_000, Some(200_000))), None);
    }

    #[test]
    fn the_token_limit_compacts_a_large_window_early() {
        let capped = Settings {
            max_tokens: Some(200_000),
            ..percent_only(80)
        };
        // 1M window: the percentage alone would wait for 800K.
        assert_eq!(trigger(capped, &usage(150_000, Some(1_000_000))), None);
        assert_eq!(
            trigger(capped, &usage(200_000, Some(1_000_000))),
            Some(Trigger {
                percent: 20,
                limit: 200_000
            })
        );
        // 200K window: 80% comes first, the limit changes nothing.
        assert_eq!(
            trigger(capped, &usage(160_000, Some(200_000))).map(|t| t.limit),
            Some(160_000)
        );
    }

    #[test]
    fn a_compaction_that_would_free_little_is_skipped() {
        let fired = Trigger {
            percent: 80,
            limit: 160_000,
        };
        assert!(worth_compacting(fired, 170_000, None));
        // Restarted from 130K: 40K of growth is a quarter of the trigger level.
        assert!(!worth_compacting(fired, 165_000, Some(130_000)));
        assert!(worth_compacting(fired, 170_000, Some(130_000)));
        assert!(worth_compacting(fired, 170_000, Some(30_000)));
    }

    #[test]
    fn the_first_report_after_a_compaction_is_the_baseline() {
        let session = format!("auto-compact-{}", uuid::Uuid::new_v4());
        let baseline = || {
            STATE
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|state| state.get(&session))
                .and_then(|state| state.baseline)
        };
        observe(&session, 90_000);
        assert_eq!(baseline(), None);
        record_compacted(&session);
        observe(&session, 0);
        observe(&session, 30_000);
        observe(&session, 60_000);
        assert_eq!(baseline(), Some(30_000));
    }

    #[test]
    fn a_failure_backs_off_until_a_success() {
        let session = format!("auto-compact-{}", uuid::Uuid::new_v4());
        let failed_at = || {
            STATE
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|state| state.get(&session))
                .and_then(|state| state.failed_at)
        };
        record_failure(&session);
        assert!(failed_at().is_some_and(|at| at.elapsed() < RETRY_AFTER_FAILURE));
        record_compacted(&session);
        assert!(failed_at().is_none());
    }
}
