//! What a chat has not been told yet: a finished turn or a question from a
//! chat it subscribes to, and the end of a run it asked to be woken for. Each
//! idle chat's next turn starts with all of its news at once, unless the agent
//! collected chat news first with `wait`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::delegation::{
    answer, inline, message_ref, message_text, pending_prompt, question_body, question_text,
    PendingPrompt,
};
use super::{
    stored_to_wire, ChatHost, OrxDisplay, OrxKind, OrxSource, TurnGuard, TurnSubmission,
    ORX_MESSAGE_PREFIX,
};
use crate::error::{bail, Result};
use crate::store::{ReadyChatSubscription, RunWakeup, Store, StoredRun, ToldEvent};

/// How much of a message the user wrote in a subscribed chat its news quotes.
const USER_PREVIEW_CHARS: usize = 300;

pub enum News {
    Answer(ReadyChatSubscription),
    Question {
        subscriber: String,
        target: String,
        prompt: Box<PendingPrompt>,
    },
    /// A run the chat asked to be woken for ended.
    Run(Box<RunWakeup>),
    /// orx lost sight of a run the chat waits on; told once per outage.
    Unmonitored(Box<RunWakeup>),
}

impl News {
    pub fn subscriber(&self) -> &str {
        match self {
            News::Answer(ready) => &ready.subscriber_session_id,
            News::Question { subscriber, .. } => subscriber,
            News::Run(wakeup) | News::Unmonitored(wakeup) => &wakeup.chat_session_id,
        }
    }

    /// The chat or run the news is about.
    pub fn target(&self) -> &str {
        match self {
            News::Answer(ready) => &ready.target_session_id,
            News::Question { target, .. } => target,
            News::Run(wakeup) | News::Unmonitored(wakeup) => &wakeup.run.id,
        }
    }

    /// Lease the news for telling, unless it was told already or another
    /// sweep holds it.
    pub fn claim(&self, store: &Store) -> Result<Option<String>> {
        match self {
            News::Answer(ready) => store.claim_chat_subscription(
                self.subscriber(),
                self.target(),
                ToldEvent::Turn(&ready.turn_id),
            ),
            News::Question { prompt, .. } => store.claim_chat_subscription(
                self.subscriber(),
                self.target(),
                ToldEvent::Prompt(&prompt.id),
            ),
            News::Run(wakeup) => store.claim_run_wakeup(&wakeup.run.id, &wakeup.chat_session_id),
            News::Unmonitored(wakeup) => Ok(store
                .set_run_wakeup_monitoring_alerted(&wakeup.run.id, &wakeup.chat_session_id, true)?
                .then(String::new)),
        }
    }

    /// Release a claim. `told` records the news as told; without it the news
    /// is offered again on a later sweep.
    pub fn settle(&self, store: &Store, token: &str, told: bool) -> Result<()> {
        match self {
            News::Answer(ready) => {
                let event = ToldEvent::Turn(&ready.turn_id);
                let told = told.then_some(event);
                store.settle_chat_subscription(self.subscriber(), self.target(), token, told)?;
            }
            News::Question { prompt, .. } => {
                let event = ToldEvent::Prompt(&prompt.id);
                let told = told.then_some(event);
                store.settle_chat_subscription(self.subscriber(), self.target(), token, told)?;
            }
            News::Run(wakeup) if told => {
                if !store.mark_run_wakeup_delivered(
                    &wakeup.run.id,
                    &wakeup.chat_session_id,
                    token,
                )? {
                    bail!("run wake-up claim expired before delivery was recorded");
                }
            }
            News::Run(wakeup) => {
                store.release_run_wakeup(&wakeup.run.id, &wakeup.chat_session_id, token)?
            }
            News::Unmonitored(_) if told => {}
            News::Unmonitored(wakeup) => {
                store.set_run_wakeup_monitoring_alerted(
                    &wakeup.run.id,
                    &wakeup.chat_session_id,
                    false,
                )?;
            }
        }
        Ok(())
    }
}

/// Everything subscriptions owe, or owe one subscriber. A chat waiting on a
/// question owes the question, not the turn that ended asking it.
pub fn chat_news(store: &Store, subscriber: Option<&str>) -> Result<Vec<News>> {
    let mut prompts: BTreeMap<String, Option<PendingPrompt>> = BTreeMap::new();
    let mut pending = |target: &str| -> Result<Option<PendingPrompt>> {
        if !prompts.contains_key(target) {
            prompts.insert(target.to_string(), pending_prompt(store, target)?);
        }
        Ok(prompts[target].clone())
    };
    let mut news = Vec::new();
    for sub in store.list_chat_subscriptions(subscriber)? {
        if sub.paused || !sub.outstanding {
            continue;
        }
        let Some(prompt) =
            pending(&sub.target_session_id)?.filter(|pending| pending.kind() == "question")
        else {
            continue;
        };
        if sub.told_prompt_id.as_deref() != Some(prompt.id.as_str()) {
            news.push(News::Question {
                subscriber: sub.subscriber_session_id,
                target: sub.target_session_id,
                prompt: Box::new(prompt),
            });
        }
    }
    for ready in store.list_ready_chat_subscriptions()? {
        if subscriber.is_some_and(|id| id != ready.subscriber_session_id) {
            continue;
        }
        if echoes_its_subscriber(store, &ready)? {
            mark_told(
                store,
                &ready.subscriber_session_id,
                &ready.target_session_id,
            )?;
            continue;
        }
        let asking =
            pending(&ready.target_session_id)?.is_some_and(|pending| pending.kind() == "question");
        if !asking {
            news.push(News::Answer(ready));
        }
    }
    Ok(news)
}

/// Whether the target's turn only answered being told the subscriber's own
/// news. That is no news to the subscriber, and telling it would wake two
/// chats subscribed to each other in turn forever.
fn echoes_its_subscriber(store: &Store, ready: &ReadyChatSubscription) -> Result<bool> {
    let Some(turn) = store.get_chat_turn(&ready.target_session_id, &ready.turn_id)? else {
        return Ok(false);
    };
    let Some(message) = turn
        .user_message_id
        .map(|id| store.get_chat_message(&id))
        .transpose()?
        .flatten()
    else {
        return Ok(false);
    };
    let parts = stored_to_wire(&message).parts;
    Ok(!parts.is_empty()
        && parts.iter().all(|part| {
            part.orx.as_ref().is_some_and(|orx| {
                matches!(orx.kind, OrxKind::Answer | OrxKind::Question)
                    && orx.from.as_deref() == Some(ready.subscriber_session_id.as_str())
            })
        }))
}

/// Runs that ended, and runs orx can no longer see, for the chats waiting on
/// them. A run that is seen again re-arms its outage alert.
fn run_news(store: &Store) -> Result<Vec<News>> {
    store.prune_run_wakeups()?;
    let mut news: Vec<News> = store
        .list_ready_run_wakeups()?
        .into_iter()
        .filter(|wakeup| wakeup.state == "pending" && run_wakeup_text(&wakeup.run).is_some())
        .map(|wakeup| News::Run(Box::new(wakeup)))
        .collect();
    for wakeup in store.list_active_run_wakeups()? {
        let unmonitored = crate::jobs::BackendDescriptor::parse(&wakeup.run.backend_json)
            .is_ok_and(|descriptor| descriptor.monitoring_error.is_some());
        if unmonitored && !wakeup.monitoring_alerted {
            news.push(News::Unmonitored(Box::new(wakeup)));
        } else if !unmonitored && wakeup.monitoring_alerted {
            store.set_run_wakeup_monitoring_alerted(
                &wakeup.run.id,
                &wakeup.chat_session_id,
                false,
            )?;
        }
    }
    Ok(news)
}

/// What a chat is told about its news, each piece as its agent reads it and
/// as the transcript shows it, with one for every run orx lost sight of.
pub fn messages<'a>(
    store: &Store,
    news: impl IntoIterator<Item = &'a News>,
) -> Result<Vec<(String, OrxDisplay)>> {
    let mut messages = Vec::new();
    let mut unmonitored = Vec::new();
    for item in news {
        let model = || -> Result<String> {
            Ok(store
                .get_chat_session(item.target())?
                .and_then(|session| session.model)
                .unwrap_or_default())
        };
        match item {
            News::Question { target, prompt, .. } => messages.push((
                question_text(target, &model()?, prompt),
                OrxDisplay {
                    text: question_body(&prompt.prompt),
                    source: source(OrxKind::Question, Some(target), None),
                },
            )),
            News::Answer(ready) => messages.push(answer_text(store, ready, &model()?)?),
            News::Run(wakeup) => messages.extend(run_wakeup_text(&wakeup.run).map(|text| {
                let shown = text.trim_start_matches(ORX_MESSAGE_PREFIX).to_string();
                let run = &wakeup.run;
                let source = source(OrxKind::Run, Some(&run.id), Some(&run.status));
                (
                    text,
                    OrxDisplay {
                        text: shown,
                        source,
                    },
                )
            })),
            News::Unmonitored(wakeup) => unmonitored.push(wakeup.run.clone()),
        }
    }
    if !unmonitored.is_empty() {
        messages.push((
            run_monitoring_text(&unmonitored),
            OrxDisplay {
                text: monitoring_lines(&unmonitored),
                source: source(OrxKind::Monitoring, None, None),
            },
        ));
    }
    Ok(messages)
}

fn source(kind: OrxKind, from: Option<&str>, status: Option<&str>) -> OrxSource {
    OrxSource {
        kind,
        from: from.map(str::to_string),
        status: status.map(str::to_string),
    }
}

/// The answer leads, right after the line naming its chat; the transcript
/// shows the answer, or the error when there is none.
fn answer_text(
    store: &Store,
    ready: &ReadyChatSubscription,
    model: &str,
) -> Result<(String, OrxDisplay)> {
    let id = &ready.target_session_id;
    let turn = store.get_chat_turn(id, &ready.turn_id)?;
    let answer = answer(store, &ready.assistant_message_id)?;
    let mut text = format!(
        "{ORX_MESSAGE_PREFIX}Chat `{id}` ({model}) finished its turn: **{}**.",
        ready.turn_state
    );
    if !answer.is_empty() {
        text.push_str(&format!(
            " Its answer is {}:\n\n{}\n",
            message_ref(&ready.assistant_message_id),
            inline(&answer)
        ));
    }
    let error = turn
        .as_ref()
        .and_then(|turn| turn.error_message.clone())
        .filter(|_| ready.turn_state != "completed");
    if let Some(error) = &error {
        text.push_str(&format!("\nError: {error}"));
    }
    let user_message = turn
        .and_then(|turn| turn.user_message_id)
        .map(|id| store.get_chat_message(&id))
        .transpose()?
        .flatten();
    if let Some(message) = user_message {
        let wrote = message_text(&message);
        let from_agent = stored_to_wire(&message)
            .parts
            .iter()
            .any(|part| part.orx.is_some());
        if !from_agent && !wrote.is_empty() {
            let preview: String = wrote.chars().take(USER_PREVIEW_CHARS).collect();
            text.push_str(&format!(
                "\nThe user wrote this turn's message there {}: {preview}",
                message_ref(&message.id)
            ));
        }
    }
    let untold = store.untold_chat_answers(ready)?;
    if !untold.is_empty() {
        let turns = if untold.len() == 1 { "turn" } else { "turns" };
        text.push_str(&format!(
            "\n\nBefore it, the chat finished {} more {turns} you were not told of. Their answers, oldest first:",
            untold.len()
        ));
        for id in &untold {
            let earlier = super::delegation::answer(store, id)?;
            if !earlier.is_empty() {
                text.push_str(&format!("\n\n{}:\n\n{}", message_ref(id), inline(&earlier)));
            }
        }
        text.push('\n');
    }
    if let Some(pending) = pending_prompt(store, id)? {
        let tool = pending.prompt.tool.as_deref().unwrap_or("a step");
        text.push_str(&format!(
            "\nIt waits for the user to approve {tool} in that chat."
        ));
    }
    let shown = OrxDisplay {
        text: if answer.is_empty() {
            error.unwrap_or_default()
        } else {
            inline(&answer)
        },
        source: source(OrxKind::Answer, Some(id), Some(&ready.turn_state)),
    };
    Ok((text.trim_end().to_string(), shown))
}

pub(super) fn run_wakeup_text(run: &StoredRun) -> Option<String> {
    if run.status == "lost" {
        return Some(format!(
            "{ORX_MESSAGE_PREFIX}Run `{}` ended with **Outcome unavailable**. Its final outcome could not be \
             collected. Inspect any partial logs using `orx logs {}`; do not assume success or failure.",
            run.id, run.id
        ));
    }
    matches!(run.status.as_str(), "done" | "failed").then(|| {
        format!(
            "{ORX_MESSAGE_PREFIX}Run `{}` finished with status **{}**. You can compare this result with other \
             project runs using `orx runs {}` and inspect the file located by `orx logs {}`.",
            run.id, run.status, run.project_id, run.id
        )
    })
}

// Only local identifiers here: the error text includes remote ssh stderr, so the agent reads it
// as command output instead of inside an orx message.
pub(super) fn run_monitoring_text(runs: &[StoredRun]) -> String {
    let lines = monitoring_lines(runs);
    format!(
        "{ORX_MESSAGE_PREFIX}orx can no longer monitor these live runs:\n{lines}\nRun `orx exp status <expId>` \
         for the reason and tell the user so they can fix it. orx wakes you once it can see a run \
         finish; until then each run keeps its current status."
    )
}

fn monitoring_lines(runs: &[StoredRun]) -> String {
    runs.iter()
        .map(|run| {
            format!(
                "- run `{}` of experiment `{}` (still **{}**)",
                run.id, run.experiment_id, run.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Record `target`'s latest turn as told to `subscriber`, as when it stops
/// the target itself: the stop is no news to it.
pub fn mark_told(store: &Store, subscriber: &str, target: &str) -> Result<()> {
    let Some(turn) = store.latest_chat_turn(target)? else {
        return Ok(());
    };
    let event = ToldEvent::Turn(&turn.id);
    if let Some(token) = store.claim_chat_subscription(subscriber, target, event)? {
        store.settle_chat_subscription(subscriber, target, &token, Some(event))?;
    }
    Ok(())
}

/// Starts each idle chat's next turn with all the news it is owed. A busy
/// chat keeps its news until it is idle. Nothing is told while the data
/// directory moves.
pub(super) async fn deliver_news(
    chat: &Arc<ChatHost>,
    store: Store,
    data_dir_move_in_progress: Option<&AtomicBool>,
) -> Result<()> {
    let moving = || data_dir_move_in_progress.is_some_and(|flag| flag.load(Ordering::SeqCst));
    if moving() {
        return Ok(());
    }
    let mut by_chat: BTreeMap<String, Vec<News>> = BTreeMap::new();
    for item in run_news(&store)?
        .into_iter()
        .chain(chat_news(&store, None)?)
    {
        // A turn's end races the host's own bookkeeping; tell it once quiet.
        if matches!(item, News::Answer(_)) && chat.is_busy(item.target()).await {
            continue;
        }
        by_chat
            .entry(item.subscriber().to_string())
            .or_default()
            .push(item);
    }
    for (session_id, items) in by_chat {
        let Some(mut guard) = TurnGuard::claim_idle(chat, &session_id).await else {
            continue;
        };
        if moving() {
            guard.release().await;
            return Ok(());
        }
        let mut claimed = Vec::new();
        for item in items {
            if let Some(token) = item.claim(&store)? {
                claimed.push((item, token));
            }
        }
        if claimed.is_empty() {
            guard.release().await;
            continue;
        }
        let pieces = match messages(&store, claimed.iter().map(|(item, _)| item)) {
            Ok(pieces) => pieces,
            Err(error) => {
                guard.release().await;
                for (item, token) in &claimed {
                    item.settle(&store, token, false)?;
                }
                return Err(error);
            }
        };
        let submitted = chat.send_orx_messages(&session_id, pieces, guard).await;
        let told = matches!(submitted, Ok(TurnSubmission::Started(_)));
        for (item, token) in &claimed {
            item.settle(&store, token, told)?;
        }
        if let Err(error) = submitted {
            if !chat.is_busy(&session_id).await {
                eprintln!("orx up: news for {session_id}: {error}");
            }
        }
    }
    Ok(())
}
