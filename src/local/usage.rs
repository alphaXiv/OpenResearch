//! `/api/usage`: what is left of the machine's agent plans, and what its
//! OpenResearch chats and runs spent over the last seven days.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::local::chat::ChatHost;
use crate::local::codex::CodexHost;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    /// None until the first read of this machine's sign-ins finishes; empty
    /// when none of them is on a plan.
    pub plans: Option<Vec<PlanUsage>>,
    /// The last seven days in the machine's local time, oldest first, as
    /// `YYYY-MM-DD`; today is last.
    pub days: Vec<String>,
    pub agents: Vec<AgentSpend>,
    pub compute: Vec<BackendSpend>,
    /// The chat `?chat=` asked about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<ChatSpend>,
}

/// One signed-in subscription: the user's own sign-in or an agent profile's.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanUsage {
    /// `claude-code` or `codex`.
    pub harness: String,
    /// The agent profile's id; None for the user's own sign-in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The plan's name as the provider gives it, such as "Claude Ultra".
    pub plan: Option<String>,
    /// The signed-in account's email.
    pub account: Option<String>,
    pub windows: Vec<PlanWindow>,
    /// Codex credits, only when the account has some.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<PlanCredits>,
    /// Claude extra usage, only when it is turned on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_usage: Option<PlanExtraUsage>,
    /// Codex resets that refill the limits now, only when one is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets: Option<PlanResets>,
    /// When `windows` were read, as RFC 3339.
    pub read_at: Option<String>,
    /// Why the latest read failed; `windows` then hold the values from `read_at`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<PlanError>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanWindow {
    /// The window's length in minutes, when the provider gives it.
    pub minutes: Option<u64>,
    /// The model a per-model window covers, as the provider names it; None
    /// for the plan-wide windows.
    pub model: Option<String>,
    /// What is left, 0 to 100: providers that report use are converted here
    /// and nowhere else.
    pub remaining_percent: f64,
    /// RFC 3339.
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanCredits {
    pub balance: Option<String>,
    pub unlimited: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanExtraUsage {
    pub used_micros: u64,
    pub limit_micros: Option<u64>,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanResets {
    pub available: u64,
    /// The available ones the provider lists, which may be fewer than
    /// `available`; None when it gives only the count.
    pub credits: Option<Vec<ResetCredit>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResetCredit {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// RFC 3339.
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PlanError {
    /// The installed agent is too old to report usage; `version` is empty when unknown.
    Unsupported {
        version: String,
    },
    Failed {
        message: String,
    },
}

/// One agent's spend in one currency, agent profiles folded in.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpend {
    pub harness: String,
    /// ISO 4217.
    pub currency: String,
    /// Millionths of `currency` per day in `UsageReport::days`, of what has a price.
    pub day_micros: Vec<u64>,
    /// Days with tokens that neither the agent nor the price list put a price on.
    pub unpriced: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackendSpend {
    /// The run backend's kind, such as `modal_job`.
    pub backend: String,
    pub currency: String,
    pub day_micros: Vec<u64>,
    /// Runs in these days on this backend whose price couldn't be read.
    pub unpriced: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatSpend {
    /// What the chat's agent has cost, per currency, largest first.
    pub agent: Vec<crate::store::Cost>,
    /// Runs the chat launched on backends that cost money.
    pub compute: Option<ChatCompute>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatCompute {
    pub runs: u32,
    pub cost_micros: u64,
    pub currency: String,
    /// One of the runs is still going, so `cost_micros` grows.
    pub running: bool,
    pub unpriced: u32,
}

/// A plan snapshot older than this is read again when someone asks for it.
pub const STALE_AFTER: Duration = Duration::from_secs(60);

/// The machine's plan snapshots, held by `orx up` and read by `/api/usage`.
pub static PLANS: LazyLock<Plans> = LazyLock::new(Plans::default);

/// A sign-in that may be on a plan: the user's own to a harness, or an agent
/// profile's, with what detection already knows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Reader {
    pub harness: String,
    pub profile: Option<String>,
    pub bin: PathBuf,
    pub version: Option<String>,
    pub plan: Option<String>,
    pub email: Option<String>,
}

impl Reader {
    /// Another account only when both emails are known: detection can report a
    /// sign-in before it has read the email, and that is the same account.
    fn same_account(&self, other: &Reader) -> bool {
        match (&self.email, &other.email) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => true,
        }
    }

    /// The entry's account when it might be on a plan. A signed-out agent, an
    /// API key or an agent turned off in Settings is known to have none; a
    /// sign-in orx couldn't check is read.
    pub fn from_entry(entry: &Value) -> Option<Reader> {
        let harness = entry["id"]
            .as_str()
            .filter(|id| matches!(*id, "claude-code" | "codex"))?;
        let api_key = matches!(
            entry["authMethod"].as_str(),
            Some("apiKey" | "thirdParty" | "local")
        );
        if api_key
            || entry["authState"] == "needsLogin"
            || entry["installed"] == false
            || entry["off"] == true
        {
            return None;
        }
        Some(Reader {
            harness: harness.to_string(),
            profile: entry["profile"]["id"].as_str().map(str::to_string),
            bin: PathBuf::from(entry["binPath"].as_str()?),
            // Detection keeps the whole `--version` line: "codex-cli 0.169.0".
            version: entry["version"].as_str().and_then(|line| {
                line.split_whitespace()
                    .find(|word| word.starts_with(|c: char| c.is_ascii_digit()))
                    .map(str::to_string)
            }),
            plan: entry["plan"]
                .as_str()
                .map(|plan| provider_plan(harness, plan)),
            email: entry["account"].as_str().map(str::to_string),
        })
    }

    fn is(&self, harness: &str, profile: Option<&str>) -> bool {
        self.harness == harness && self.profile.as_deref() == profile
    }

    /// The profile's own agent home; None for the user's own sign-in.
    pub fn home(&self) -> crate::error::Result<Option<PathBuf>> {
        self.profile
            .as_deref()
            .map(|id| crate::local::agent_profiles::home(id, &self.harness))
            .transpose()
    }

    async fn read(&self, codex: &CodexHost) -> Read {
        let home = match self.home() {
            Ok(home) => home,
            Err(error) => return Read::Failed(format!("{error:#}")),
        };
        if self.harness == "claude-code" {
            crate::local::harness::claude::read_plan(self.bin.clone(), home).await
        } else {
            let profile = self.profile.as_deref();
            crate::local::harness::codex::read_plan(codex, profile, &self.bin, home.as_deref())
                .await
        }
    }
}

/// The tier with its provider in front: "Ultra" reads as "Claude Ultra".
fn provider_plan(harness: &str, plan: &str) -> String {
    let provider = if harness == "codex" {
        "ChatGPT"
    } else {
        "Claude"
    };
    if plan.starts_with(provider) {
        plan.to_string()
    } else {
        format!("{provider} {plan}")
    }
}

/// Why a throwaway agent child gave no answer.
#[derive(Debug)]
pub enum Unanswered {
    /// The binary is missing or couldn't start.
    Spawn(std::io::Error),
    /// It exited or closed its output before answering.
    Closed,
    TimedOut,
}

impl Unanswered {
    pub fn message(self, agent: &str, bin: &Path) -> String {
        match self {
            Unanswered::Spawn(error) if error.kind() == std::io::ErrorKind::NotFound => {
                format!("{agent} isn't installed at {} anymore.", bin.display())
            }
            Unanswered::Spawn(error) => {
                format!("{agent} at {} couldn't start: {error}.", bin.display())
            }
            Unanswered::Closed => format!("{agent} exited without answering."),
            Unanswered::TimedOut => format!("{agent} didn't answer within 15 seconds."),
        }
    }
}

/// What one read of an account's plan found.
#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    Plan(Snapshot),
    /// The account is on no plan: an API key or a provider without limits.
    NoPlan,
    /// The installed agent is too old to report usage.
    Unsupported,
    Failed(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// The plan's name as the read reported it, when it did.
    pub plan: Option<String>,
    pub windows: Vec<PlanWindow>,
    pub credits: Option<PlanCredits>,
    pub extra_usage: Option<PlanExtraUsage>,
    pub resets: Option<PlanResets>,
}

/// What is left of a window a provider reports as percent used.
pub fn remaining_percent(used: f64) -> f64 {
    (100.0 - used).clamp(0.0, 100.0)
}

#[derive(Default)]
pub struct Plans(Mutex<State>);

#[derive(Default)]
struct State {
    /// A sweep found every account and read each once.
    settled: bool,
    sweeping: bool,
    swept_at: Option<Instant>,
    slots: Vec<Slot>,
}

struct Slot {
    reader: Reader,
    /// None until a read finds a plan, and after one finds none.
    usage: Option<PlanUsage>,
    attempted_at: Option<Instant>,
    reading: bool,
}

impl Slot {
    fn due(&self, now: Instant, force: bool) -> bool {
        !self.reading
            && (force
                || self
                    .attempted_at
                    .is_none_or(|at| now.duration_since(at) >= STALE_AFTER))
    }
}

impl Plans {
    /// Every plan the machine's sign-ins are on; None until the first sweep
    /// ends, and while any account hasn't been read since it appeared: an
    /// account orx hasn't asked yet is unknown, not one without a plan.
    pub fn report(&self) -> Option<Vec<PlanUsage>> {
        let state = self.0.lock().unwrap();
        let unread = state
            .slots
            .iter()
            .any(|slot| slot.usage.is_none() && slot.attempted_at.is_none());
        (state.settled && !unread).then(|| {
            let mut plans = state
                .slots
                .iter()
                .filter_map(|slot| slot.usage.clone())
                .collect::<Vec<_>>();
            plans.sort_by(|a, b| {
                (a.profile.is_some(), &a.harness, &a.profile).cmp(&(
                    b.profile.is_some(),
                    &b.harness,
                    &b.profile,
                ))
            });
            plans
        })
    }

    /// Claims the next sweep: one at a time, and only once the last is stale.
    /// The sweep ends when the returned guard drops, a panic included.
    pub fn begin_sweep(&self, now: Instant, force: bool) -> Option<Sweep<'_>> {
        let mut state = self.0.lock().unwrap();
        let due = force
            || state
                .swept_at
                .is_none_or(|at| now.duration_since(at) >= STALE_AFTER);
        if state.sweeping || !due {
            return None;
        }
        state.sweeping = true;
        state.swept_at = Some(now);
        Some(Sweep(self))
    }

    /// Takes the sweep's accounts as the machine's: a new one is added, and one
    /// gone, signed out or removed, or now signed in as someone else drops its
    /// snapshot.
    pub fn sync(&self, readers: Vec<Reader>) {
        let mut state = self.0.lock().unwrap();
        let mut slots = std::mem::take(&mut state.slots);
        state.slots = readers
            .into_iter()
            .map(|reader| {
                let kept = slots
                    .iter()
                    .position(|slot| slot.reader.is(&reader.harness, reader.profile.as_deref()))
                    .map(|at| slots.swap_remove(at));
                match kept {
                    Some(slot) if slot.reader.same_account(&reader) => Slot { reader, ..slot },
                    Some(slot) => Slot {
                        reader,
                        usage: None,
                        attempted_at: None,
                        reading: slot.reading,
                    },
                    None => Slot {
                        reader,
                        usage: None,
                        attempted_at: None,
                        reading: false,
                    },
                }
            })
            .collect();
    }

    /// Marks each account due for a read, or only `account` when given, as
    /// being read and returns it, so concurrent callers never read one
    /// account twice.
    pub fn claim(
        &self,
        account: Option<(&str, Option<&str>)>,
        now: Instant,
        force: bool,
    ) -> Vec<Reader> {
        let mut state = self.0.lock().unwrap();
        state
            .slots
            .iter_mut()
            .filter(|slot| {
                account.is_none_or(|(harness, profile)| slot.reader.is(harness, profile))
                    && slot.due(now, force)
            })
            .map(|slot| {
                slot.reading = true;
                slot.reader.clone()
            })
            .collect()
    }

    /// The account's reader, whether or not a read of it is due.
    pub fn reader(&self, harness: &str, profile: Option<&str>) -> Option<Reader> {
        let state = self.0.lock().unwrap();
        let slot = state
            .slots
            .iter()
            .find(|slot| slot.reader.is(harness, profile))?;
        Some(slot.reader.clone())
    }

    /// Records a claimed read. A failed one keeps the last values and the
    /// time they were read; one of an account the slot no longer holds is
    /// dropped, and the slot read again.
    pub fn finish(&self, reader: &Reader, read: Read, now: Instant, read_at: String) {
        // Also called while a panicking read unwinds, when the lock may be poisoned.
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(slot) = state
            .slots
            .iter_mut()
            .find(|slot| slot.reader.is(&reader.harness, reader.profile.as_deref()))
        else {
            return;
        };
        slot.reading = false;
        if !slot.reader.same_account(reader) {
            return;
        }
        slot.attempted_at = Some(now);
        let reader = &slot.reader;
        let unread = PlanUsage {
            harness: reader.harness.clone(),
            profile: reader.profile.clone(),
            plan: reader.plan.clone(),
            account: reader.email.clone(),
            windows: Vec::new(),
            credits: None,
            extra_usage: None,
            resets: None,
            read_at: None,
            error: None,
        };
        let error = match read {
            Read::Plan(snapshot) => {
                slot.usage = Some(PlanUsage {
                    plan: unread.plan.or_else(|| {
                        let read = snapshot.plan?;
                        Some(provider_plan(&reader.harness, &read))
                    }),
                    windows: snapshot.windows,
                    credits: snapshot.credits,
                    extra_usage: snapshot.extra_usage,
                    resets: snapshot.resets,
                    read_at: Some(read_at),
                    ..unread
                });
                return;
            }
            Read::NoPlan => {
                slot.usage = None;
                return;
            }
            Read::Unsupported => PlanError::Unsupported {
                version: reader.version.clone().unwrap_or_default(),
            },
            Read::Failed(message) => PlanError::Failed { message },
        };
        slot.usage.get_or_insert(unread).error = Some(error);
    }

    /// Reads every claimed account at once, each recorded as it finishes.
    pub async fn read(&self, readers: Vec<Reader>, codex: &CodexHost) {
        futures::future::join_all(readers.iter().map(|reader| async move {
            let mut claim = Claim {
                plans: self,
                reader: Some(reader),
            };
            let read = reader.read(codex).await;
            claim.reader = None;
            self.finish(
                reader,
                read,
                Instant::now(),
                crate::telemetry::iso8601_utc(crate::store::now_ms()),
            );
        }))
        .await;
    }
}

/// A sweep in progress; dropping it ends the sweep and settles the report.
pub struct Sweep<'a>(&'a Plans);

impl Drop for Sweep<'_> {
    fn drop(&mut self) {
        let mut state = self
            .0
             .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.sweeping = false;
        state.settled = true;
    }
}

/// A read that ends without being recorded, by a panic or a cancelled future,
/// frees its account for the next one.
struct Claim<'a> {
    plans: &'a Plans,
    reader: Option<&'a Reader>,
}

/// A read that ends without an answer, by a panic or by being dropped, is a
/// failed read: the panel then offers Try again instead of reading forever.
impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if let Some(reader) = self.reader {
            let read = Read::Failed("The usage read stopped before it answered.".into());
            self.plans
                .finish(reader, read, Instant::now(), String::new());
        }
    }
}

/// A running session saw its account's limits move: re-read that account's
/// plan unless its snapshot is fresh, then tell the dashboard.
pub fn observed(host: &Arc<ChatHost>, harness: &str, profile: Option<&str>) {
    let claimed = PLANS.claim(Some((harness, profile)), Instant::now(), false);
    if claimed.is_empty() {
        return;
    }
    let host = host.clone();
    tokio::spawn(async move {
        PLANS.read(claimed, &host.codex).await;
        host.emit_event("usage.changed", json!({}));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(harness: &str, profile: Option<&str>) -> Reader {
        Reader {
            harness: harness.into(),
            profile: profile.map(str::to_string),
            bin: PathBuf::from("/bin/agent"),
            version: Some("2.0.9".into()),
            plan: None,
            email: Some("ada@example.com".into()),
        }
    }

    fn settle(plans: &Plans) {
        drop(plans.begin_sweep(Instant::now(), true).unwrap());
    }

    fn window(remaining_percent: f64) -> PlanWindow {
        PlanWindow {
            minutes: Some(300),
            model: None,
            remaining_percent,
            resets_at: None,
        }
    }

    fn plan(remaining: f64) -> Read {
        Read::Plan(Snapshot {
            plan: Some("Ultra".into()),
            windows: vec![window(remaining)],
            ..Default::default()
        })
    }

    #[test]
    fn remaining_is_what_is_left_clamped_to_a_percent() {
        assert_eq!(remaining_percent(38.0), 62.0);
        assert_eq!(remaining_percent(0.0), 100.0);
        assert_eq!(remaining_percent(130.0), 0.0);
        assert_eq!(remaining_percent(-4.0), 100.0);
    }

    #[test]
    fn only_sign_ins_that_may_be_on_a_plan_are_read() {
        let own = json!({"id": "claude-code", "installed": true, "authState": "ready",
            "authMethod": "oauth", "binPath": "/bin/claude", "version": "2.1.323 (Claude Code)",
            "plan": "Ultra", "account": "ada@example.com"});
        let reader = Reader::from_entry(&own).unwrap();
        assert_eq!(reader.profile, None);
        assert_eq!(reader.version.as_deref(), Some("2.1.323"));
        assert_eq!(reader.plan.as_deref(), Some("Claude Ultra"));

        let profile = json!({"id": "codex", "installed": true, "authState": "ready",
            "authMethod": "oauth", "binPath": "/bin/codex", "version": "codex-cli 0.169.0",
            "plan": "Pro 200", "profile": {"id": "codex-abcdefghij"}});
        let reader = Reader::from_entry(&profile).unwrap();
        assert_eq!(reader.profile.as_deref(), Some("codex-abcdefghij"));
        assert_eq!(reader.version.as_deref(), Some("0.169.0"));
        assert_eq!(reader.plan.as_deref(), Some("ChatGPT Pro 200"));

        // A sign-in that couldn't be checked may still be on a plan.
        let unchecked = json!({"id": "claude-code", "installed": true, "authState": "unknown",
            "binPath": "/bin/claude"});
        assert!(Reader::from_entry(&unchecked).is_some());

        for absent in [
            json!({"id": "claude-code", "installed": true, "authState": "ready", "authMethod": "apiKey", "binPath": "/bin/claude"}),
            json!({"id": "codex", "installed": true, "authState": "needsLogin", "binPath": "/bin/codex"}),
            json!({"id": "claude-code", "installed": false, "authState": "unknown"}),
            json!({"id": "opencode", "installed": true, "authState": "ready", "binPath": "/bin/opencode"}),
            json!({"id": "codex", "installed": true, "authState": "ready", "off": true, "binPath": "/bin/codex"}),
        ] {
            assert_eq!(Reader::from_entry(&absent), None, "{absent}");
        }
    }

    #[test]
    fn plans_are_unknown_until_the_first_sweep_reads_every_account() {
        let plans = Plans::default();
        let now = Instant::now();
        let sweep = plans.begin_sweep(now, false).unwrap();
        assert!(
            plans.begin_sweep(now, true).is_none(),
            "one sweep at a time"
        );
        plans.sync(vec![reader("claude-code", None)]);
        let claimed = plans.claim(None, now, false);
        assert_eq!(plans.report(), None);

        plans.finish(&claimed[0], plan(40.0), now, "t1".into());
        drop(sweep);
        let report = plans.report().unwrap();
        assert_eq!(report[0].windows, vec![window(40.0)]);
        assert_eq!(report[0].plan.as_deref(), Some("Claude Ultra"));
        assert_eq!(report[0].read_at.as_deref(), Some("t1"));

        assert!(plans.begin_sweep(now + STALE_AFTER / 2, false).is_none());
        assert!(plans.begin_sweep(now + STALE_AFTER, false).is_some());
    }

    #[test]
    fn a_sweep_with_no_plans_settles_on_none() {
        let plans = Plans::default();
        let sweep = plans.begin_sweep(Instant::now(), false).unwrap();
        plans.sync(Vec::new());
        drop(sweep);
        assert_eq!(plans.report(), Some(Vec::new()));
    }

    #[test]
    fn a_sweep_or_read_that_panics_still_ends() {
        let plans = Plans::default();
        let now = Instant::now();
        plans.sync(vec![reader("codex", None)]);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _sweep = plans.begin_sweep(now, false).unwrap();
            let claimed = plans.claim(None, now, false);
            let _claim = Claim {
                plans: &plans,
                reader: Some(&claimed[0]),
            };
            panic!("the read panicked");
        }));
        assert!(panicked.is_err());
        let report = plans.report().expect("the sweep settled");
        assert!(
            matches!(report[0].error, Some(PlanError::Failed { .. })),
            "the read that panicked is a failed read, with Try again"
        );
        assert!(
            plans.begin_sweep(now, true).is_some(),
            "a refresh can start again"
        );
        assert_eq!(
            plans.claim(None, now, true).len(),
            1,
            "Try again reads it; it isn't left reading"
        );
    }

    #[test]
    fn a_sign_in_whose_email_detection_hasnt_read_yet_keeps_its_plan() {
        let plans = Plans::default();
        let now = Instant::now();
        let known = reader("codex", None);
        let mut unread = known.clone();
        unread.email = None;

        plans.sync(vec![known.clone()]);
        plans.claim(None, now, false);
        plans.finish(&known, plan(40.0), now, "t1".into());
        settle(&plans);
        plans.sync(vec![unread.clone()]);
        assert_eq!(plans.report().unwrap()[0].windows, vec![window(40.0)]);

        // A forced read that started before detection had the email still lands.
        assert_eq!(plans.claim(None, now, true), vec![unread.clone()]);
        plans.sync(vec![known.clone()]);
        plans.finish(&unread, plan(30.0), now, "t2".into());
        assert_eq!(plans.report().unwrap()[0].windows, vec![window(30.0)]);
    }

    #[test]
    fn an_account_signed_in_as_someone_else_drops_the_last_ones_plan() {
        let plans = Plans::default();
        let now = Instant::now();
        let ada = reader("claude-code", None);
        let mut grace = ada.clone();
        grace.email = Some("grace@example.com".into());

        plans.sync(vec![ada.clone()]);
        plans.claim(None, now, false);
        plans.finish(&ada, plan(40.0), now, "t1".into());
        settle(&plans);
        plans.sync(vec![grace.clone()]);
        assert_eq!(
            plans.report(),
            None,
            "the new account is unknown until read"
        );

        // A read of the old account still in flight when the account changes is dropped.
        plans.sync(vec![ada.clone()]);
        assert_eq!(plans.claim(None, now, true), vec![ada.clone()]);
        plans.sync(vec![grace.clone()]);
        plans.finish(&ada, plan(10.0), now, "t2".into());
        assert_eq!(plans.report(), None);

        assert_eq!(
            plans.claim(None, now, false),
            vec![grace.clone()],
            "read again at once"
        );
        plans.finish(&grace, plan(70.0), now, "t3".into());
        let report = plans.report().unwrap();
        assert_eq!(report[0].account.as_deref(), Some("grace@example.com"));
        assert_eq!(report[0].windows, vec![window(70.0)]);
    }

    #[test]
    fn an_account_is_read_once_at_a_time_and_again_when_stale() {
        let plans = Plans::default();
        let now = Instant::now();
        plans.sync(vec![reader("codex", None)]);
        let claimed = plans.claim(None, now, false);
        assert_eq!(claimed.len(), 1);
        assert!(
            plans.claim(None, now, true).is_empty(),
            "a read in flight is never doubled"
        );
        assert!(plans.claim(Some(("codex", None)), now, true).is_empty());

        plans.finish(&claimed[0], plan(90.0), now, "t1".into());
        assert!(plans.claim(None, now + STALE_AFTER / 2, false).is_empty());
        assert!(plans
            .claim(Some(("codex", None)), now + STALE_AFTER / 2, false)
            .is_empty());
        assert_eq!(
            plans.claim(None, now + STALE_AFTER / 2, true).len(),
            1,
            "a refresh forces it"
        );
    }

    #[test]
    fn a_failed_read_keeps_the_last_values_and_their_time() {
        let plans = Plans::default();
        let now = Instant::now();
        plans.sync(vec![reader("claude-code", None)]);
        let claimed = plans.claim(None, now, false).remove(0);
        plans.finish(&claimed, plan(55.0), now, "t1".into());

        plans.claim(None, now + STALE_AFTER, false);
        plans.finish(
            &claimed,
            Read::Failed("offline".into()),
            now + STALE_AFTER,
            "t2".into(),
        );
        settle(&plans);
        let usage = plans.report().unwrap().remove(0);
        assert_eq!(usage.windows, vec![window(55.0)]);
        assert_eq!(usage.read_at.as_deref(), Some("t1"));
        assert_eq!(
            usage.error,
            Some(PlanError::Failed {
                message: "offline".into()
            })
        );

        plans.claim(None, now + STALE_AFTER * 2, false);
        plans.finish(&claimed, plan(50.0), now + STALE_AFTER * 2, "t3".into());
        assert_eq!(plans.report().unwrap()[0].error, None);
    }

    #[test]
    fn an_agent_too_old_to_report_names_its_version_when_known() {
        let plans = Plans::default();
        let now = Instant::now();
        let mut unversioned = reader("codex", None);
        unversioned.version = None;
        plans.sync(vec![reader("claude-code", None), unversioned]);
        for claimed in plans.claim(None, now, false) {
            plans.finish(&claimed, Read::Unsupported, now, "t1".into());
        }
        settle(&plans);
        let report = plans.report().unwrap();
        assert_eq!(
            report[0].error,
            Some(PlanError::Unsupported {
                version: "2.0.9".into()
            })
        );
        assert_eq!(report[0].read_at, None);
        assert_eq!(
            report[1].error,
            Some(PlanError::Unsupported {
                version: String::new()
            }),
            "still offers the update"
        );
    }

    #[test]
    fn accounts_on_no_plan_or_gone_leave_the_report() {
        let plans = Plans::default();
        let now = Instant::now();
        plans.sync(vec![
            reader("codex", Some("codex-abcdefghij")),
            reader("claude-code", None),
            reader("codex", None),
        ]);
        for claimed in plans.claim(None, now, false) {
            let read = if claimed.harness == "claude-code" {
                Read::NoPlan
            } else {
                plan(10.0)
            };
            plans.finish(&claimed, read, now, "t1".into());
        }
        settle(&plans);
        let report = plans.report().unwrap();
        assert_eq!(
            report
                .iter()
                .map(|usage| (usage.harness.as_str(), usage.profile.as_deref()))
                .collect::<Vec<_>>(),
            [("codex", None), ("codex", Some("codex-abcdefghij"))],
            "the user's own sign-in first, Claude's API key absent"
        );

        plans.sync(vec![reader("codex", None)]);
        assert_eq!(plans.report().unwrap().len(), 1);
        assert!(
            plans.claim(None, now, false).is_empty(),
            "a kept account keeps its snapshot"
        );
    }
}
