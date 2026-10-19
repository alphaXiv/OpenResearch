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

/// After a failed auto-compact, wait this long before trying again, so a
/// harness that cannot compact does not get asked after every turn.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    pub percent: u8,
}

impl Settings {
    fn from_stored(enabled: Option<bool>, percent: Option<u8>) -> Self {
        Self {
            enabled: enabled.unwrap_or(true),
            percent: percent
                .unwrap_or(DEFAULT_PERCENT)
                .clamp(MIN_PERCENT, MAX_PERCENT),
        }
    }
}

pub fn settings() -> Settings {
    let (enabled, percent) = crate::telemetry::auto_compact();
    Settings::from_stored(enabled, percent)
}

pub fn set_settings(enabled: Option<bool>, percent: Option<u8>) -> anyhow::Result<Settings> {
    if let Some(percent) = percent {
        if !(MIN_PERCENT..=MAX_PERCENT).contains(&percent) {
            anyhow::bail!("the threshold must be between {MIN_PERCENT}% and {MAX_PERCENT}%");
        }
    }
    crate::telemetry::set_auto_compact(enabled, percent)?;
    Ok(settings())
}

/// How full the window is, in whole percent, when the window is known.
pub fn used_percent(usage: &ContextUsage) -> Option<u8> {
    let window = usage.context_window.filter(|window| *window > 0)?;
    Some((usage.used_tokens.saturating_mul(100) / window).min(100) as u8)
}

/// The fill percentage to report when this usage should trigger an
/// auto-compact, `None` otherwise. A harness that does not report its window
/// is left to its own compaction.
fn trigger(settings: Settings, usage: &ContextUsage) -> Option<u8> {
    if !settings.enabled {
        return None;
    }
    used_percent(usage).filter(|percent| *percent >= settings.percent)
}

static FAILED_AT: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// Whether the turn that just ended in `session_id` with `usage` should be
/// followed by an auto-compact; `Some(percent full)` when it should.
pub fn due(session_id: &str, usage: &ContextUsage) -> Option<u8> {
    let percent = trigger(settings(), usage)?;
    let recently_failed = FAILED_AT
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|failed| failed.get(session_id))
        .is_some_and(|at| at.elapsed() < RETRY_AFTER_FAILURE);
    (!recently_failed).then_some(percent)
}

/// Record how an auto-compact ended, for the retry back-off.
pub fn record_outcome(session_id: &str, succeeded: bool) {
    let mut failed = FAILED_AT.lock().unwrap();
    let failed = failed.get_or_insert_with(HashMap::new);
    if succeeded {
        failed.remove(session_id);
    } else {
        failed.insert(session_id.to_string(), Instant::now());
    }
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

    #[test]
    fn stored_settings_default_on_at_eighty_and_clamp() {
        assert_eq!(
            Settings::from_stored(None, None),
            Settings {
                enabled: true,
                percent: 80
            }
        );
        assert_eq!(Settings::from_stored(Some(false), Some(10)).percent, 50);
        assert_eq!(Settings::from_stored(None, Some(99)).percent, 95);
    }

    #[test]
    fn triggers_only_past_the_threshold_of_a_known_window() {
        let on = Settings {
            enabled: true,
            percent: 80,
        };
        assert_eq!(trigger(on, &usage(150_000, Some(200_000))), None);
        assert_eq!(trigger(on, &usage(160_000, Some(200_000))), Some(80));
        assert_eq!(trigger(on, &usage(250_000, Some(200_000))), Some(100));
        assert_eq!(trigger(on, &usage(190_000, None)), None);
        assert_eq!(trigger(on, &usage(190_000, Some(0))), None);
        let off = Settings {
            enabled: false,
            ..on
        };
        assert_eq!(trigger(off, &usage(199_000, Some(200_000))), None);
    }

    #[test]
    fn a_failure_backs_off_until_a_success() {
        let session = format!("auto-compact-{}", uuid::Uuid::new_v4());
        record_outcome(&session, false);
        let failed = FAILED_AT
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|failed| failed.get(&session).copied());
        assert!(failed.is_some_and(|at| at.elapsed() < RETRY_AFTER_FAILURE));
        record_outcome(&session, true);
        assert!(FAILED_AT
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|failed| !failed.contains_key(&session)));
    }
}
