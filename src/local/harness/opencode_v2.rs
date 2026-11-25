//! V2 admits prompts before execution finishes. Its export holds finished steps; the event
//! stream carries the step in flight.
use std::pin::Pin;

use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt, TryStreamExt};

use serde::{Deserialize, Serialize};

use super::*;
use crate::error::Context;
use crate::keep::KeptHandle;
use crate::local::chat::Resume;
use crate::local::harness::{next_steer, SteerMessage};
use crate::local::opencode::{steer_inbox_id, AgentEndpoint};

async fn get(endpoint: &AgentEndpoint, path: &str) -> Result<Value> {
    Ok(endpoint
        .client
        .get(format!("{}{path}", endpoint.base_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn post(endpoint: &AgentEndpoint, path: &str, body: &Value) -> Result<()> {
    endpoint
        .client
        .post(format!("{}{path}", endpoint.base_url))
        .json(body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Deletes what `path` names; one already gone counts as deleted.
async fn delete(endpoint: &AgentEndpoint, path: &str) -> Result<()> {
    let response = endpoint
        .client
        .delete(format!("{}{path}", endpoint.base_url))
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::NOT_FOUND {
        response.error_for_status()?;
    }
    Ok(())
}

async fn reply_permission(endpoint: &AgentEndpoint, path: &str, decision: &str) -> Result<()> {
    let body = if endpoint.legacy_v2_api {
        json!({"reply":decision})
    } else {
        json!({"decision":decision})
    };
    post(endpoint, path, &body).await
}

fn permission_card(request: &Value, id: &str) -> WirePrompt {
    let mut input = request["metadata"]["input"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let resources = request["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    if !resources.is_empty()
        && !["command", "cmd", "filePath", "file_path", "path"]
            .iter()
            .any(|key| input.get(*key).and_then(Value::as_str).is_some())
    {
        let key = if request["action"] == "shell" {
            "command"
        } else {
            "path"
        };
        input.insert(key.into(), json!(resources));
    }
    WirePrompt {
        kind: "permission".into(),
        native_id: Some(id.into()),
        tool: request["action"].as_str().map(str::to_owned),
        tool_input: Some(Value::Object(input)),
        header: request["message"].as_str().map(str::to_owned),
        ..Default::default()
    }
}

pub(super) async fn run_turn(
    ctx: &mut TurnCtx,
    store: NativeStore,
    binary: crate::local::opencode::ResolvedBinary,
    database: PathBuf,
) -> Result<()> {
    ensure_runtime(ctx, store, binary, database).await?;
    let (endpoint, keeper) = ctx
        .host
        .opencode
        .server_for(&ctx.session_id)
        .await
        .ok_or_else(|| anyhow!("OpenCode stopped during setup"))?;
    let mut model = ctx
        .model
        .as_deref()
        .and_then(|id| id.split_once('/'))
        .map(|(provider, id)| json!({"providerID":provider,"id":id}));
    if let (Some(model), Some(variant)) = (
        model.as_mut(),
        opencode_variant(ctx.reasoning_level.as_deref()),
    ) {
        model["variant"] = json!(variant);
    }
    let native_id = if let Some(id) = &ctx.native_session_id {
        get(&endpoint, &format!("/api/session/{id}")).await?;
        id.clone()
    } else {
        let directory = crate::local::git::existing_session_worktree_path(
            &crate::store::Store::open()?,
            &ctx.project,
            &ctx.session_id,
        )?;
        let mut body =
            json!({"agent":opencode_agent(ctx.plan_mode),"location":{"directory":directory}});
        if let Some(model) = &model {
            body["model"] = model.clone();
        }
        let session: Value = endpoint
            .client
            .post(format!("{}/api/session", endpoint.base_url))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let id = session
            .pointer("/data/id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("OpenCode V2 session response has no id"))?
            .to_owned();
        ctx.persist_native_session_id(&id)?;
        id
    };
    let path = format!("/api/session/{native_id}");
    post(
        &endpoint,
        &format!("{path}/agent"),
        &json!({"agent":opencode_agent(ctx.plan_mode)}),
    )
    .await?;
    if let Some(model) = model {
        post(&endpoint, &format!("{path}/model"), &json!({"model":model})).await?;
    }
    let events = subscribe(&endpoint).await?;
    let checkpoint = Checkpoint {
        prompt_id: format!("msg_{}", uuid::Uuid::new_v4().simple()),
    };
    ctx.persist_delivery(DeliveryState::Unknown)?;
    // A transport failure can follow durable admission. Never replay this POST.
    let admission = post(
        &endpoint,
        &format!("{path}/prompt"),
        &json!({"id":checkpoint.prompt_id,"text":ctx.text}),
    )
    .await;
    if admission.is_ok() {
        ctx.persist_delivery(DeliveryState::Accepted)?;
    }
    let mut turn = Turn {
        endpoint: &endpoint,
        keeper: &keeper,
        checkpoint: &checkpoint,
        native_id: &native_id,
        path: &path,
        delivered: false,
        steers: Vec::new(),
        steps: Vec::new(),
        recorded: HashSet::new(),
        surfaced: HashSet::new(),
    };
    turn.commit(ctx)?;
    let answered = ctx.host.prompt_answered.subscribe();
    turn.reconcile(ctx).await?;
    // A rejected prompt starts no execution, so no event would ever end this wait.
    let ended = admission.is_err() && turn.idle().await?;
    turn.follow(ctx, events, answered, ended, admission).await
}

/// What a turn checkpoints beside its transcript; the export and the pending requests hold the
/// rest.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Checkpoint {
    prompt_id: String,
}

/// Goes on with a turn a previous server left running on this chat's kept server: its prompt's
/// place in the export marks which messages are the turn's, and the transcript's cards are the
/// requests already surfaced.
pub(super) async fn resume_turn(ctx: &mut TurnCtx, resume: Resume) -> Result<()> {
    let (endpoint, keeper) = ctx
        .host
        .opencode
        .server_for(&ctx.session_id)
        .await
        .filter(|(_, keeper)| keeper.id() == resume.keep_id)
        .ok_or_else(|| {
            anyhow!("The OpenCode server this turn ran on stopped while orx restarted")
        })?;
    let native_id = ctx
        .native_session_id
        .clone()
        .ok_or_else(|| anyhow!("This OpenCode turn has no session to resume"))?;
    let checkpoint: Checkpoint = serde_json::from_value(resume.state)
        .map_err(|error| anyhow!("This OpenCode turn has an unreadable checkpoint: {error}"))?;
    let path = format!("/api/session/{native_id}");
    // Subscribed before reading back, so nothing between the two is missed.
    let events = subscribe(&endpoint).await?;
    let surfaced = ctx
        .assistant
        .parts
        .iter()
        .filter_map(|part| part.prompt.as_ref()?.native_id.clone())
        .collect();
    let mut turn = Turn {
        endpoint: &endpoint,
        keeper: &keeper,
        checkpoint: &checkpoint,
        native_id: &native_id,
        path: &path,
        delivered: false,
        steers: Vec::new(),
        steps: Vec::new(),
        recorded: HashSet::new(),
        surfaced,
    };
    let answered = ctx.host.prompt_answered.subscribe();
    // The stream no longer carries the delivery a previous server may have seen.
    let (projection, _) = turn.reconcile(ctx).await?;
    turn.delivered = turn.holds_prompt(&projection)?;
    let ended = turn.idle().await?;
    turn.follow(ctx, events, answered, ended, Ok(())).await
}

const FINAL_READS: usize = 3;

/// OpenCode's event bus. Text deltas and the step in flight reach only this stream; the export
/// gains an assistant message when its step ends.
async fn subscribe(endpoint: &AgentEndpoint) -> Result<Events> {
    use tokio::io::AsyncBufReadExt;
    let response = endpoint
        .client
        .get(format!("{}/api/event", endpoint.base_url))
        .send()
        .await?
        .error_for_status()?;
    let body = response.bytes_stream().map_err(std::io::Error::other);
    let lines = tokio::io::BufReader::new(tokio_util::io::StreamReader::new(body)).lines();
    let events = futures::stream::try_unfold(lines, |mut lines| async move {
        while let Some(line) = lines.next_line().await? {
            if let Some(data) = line.strip_prefix("data:") {
                let event: Value = serde_json::from_str(data.trim())
                    .with_context(|| format!("OpenCode sent an unreadable event: {data}"))?;
                return Ok(Some((event, lines)));
            }
        }
        Ok(None)
    });
    let mut events = events.boxed().peekable();
    // Headers can precede the feed's registration; an event before then is not delivered.
    let connected = tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .map_err(|_| anyhow!("OpenCode's event stream did not confirm the connection"))?;
    match connected {
        Some(Ok(event)) if event["type"] == "server.connected" => Ok(events),
        _ => Err(anyhow!(
            "OpenCode's event stream did not begin with a connection"
        )),
    }
}

/// What an event changed in the steps in flight.
enum Folded<'a> {
    Text { part: String, delta: &'a str },
    Step(usize),
}

/// Folds this session's step events into the export's assistant message shape, so the step in
/// flight renders with the part ids the export will give it.
fn fold<'a>(steps: &mut Vec<Value>, native_id: &str, event: &'a Value) -> Option<Folded<'a>> {
    let data = &event["data"];
    if data["sessionID"].as_str() != Some(native_id) {
        return None;
    }
    let id = data["assistantMessageID"].as_str()?;
    let kind = event["type"].as_str()?.strip_prefix("session.")?;
    if kind == "step.started" {
        steps.retain(|step| step["id"] != id);
        steps.push(json!({
            "id": id, "type": "assistant", "agent": data["agent"], "model": data["model"],
            "content": [],
        }));
        return Some(Folded::Step(steps.len() - 1));
    }
    let index = steps.iter().position(|step| step["id"] == id)?;
    let content = steps[index]["content"].as_array_mut()?;
    let (block, event_kind) = kind.split_once('.')?;
    if let ("text" | "reasoning", "started") = (block, event_kind) {
        content.push(json!({"type": block, "text": ""}));
        return Some(Folded::Step(index));
    }
    if let "text" | "reasoning" = block {
        let ordinal = data["ordinal"].as_u64()? as usize;
        let (position, entry) = content
            .iter_mut()
            .enumerate()
            .filter(|(_, entry)| entry["type"] == block)
            .nth(ordinal)?;
        return match event_kind {
            "delta" => {
                let delta = data["delta"].as_str()?;
                let text = entry["text"].as_str().unwrap_or_default();
                entry["text"] = json!(format!("{text}{delta}"));
                Some(Folded::Text {
                    part: format!("{id}:{position}"),
                    delta,
                })
            }
            "ended" => {
                entry["text"] = data["text"].clone();
                Some(Folded::Step(index))
            }
            _ => None,
        };
    }
    let call = data["id"].as_str()?;
    if kind == "tool.input.started" {
        content.push(json!({
            "type": "tool", "id": call, "name": data["name"],
            "state": {"status": "streaming", "input": ""},
        }));
        return Some(Folded::Step(index));
    }
    let tool = content
        .iter_mut()
        .find(|entry| entry["type"] == "tool" && entry["id"] == call)?;
    let state = &mut tool["state"];
    match kind {
        "tool.input.delta" => {
            let input = state["input"].as_str().unwrap_or_default();
            state["input"] = json!(format!("{input}{}", data["delta"].as_str()?));
        }
        "tool.called" => *state = json!({"status": "running", "input": data["input"]}),
        "tool.progress" => state["metadata"] = data["metadata"].clone(),
        "tool.success" | "tool.failed" => {
            let input = state["input"].take();
            *state = json!({
                "status": if kind == "tool.success" { "completed" } else { "error" },
                "input": input, "content": data["content"], "metadata": data["metadata"],
            });
            if let Some(error) = data.get("error") {
                state["error"] = error.clone();
            }
        }
        _ => return None,
    }
    Some(Folded::Step(index))
}

type Events = futures::stream::Peekable<BoxStream<'static, Result<Value>>>;

enum Waited {
    Event(Option<Result<Value>>),
    Flush,
    Answer(std::result::Result<String, tokio::sync::broadcast::error::RecvError>),
    Steer(SteerMessage),
}

/// One prompt's view of its session: the export for finished steps, `steps` for those in flight.
struct Turn<'a> {
    endpoint: &'a AgentEndpoint,
    /// The server's keeper, which the turn checkpoints against.
    keeper: &'a KeptHandle,
    checkpoint: &'a Checkpoint,
    native_id: &'a str,
    path: &'a str,
    /// Whether the event stream has said OpenCode took the prompt out of its inbox to run it.
    delivered: bool,
    /// Steers posted to the session's inbox, recorded once OpenCode delivers them.
    steers: Vec<SteerMessage>,
    steps: Vec<Value>,
    recorded: HashSet<String>,
    surfaced: HashSet<String>,
}

impl Turn<'_> {
    /// Commits the transcript with the turn's checkpoint. The server's own output carries
    /// nothing a turn applies, so the checkpoint sits where this attachment's output starts.
    fn commit(&self, ctx: &mut TurnCtx) -> Result<()> {
        ctx.flush_at(self.keeper, self.keeper.start(), self.checkpoint)
    }

    /// Follows the session's events until the prompt's execution ends, then reads its outcome.
    async fn follow(
        &mut self,
        ctx: &mut TurnCtx,
        mut events: Events,
        mut answered: tokio::sync::broadcast::Receiver<String>,
        mut ended: bool,
        admission: Result<()>,
    ) -> Result<()> {
        let prompt_id = self.checkpoint.prompt_id.as_str();
        let mut failure = None;
        let mut failure_retry_after = None;
        let mut stopped = false;
        let mut stale = false;
        while !ended {
            let waited = {
                let flush_due = ctx.flush_due();
                let steering = &mut ctx.steering;
                tokio::select! {
                    event = events.next() => Waited::Event(event),
                    _ = flush_due => Waited::Flush,
                    answer = answered.recv() => Waited::Answer(answer),
                    steer = next_steer(steering) => Waited::Steer(steer),
                }
            };
            let event = match waited {
                Waited::Event(event) => event,
                Waited::Flush => {
                    self.commit(ctx)?;
                    continue;
                }
                Waited::Answer(answer) => {
                    // A local answer can activate a conditional form field; the card is resolved
                    // only after the harness replies, so adopt it before looking again.
                    if !matches!(&answer, Ok(session) if *session != ctx.session_id) {
                        self.commit(ctx)?;
                        self.reconcile(ctx).await?;
                    }
                    continue;
                }
                Waited::Steer(steer) => {
                    self.steer(ctx, steer).await;
                    continue;
                }
            };
            let Some(Ok(event)) = event else {
                events = subscribe(self.endpoint).await.map_err(|error| {
                    anyhow!("OpenCode stopped streaming this chat's events: {error}")
                })?;
                let (projection, _) = self.reconcile(ctx).await?;
                self.delivered |= self.holds_prompt(&projection)?;
                ended = self.idle().await? && !self.settle_steers(ctx).await?;
                continue;
            };
            let kind = event["type"].as_str().unwrap_or_default();
            let data = &event["data"];
            match fold(&mut self.steps, self.native_id, &event) {
                Some(Folded::Text { part, delta }) => ctx.append_part_text(&part, delta),
                Some(Folded::Step(index)) => {
                    for (part, _) in projected_parts(&self.steps[index]) {
                        ctx.upsert_part(part);
                    }
                }
                None if kind == "session.inbox.delivered" => {
                    let inbox = data["inboxID"].as_str();
                    if inbox == Some(prompt_id) {
                        self.delivered = true;
                        stale = true;
                    } else if let Some(index) = self
                        .steers
                        .iter()
                        .position(|steer| inbox == Some(steer_inbox_id(&steer.id).as_str()))
                    {
                        let steer = self.steers.remove(index);
                        ctx.record_steer(&steer);
                        stale = true;
                    }
                }
                None if data["sessionID"].as_str() == Some(self.native_id)
                    && matches!(
                        kind,
                        "session.execution.succeeded"
                            | "session.execution.failed"
                            | "session.execution.interrupted"
                    ) =>
                {
                    ended = self.finished(ctx).await? && !self.settle_steers(ctx).await?;
                    if ended {
                        failure = data.get("error").map(error_text);
                        failure_retry_after = data.get("error").and_then(error_retry_after_ms);
                        stopped = kind == "session.execution.interrupted";
                    }
                }
                // A failed step is only terminal once no retry follows, which the next event says.
                None if kind == "session.step.failed" => stale = false,
                None => {
                    stale |= event.get("durable").is_some()
                        || kind == "session.retry.scheduled"
                        || kind.starts_with("permission.")
                        || kind.starts_with("form.");
                }
            }
            if stale && Pin::new(&mut events).peek().now_or_never().is_none() {
                self.reconcile(ctx).await?;
                stale = false;
            }
            ctx.maybe_flush_at(self.keeper, self.keeper.start(), self.checkpoint);
        }
        self.steps.clear();
        for read in 0..FINAL_READS {
            // The export can trail the event that ended the execution.
            if read > 0 {
                tokio::time::sleep(crate::config::paced(Duration::from_millis(100))).await;
            }
            let (projection, answered) = self.reconcile(ctx).await?;
            let delivered = self.holds_prompt(&projection)?;
            let outcome = projection
                .pointer("/data/info/outcome")
                .and_then(Value::as_str);
            // Declining a permission or question interrupts the execution without an export outcome.
            if delivered
                && answered
                && (stopped || matches!(outcome, Some("succeeded" | "interrupted")))
            {
                ctx.persist_delivery(DeliveryState::Accepted)?;
                ctx.mark_final_text_tail();
                return Ok(());
            }
            if let (true, Some(message)) = (delivered, &failure) {
                // Failures before a step (e.g. an unavailable model) leave no assistant message.
                ctx.mark_native_retry_exhausted();
                if let Some(wait) = failure_retry_after {
                    ctx.mark_usage_reset_at(crate::store::now_ms() + wait);
                }
                ctx.mark_terminal_failure("opencode_terminal", message);
                return Err(anyhow!("OpenCode V2: {message}"));
            }
        }
        admission?;
        Err(anyhow!(
            "OpenCode V2 became idle without completing this prompt"
        ))
    }

    /// Hands `steer` to the running execution, which reads it at its next step. An inbox
    /// OpenCode refuses sends it back to orx's queue.
    async fn steer(&mut self, ctx: &mut TurnCtx, steer: SteerMessage) {
        if !ctx.host.steer_pending(&ctx.session_id, &steer.id) {
            return;
        }
        if self.endpoint.legacy_v2_api {
            ctx.host.release_steer(&ctx.session_id, &steer.id);
            return;
        }
        let inbox_id = steer_inbox_id(&steer.id);
        let body = json!({"id":inbox_id,"text":steer.text});
        // A failed reply may still have admitted it, so the turn settles it like any other.
        if let Err(error) = post(self.endpoint, &format!("{}/prompt", self.path), &body).await {
            eprintln!(
                "orx up: OpenCode may not have taken a steer for chat {}: {error:#}",
                ctx.session_id
            );
        }
        self.steers.push(steer);
    }

    /// Settles the steers posted to an execution that ended: one OpenCode delivered joins the
    /// turn, one it never read goes back to orx's queue. Returns whether a delivered steer
    /// still runs, so the turn goes on with it.
    async fn settle_steers(&mut self, ctx: &mut TurnCtx) -> Result<bool> {
        if self.steers.is_empty() {
            return Ok(false);
        }
        // Cancelled first, so the transcript read after says which ones OpenCode read.
        let mut cancelled = Vec::new();
        for steer in &self.steers {
            let path = format!("{}/inbox/{}", self.path, steer_inbox_id(&steer.id));
            cancelled.push(delete(self.endpoint, &path).await);
        }
        let projection = get(self.endpoint, &self.endpoint.v2_export_path(self.native_id)).await?;
        let mut delivered = false;
        for (steer, cancelled) in std::mem::take(&mut self.steers).into_iter().zip(cancelled) {
            let inbox_id = steer_inbox_id(&steer.id);
            let exported = messages(&projection)?
                .iter()
                .any(|message| message["id"].as_str() == Some(inbox_id.as_str()));
            if !exported && cancelled.is_ok() {
                ctx.host.release_steer(&ctx.session_id, &steer.id);
                continue;
            }
            // One still in the inbox reaches the agent with its next prompt; queueing it again
            // would send it twice.
            if let (false, Err(error)) = (exported, cancelled) {
                eprintln!(
                    "orx up: OpenCode kept steer {inbox_id} for chat {}: {error:#}",
                    ctx.session_id
                );
            }
            ctx.record_steer(&steer);
            delivered = true;
        }
        Ok(delivered && !self.idle().await?)
    }

    /// Merges the export and the steps it does not hold yet, then surfaces pending requests.
    async fn reconcile(&mut self, ctx: &mut TurnCtx) -> Result<(Value, bool)> {
        let projection = get(self.endpoint, &self.endpoint.v2_export_path(self.native_id)).await?;
        let exported = messages(&projection)?;
        self.steps.retain(|step| {
            !exported.iter().any(|message| {
                message["id"] == step["id"]
                    && message
                        .pointer("/time/completed")
                        .is_some_and(|time| !time.is_null())
            })
        });
        // The export orders steps by when they started but holds each only once it settles, so a
        // step OpenCode finalizes late, like one a stopped server left running, lands before this
        // prompt: only what follows the prompt is this turn's.
        let mut current: Vec<Value> = exported
            .iter()
            .skip_while(|message| message["id"].as_str() != Some(&self.checkpoint.prompt_id))
            .skip(1)
            .cloned()
            .collect();
        for step in &self.steps {
            match current
                .iter_mut()
                .find(|message| message["id"] == step["id"])
            {
                Some(message) => *message = step.clone(),
                None => current.push(step.clone()),
            }
        }
        let answered = merge_projection(
            ctx,
            self.endpoint,
            self.native_id,
            &current,
            &mut self.recorded,
        )
        .await?;
        if self.holds_prompt(&projection)? && ctx.delivery_state() != DeliveryState::Accepted {
            ctx.persist_delivery(DeliveryState::Accepted)?;
        }
        let surfaced = self.surfaced.len();
        prompts(ctx, self.endpoint, self.path, &mut self.surfaced).await?;
        // A card renders at once, and an answer orx gave is checkpointed after it was sent.
        if self.surfaced.len() > surfaced {
            self.commit(ctx)?;
        }
        Ok((projection, answered))
    }

    fn holds_prompt(&self, projection: &Value) -> Result<bool> {
        Ok(messages(projection)?
            .iter()
            .any(|message| message["id"].as_str() == Some(self.checkpoint.prompt_id.as_str())))
    }

    /// Whether an execution's end is this prompt's. The export can hold the prompt before the
    /// stream has delivered an earlier execution's end, so only the stream's own order, or a
    /// prompt that never left the inbox's reach, says so.
    async fn finished(&mut self, ctx: &mut TurnCtx) -> Result<bool> {
        if self.delivered {
            return Ok(true);
        }
        if self.queued().await? {
            return Ok(false);
        }
        let (projection, _) = self.reconcile(ctx).await?;
        Ok(!self.holds_prompt(&projection)?)
    }

    async fn queued(&self) -> Result<bool> {
        let inbox = get(self.endpoint, &format!("{}/inbox", self.path)).await?;
        Ok(inbox["data"]
            .as_array()
            .ok_or_else(|| anyhow!("OpenCode V2 inbox response is invalid"))?
            .iter()
            .any(|item| item["id"].as_str() == Some(self.checkpoint.prompt_id.as_str())))
    }

    async fn idle(&self) -> Result<bool> {
        let active = get(self.endpoint, "/api/session/active").await?;
        let running = active["data"]
            .as_object()
            .ok_or_else(|| anyhow!("OpenCode V2 active response is invalid"))?
            .contains_key(self.native_id);
        Ok(!running && !self.queued().await?)
    }
}

async fn merge_projection(
    ctx: &mut TurnCtx,
    endpoint: &AgentEndpoint,
    native_id: &str,
    messages: &[Value],
    recorded: &mut HashSet<String>,
) -> Result<bool> {
    let mut answered = false;
    let mut latest_usage = None;
    let mut latest_retry = None;
    for message in messages.iter().filter(|m| m["type"] == "assistant") {
        if message.get("retry").is_none_or(Value::is_null)
            && message
                .pointer("/time/completed")
                .is_some_and(|value| !value.is_null())
        {
            if let Some(error) = message
                .get("error")
                .filter(|error| !error.is_null() && !is_interrupted_step(error))
            {
                let message = error_text(error);
                ctx.mark_native_retry_exhausted();
                if let Some(wait) = error_retry_after_ms(error) {
                    ctx.mark_usage_reset_at(crate::store::now_ms() + wait);
                }
                ctx.mark_terminal_failure("opencode_terminal", &message);
                return Err(anyhow!("OpenCode V2: {message}"));
            }
        }
        answered |= message
            .pointer("/time/completed")
            .is_some_and(|v| !v.is_null());
        record_message_usage(ctx, message, recorded);
        if let Some(used) = opencode_used_tokens(message.get("tokens")) {
            latest_usage = Some(ContextUsage {
                used_tokens: used,
                context_window: None,
                ..Default::default()
            });
        }
        latest_retry = message.get("retry").filter(|retry| !retry.is_null());
        let mut parts = projected_parts(message);
        for (part, content) in &mut parts {
            if content["type"] != "tool" || content["name"] != "subagent" {
                continue;
            }
            let Some(child_id) = content
                .pointer("/state/metadata/sessionID")
                .and_then(Value::as_str)
            else {
                continue;
            };
            let child = get(endpoint, &endpoint.v2_export_path(child_id)).await?;
            if child.pointer("/data/info/parentID").and_then(Value::as_str) != Some(native_id) {
                return Err(anyhow!("OpenCode returned an unrelated subagent session"));
            }
            let child_messages: Vec<_> = self::messages(&child)?
                .iter()
                .filter(|m| m["type"] == "assistant")
                .collect();
            for message in &child_messages {
                record_message_usage(ctx, message, recorded);
            }
            part.children = child_messages.into_iter().flat_map(wire_parts).collect();
        }
        for (part, _) in parts {
            ctx.upsert_part_raw(part);
        }
    }
    if let Some(retry) = latest_retry {
        ctx.show_retry_status(
            "native",
            &error_text(&retry["error"]),
            retry["attempt"].as_i64().unwrap_or(1),
            None,
            retry["at"].as_i64(),
        );
    } else {
        ctx.clear_retry_status();
    }
    if let Some(usage) = latest_usage {
        ctx.report_usage(usage);
    }
    Ok(answered)
}

/// A native step's own model (a sub-agent's included) once output proves it ran, and its tokens
/// once it completes; each state is recorded once.
fn record_message_usage(ctx: &TurnCtx, message: &Value, recorded: &mut HashSet<String>) {
    let Some(id) = message["id"].as_str() else {
        return;
    };
    let completed = message
        .pointer("/time/completed")
        .is_some_and(|time| !time.is_null());
    let ran = completed
        || message
            .pointer("/time/streamed")
            .is_some_and(|time| !time.is_null())
        || message["content"]
            .as_array()
            .is_some_and(|content| !content.is_empty());
    if !ran || !recorded.insert(format!("{id}:{completed}")) {
        return;
    }
    ctx.record_native_usage(
        id,
        message.pointer("/model/id").and_then(Value::as_str),
        message.pointer("/model/providerID").and_then(Value::as_str),
        message
            .get("tokens")
            .filter(|_| completed)
            .map(opencode_native_usage)
            .unwrap_or_default(),
        opencode_cost(message).filter(|_| completed),
    );
}

fn messages(projection: &Value) -> Result<&Vec<Value>> {
    projection
        .pointer("/data/messages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("OpenCode V2 history response is invalid"))
}

/// How long the provider asked to wait before trying again, read from the same structured error
/// `error_text` reports.
fn error_retry_after_ms(error: &Value) -> Option<i64> {
    ["/data/retryAfterMs", "/retryAfterMs"]
        .into_iter()
        .find_map(|path| error.pointer(path).and_then(Value::as_i64))
        .filter(|ms| *ms >= 0)
}

fn error_text(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string())
}

fn is_interrupted_step(error: &Value) -> bool {
    error["type"] == "aborted" && error["message"] == "Step interrupted"
}

fn wire_parts(message: &Value) -> Vec<WirePart> {
    projected_parts(message)
        .into_iter()
        .map(|(part, _)| part)
        .collect()
}

fn projected_parts(message: &Value) -> Vec<(WirePart, &Value)> {
    let Some(id) = message["id"].as_str() else {
        return Vec::new();
    };
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, content)| {
            let mut part = content.clone();
            part["id"] = json!(format!("{id}:{index}"));
            if content["type"] == "tool" {
                part["tool"] = content["name"].clone();
                let state = &content["state"];
                if state["status"] == "streaming" {
                    part["state"]["status"] = json!("running");
                }
                if let Some(error) = state.get("error") {
                    part["state"]["error"] = json!(error_text(error));
                }
                if let Some(output) = state["content"].as_array() {
                    part["state"]["output"] = json!(output
                        .iter()
                        .filter_map(|item| item["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"));
                }
                if content["name"] == "question"
                    && state["status"] == "error"
                    && state["error"]["message"] == "The user dismissed this question"
                {
                    part["state"]["status"] = json!("completed");
                    part["state"]["output"] = state["error"]["message"].clone();
                    part["state"]["error"] = Value::Null;
                }
            }
            let mut wire = to_wire_part(&part)?;
            if content["name"] == "execute" {
                wire.children = content
                    .pointer("/state/metadata/toolCalls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .filter_map(|(call_index, call)| {
                        let tool = call["tool"].as_str()?.strip_prefix("orx_alphaxiv.")?;
                        to_wire_part(&json!({
                            "id":format!("{id}:{index}:alphaxiv:{call_index}"),
                            "type":"tool", "tool":format!("orx_alphaxiv_{tool}"),
                            "state":{"status":call["status"], "input":call["input"]}
                        }))
                    })
                    .collect();
            }
            Some((wire, content))
        })
        .collect()
}

const REVIEW_TIMEOUT: Duration = Duration::from_secs(30);
const REVIEW_TARGET_LIMIT: usize = 2000;

/// Who reviews Auto's requests, in the order tried: the user's setting, then the
/// chat's own model. A chat on a local model keeps its commands on this
/// machine, so it never reaches the setting's.
fn reviewers(chat: Option<&str>, configured: Option<&str>, chat_is_local: bool) -> Vec<String> {
    let configured = configured.filter(|_| !chat_is_local);
    configured
        .into_iter()
        .chain(chat.filter(|chat| Some(*chat) != configured))
        .map(str::to_owned)
        .collect()
}

/// Whether a model's provider is known to serve from another machine: the
/// server lists it with a base URL that is not on loopback. Anything unknown
/// keeps the chat's own model, so a local chat's commands stay local.
async fn serves_remotely(endpoint: &AgentEndpoint, model: &str) -> bool {
    let Some((provider, _)) = model.split_once('/') else {
        return false;
    };
    let Ok(providers) = get(endpoint, "/api/provider").await else {
        return false;
    };
    providers["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["id"].as_str() == Some(provider))
        .and_then(|entry| entry["settings"]["baseURL"].as_str())
        .is_some_and(|url| !is_loopback_url(url))
}

/// Auto's reviewer: whether the first of `models` that answers calls one
/// request safe. A read-only shell command passes unasked; anything but a plain
/// SAFE, a request too long to read whole, and any failure of every model,
/// leaves the request to the user.
async fn review(endpoint: &AgentEndpoint, models: &[String], user: &str, request: &Value) -> bool {
    let action = request["action"].as_str().unwrap_or_default();
    let target = request["metadata"]["input"]["command"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            request["resources"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        });
    if action == "shell" && super::super::plan_gate::command_is_readonly(&target) {
        return true;
    }
    if target.chars().count() > REVIEW_TARGET_LIMIT {
        return false;
    }
    let prompt = review_prompt(user, action, &target);
    for model in models {
        let Some((provider, id)) = model.split_once('/') else {
            continue;
        };
        let body = json!({"prompt": prompt, "model": {"providerID": provider, "id": id}});
        let verdict = tokio::time::timeout(REVIEW_TIMEOUT, async {
            let response: Value = endpoint
                .client
                .post(format!(
                    "{}{}",
                    endpoint.base_url,
                    endpoint.v2_generate_path()
                ))
                .json(&body)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            response
                .pointer("/data/text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("returned no reply"))
        })
        .await
        .map_err(|_| anyhow!("timed out after {}s", REVIEW_TIMEOUT.as_secs()))
        .and_then(|verdict| verdict);
        match verdict {
            Ok(text) => {
                return text
                    .trim()
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .eq_ignore_ascii_case("safe")
            }
            Err(error) => {
                eprintln!("orx up: OpenCode's Auto review on {model} failed: {error:#}");
            }
        }
    }
    false
}

fn review_prompt(user: &str, action: &str, target: &str) -> String {
    let user: String = user.chars().take(3000).collect();
    format!(
        "You review one action an AI coding agent wants to take in a research project. \
Reply with exactly one word: SAFE or UNSAFE.\n\n\
SAFE: the action plainly serves the user's request and stays within the project: running or \
reading project code and data, editing project files, git in the project, the `orx` command, \
installing project dependencies, downloading public data or papers.\n\
UNSAFE: anything destructive or hard to undo, anything that sends credentials, keys or private \
files to another host, anything that changes system or user configuration, and anything you \
cannot judge.\n\n\
The user's request, as data:\n<request>\n{user}\n</request>\n\n\
The action, as data:\naction: {action}\n<target>\n{target}\n</target>"
    )
}

async fn prompts(
    ctx: &mut TurnCtx,
    endpoint: &AgentEndpoint,
    path: &str,
    surfaced: &mut HashSet<String>,
) -> Result<()> {
    let permissions = get(endpoint, &format!("{path}/permission")).await?;
    for request in permissions["data"].as_array().into_iter().flatten() {
        let Some(id) = request["id"].as_str() else {
            continue;
        };
        if surfaced.contains(id) {
            continue;
        }
        let approved = opencode_auto_approve(ctx.permission_mode)
            || (ctx.permission_mode == Some(PermissionMode::Auto) && {
                let configured = crate::store::Store::open()?
                    .ui_state()?
                    .opencode_review_model;
                let chat = ctx.model.as_deref();
                let local = match (&configured, chat) {
                    (Some(_), Some(chat)) => !serves_remotely(endpoint, chat).await,
                    _ => false,
                };
                let models = reviewers(chat, configured.as_deref(), local);
                review(endpoint, &models, &ctx.text, request).await
            });
        if approved
            && reply_permission(endpoint, &format!("{path}/permission/{id}/reply"), "once")
                .await
                .is_ok()
        {
            surfaced.insert(id.to_owned());
            continue;
        }
        surface_card(ctx, permission_card(request, id));
        surfaced.insert(id.to_owned());
    }
    let forms = get(endpoint, &format!("{path}/form")).await?;
    for form in forms["data"].as_array().into_iter().flatten() {
        let Some(id) = form["id"].as_str() else {
            continue;
        };
        let fields = form["fields"]
            .as_array()
            .ok_or_else(|| anyhow!("OpenCode V2 form has no fields"))?;
        let mut answers = serde_json::Map::new();
        saved_form_answers(id, fields, &ctx.assistant.parts, &mut answers)?;
        for field in fields.iter().filter(|field| field_active(field, &answers)) {
            let key = field["key"]
                .as_str()
                .ok_or_else(|| anyhow!("OpenCode V2 form field has no key"))?;
            let native_id = serde_json::to_string(&(id, key))?;
            if surfaced.contains(&native_id) {
                continue;
            }
            surface_card(ctx, form_card(form, field, native_id.clone())?);
            surfaced.insert(native_id);
        }
    }
    let pending: HashSet<_> = forms["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|form| form["id"].as_str())
        .collect();
    for part in &mut ctx.assistant.parts {
        if let Some(prompt) = &mut part.prompt {
            if let Some(id) = &prompt.native_id {
                if let Ok((form, _)) = serde_json::from_str::<(String, String)>(id) {
                    if !pending.contains(form.as_str()) {
                        prompt.resolved = true;
                    }
                }
            }
        }
    }
    Ok(())
}

fn field_active(field: &Value, answers: &serde_json::Map<String, Value>) -> bool {
    field["when"]
        .as_array()
        .into_iter()
        .flatten()
        .all(|condition| {
            let Some(answer) = condition["key"].as_str().and_then(|key| answers.get(key)) else {
                return false;
            };
            let equal = answer
                .as_array()
                .map(|items| items.contains(&condition["value"]))
                .unwrap_or_else(|| answer == &condition["value"]);
            match condition["op"].as_str() {
                Some("eq") => equal,
                Some("neq") => !equal,
                _ => false,
            }
        })
}

fn form_card(form: &Value, field: &Value, native_id: String) -> Result<WirePrompt> {
    let mut options: Vec<_> = field["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            Some(WireQuestionOption {
                label: option["label"].as_str()?.into(),
                description: option["description"].as_str().map(str::to_owned),
            })
        })
        .collect();
    if field["type"] == "boolean" {
        options = ["Yes", "No"]
            .into_iter()
            .map(|label| WireQuestionOption {
                label: label.into(),
                description: None,
            })
            .collect();
    }
    let question = field["title"]
        .as_str()
        .or_else(|| form["title"].as_str())
        .unwrap_or("Answer");
    let question = if field["type"] == "external" {
        format!(
            "{question}\n\n{}",
            field["url"].as_str().unwrap_or_default()
        )
    } else {
        question.to_owned()
    };
    if field["type"] == "external" {
        options = vec![WireQuestionOption {
            label: "Done".into(),
            description: None,
        }];
    }
    Ok(WirePrompt {
        kind: "question".into(),
        native_id: Some(native_id),
        header: form["title"].as_str().map(str::to_owned),
        question: Some(match field["description"].as_str() {
            Some(description) => format!("{question}\n\n{description}"),
            None => question,
        }),
        options,
        multi_select: field["type"] == "multiselect",
        ..Default::default()
    })
}

pub(super) async fn reply(
    ctx: &ResumeCtx,
    endpoint: &AgentEndpoint,
    prompt: &WirePrompt,
    answer: &PromptAnswer,
) -> Result<()> {
    let session = ctx
        .native_session_id
        .as_deref()
        .ok_or_else(|| anyhow!("OpenCode session has no native id"))?;
    let id = prompt
        .native_id
        .as_deref()
        .ok_or_else(|| anyhow!("OpenCode prompt has no id"))?;
    let path = format!("/api/session/{session}");
    if prompt.kind == "permission" {
        return reply_permission(
            endpoint,
            &format!("{path}/permission/{id}/reply"),
            if answer.approve { "once" } else { "reject" },
        )
        .await;
    }
    let (form_id, field_key): (String, String) = serde_json::from_str(id)?;
    // A form field holds one value of its own type, with no room for a note.
    if !answer.answers.is_empty() && answer.note.as_deref().is_some_and(|n| !n.trim().is_empty()) {
        return Err(anyhow!(
            "This question takes a choice or a typed answer, not both. Remove one and submit again."
        ));
    }
    let submitted = submitted_answers(&answer.answers, answer.note.as_ref());
    if !answer.approve || submitted.is_empty() {
        if !endpoint.legacy_v2_api {
            endpoint
                .client
                .delete(format!("{}{path}/form/{form_id}", endpoint.base_url))
                .send()
                .await?
                .error_for_status()?;
            return Ok(());
        }
        return post(
            endpoint,
            &format!("{path}/form/{form_id}/cancel"),
            &json!({}),
        )
        .await;
    }
    let form = get(endpoint, &format!("{path}/form/{form_id}")).await?;
    let fields = form["data"]["fields"]
        .as_array()
        .ok_or_else(|| anyhow!("OpenCode V2 form has no fields"))?;
    let mut values = serde_json::Map::new();
    let messages = crate::store::Store::open()?.list_chat_messages(&ctx.session_id)?;
    for message in messages {
        let parts: Vec<WirePart> = serde_json::from_str(&message.parts_json)?;
        saved_form_answers(&form_id, fields, &parts, &mut values)?;
    }
    let field = fields
        .iter()
        .find(|field| field["key"].as_str() == Some(&field_key))
        .ok_or_else(|| anyhow!("OpenCode form field no longer exists"))?;
    values.insert(field_key, field_answer(field, &submitted)?);
    let active: Vec<_> = fields
        .iter()
        .filter(|field| field_active(field, &values))
        .collect();
    if active.iter().all(|field| {
        field["key"]
            .as_str()
            .is_some_and(|key| values.contains_key(key))
    }) {
        values.retain(|key, _| {
            active
                .iter()
                .any(|field| field["key"].as_str() == Some(key) && field["type"] != "external")
        });
        post(
            endpoint,
            &format!("{path}/form/{form_id}/reply"),
            &json!({"answer":values}),
        )
        .await?;
    }
    Ok(())
}

fn saved_form_answers(
    form_id: &str,
    fields: &[Value],
    parts: &[WirePart],
    answers: &mut serde_json::Map<String, Value>,
) -> Result<()> {
    for part in parts {
        let Some(prompt) = part.prompt.as_ref().filter(|prompt| prompt.resolved) else {
            continue;
        };
        let Some(native_id) = &prompt.native_id else {
            continue;
        };
        let Ok((saved_form, key)) = serde_json::from_str::<(String, String)>(native_id) else {
            continue;
        };
        if saved_form != form_id {
            continue;
        }
        let submitted = submitted_answers(&prompt.answers, prompt.note.as_ref());
        if submitted.is_empty() {
            continue;
        }
        if let Some(field) = fields
            .iter()
            .find(|field| field["key"].as_str() == Some(&key))
        {
            answers.insert(key, field_answer(field, &submitted)?);
        }
    }
    Ok(())
}

fn field_answer(field: &Value, answers: &[String]) -> Result<Value> {
    let values: Vec<_> = answers
        .iter()
        .map(|answer| {
            field["options"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|option| option["label"].as_str() == Some(answer))
                .and_then(|option| option["value"].as_str())
                .unwrap_or(answer)
                .to_owned()
        })
        .collect();
    let first = values
        .first()
        .ok_or_else(|| anyhow!("Missing form answer"))?;
    match field["type"].as_str() {
        Some("multiselect") => Ok(json!(values)),
        Some("boolean") => match first.to_ascii_lowercase().as_str() {
            "true" | "yes" => Ok(json!(true)),
            "false" | "no" => Ok(json!(false)),
            _ => Err(anyhow!("Choose Yes or No")),
        },
        Some("integer") => Ok(json!(first
            .parse::<i64>()
            .map_err(|_| anyhow!("Enter a whole number"))?)),
        Some("number") => {
            let number = first
                .parse::<f64>()
                .map_err(|_| anyhow!("Enter a number"))?;
            if !number.is_finite() {
                return Err(anyhow!("Enter a finite number"));
            }
            Ok(json!(number))
        }
        _ => Ok(json!(first)),
    }
}

fn reconnect_command(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        format!(
            "$env:OPENCODE_DB='{}'; opencode auth login",
            path.replace('\'', "''")
        )
    }
    #[cfg(not(windows))]
    {
        format!(
            "OPENCODE_DB='{}' opencode auth login",
            path.replace('\'', "'\"'\"'")
        )
    }
}

fn selectable_model(model: &Value, connected: &HashSet<&str>) -> bool {
    if model["enabled"] != true {
        return false;
    }
    // V2 scopes the catalog to available providers; anonymous Zen must also be explicitly free.
    model["providerID"] != "opencode" || connected.contains("opencode") || free_model(model)
}

/// One of OpenCode's own models at no cost: what the free-models switch governs.
fn free_model(model: &Value) -> bool {
    model["providerID"] == "opencode"
        && model["cost"].as_array().is_some_and(|tiers| {
            !tiers.is_empty()
                && tiers.iter().all(|cost| {
                    cost["input"].as_f64() == Some(0.0) && cost["output"].as_f64() == Some(0.0)
                })
        })
}

/// Unknown for an empty `capabilities.input`, which is V2's placeholder for uncatalogued models.
fn image_input(model: &Value) -> Option<bool> {
    let input = model["capabilities"]["input"].as_array()?;
    (!input.is_empty()).then(|| input.iter().any(|modality| modality == "image"))
}

pub(super) async fn detect(
    binary: crate::local::opencode::ResolvedBinary,
    mut info: HarnessInfo,
) -> HarnessInfo {
    info.configured_model = super::snapshot_config().0["model"]
        .as_str()
        .map(str::to_owned);
    let refreshed = crate::local::local_models::refresh_alias_models(&binary).await;
    let records = crate::local::local_models::providers().unwrap_or_default();
    let catalog = tokio::spawn(async move {
        let db = native_store::prepare_opencode(NativeStore::Isolated)?;
        let _lease = crate::local::opencode::prepare_database(&binary, &db).await?;
        let mut cmd = crate::process::tokio_command(&binary.path);
        crate::local::local_models::prepare_env(&mut cmd, None)?;
        cmd.env("OPENCODE_DB", _lease.path())
            .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1")
            .current_dir(
                db.parent()
                    .ok_or_else(|| anyhow!("OpenCode database has no parent"))?,
            );
        let (mut child, endpoint) = crate::local::opencode::start_server(&binary, cmd).await?;
        // A legacy V2 server can't steer, so its steers wait in orx's queue.
        let steers = !endpoint.legacy_v2_api;
        let result = discover_models(&endpoint).await;
        let _ = child.kill().await;
        result.map(|catalog| (catalog, steers))
    })
    .await;
    match &catalog {
        Ok(Ok(((models, integrations), steers))) => {
            info.supports_steering = *steers;
            let connected: HashSet<&str> = integrations["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|integration| {
                    integration["connections"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty())
                })
                .filter_map(|integration| integration["id"].as_str())
                .collect();
            let free = super::free_models_offered();
            info.models = models["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|model| selectable_model(model, &connected))
                .filter(|model| free || !free_model(model))
                .filter_map(|model| {
                    let provider = model["providerID"].as_str()?;
                    let id = model["id"].as_str()?;
                    let variants: Vec<_> = model["variants"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v["id"].as_str())
                        .collect();
                    let label = crate::local::local_models::model_label(
                        provider,
                        model["name"].as_str().unwrap_or(id),
                        &records,
                    );
                    Some(
                        ModelInfo::new(format!("{provider}/{id}"))
                            .with_label(Some(&label), None)
                            .with_reasoning(&variants)
                            .with_image_input(image_input(model)),
                    )
                })
                .collect();
            info.authenticated =
                !connected.is_empty() || crate::local::local_models::configures_remote();
            info.agent_ready = !info.models.is_empty();
            info.auth_state = if info.authenticated || info.agent_ready {
                HarnessAuthState::Ready
            } else {
                HarnessAuthState::NeedsLogin
            };
            if !info.agent_ready {
                if info.authenticated {
                    info.agent_note = Some("OpenCode V2 listed no enabled models. Check its model configuration and re-check OpenCode.".into());
                } else {
                    info.agent_note = Some(format!(
                        "Connect a provider for OpenResearch's OpenCode V2 database: `{}`",
                        reconnect_command(&native_store::opencode_db(NativeStore::Isolated))
                    ));
                }
            }
        }
        Ok(Err(error)) => info.agent_note = Some(error.to_string()),
        Err(error) => info.agent_note = Some(format!("OpenCode discovery failed: {error}")),
    }

    let integrations = match &catalog {
        Ok(Ok(((_, integrations), _))) => integrations,
        _ => &Value::Null,
    };
    let catalog = info
        .models
        .iter()
        .map(|model| super::model_provider(&model.id));
    info.found_providers = super::found_providers(catalog, integrations);
    if let Err(error) = refreshed {
        info.agent_note = Some(format!("Could not refresh provider models: {error}"));
    }
    info
}

async fn discover_models(endpoint: &AgentEndpoint) -> Result<(Value, Value)> {
    discover_models_within(
        endpoint,
        Duration::from_secs(30),
        Duration::from_millis(200),
    )
    .await
}

async fn catalog_get(endpoint: &AgentEndpoint, path: &str) -> Result<Option<Value>> {
    let response = endpoint
        .client
        .get(format!("{}{path}", endpoint.base_url))
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    let body = serde_json::from_str::<Value>(&text);
    if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
        && body.as_ref().is_ok_and(|body| {
            body["_tag"] == "ServiceUnavailableError" && body["service"] == "plugin"
        })
    {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(anyhow!(
            "OpenCode {path} returned {status}: {}",
            crate::local::harness::one_line(&text, 300)
        ));
    }
    let body = body?;
    if !body["data"].is_array() {
        return Err(anyhow!("OpenCode {path} returned an invalid catalog"));
    }
    Ok(Some(body))
}

async fn discover_models_within(
    endpoint: &AgentEndpoint,
    timeout: Duration,
    retry: Duration,
) -> Result<(Value, Value)> {
    let mut catalog = None;
    let mut plugins_pending = false;
    let deadline = tokio::time::Instant::now() + timeout;
    let result = tokio::time::timeout_at(deadline, async {
        if endpoint.legacy_v2_api {
            post(endpoint, "/api/plugin/await-activation", &json!({})).await?;
        }
        loop {
            if !endpoint.legacy_v2_api {
                // A plugin check the deadline cuts off counts as pending; an answer leaves what the
                // catalogs last said.
                let catalogs_pending = std::mem::replace(&mut plugins_pending, true);
                if !endpoint.plugins_active(deadline).await? {
                    tokio::time::sleep(retry).await;
                    continue;
                }
                plugins_pending = catalogs_pending;
            }
            let models = catalog_get(endpoint, "/api/model").await?;
            plugins_pending = models.is_none();
            if let Some(models) = models {
                let integrations = catalog_get(endpoint, "/api/integration").await?;
                plugins_pending = integrations.is_none();
                if let Some(integrations) = integrations {
                    let ready = models["data"]
                        .as_array()
                        .is_some_and(|models| !models.is_empty());
                    catalog = Some((models, integrations));
                    if endpoint.legacy_v2_api || ready {
                        return Ok::<_, anyhow::Error>(());
                    }
                }
            }
            // OpenCode 2.0.4 removed await-activation; cold catalogs can initially be empty.
            tokio::time::sleep(retry).await;
        }
    })
    .await;
    if let Ok(result) = result {
        result?;
    }
    catalog.ok_or_else(|| {
        if plugins_pending {
            anyhow!(
                "OpenCode plugins did not finish activating within {} seconds. Re-check OpenCode.",
                timeout.as_secs()
            )
        } else {
            anyhow!("OpenCode model discovery timed out")
        }
    })
}

pub(super) async fn generate(
    binary: crate::local::opencode::ResolvedBinary,
    model: Option<String>,
    prompt: String,
    timeout: Duration,
) -> Result<String> {
    let model = selected_generation_model(model.as_deref())?.to_owned();
    let (provider, id) = model
        .split_once('/')
        .filter(|(provider, id)| !provider.trim().is_empty() && !id.trim().is_empty())
        .ok_or_else(|| anyhow!("OpenCode V2 needs a model in provider/model form."))?;
    let body = json!({"prompt":prompt,"model":{"providerID":provider,"id":id}});
    // Inline, so a caller that gives up drops the server with it.
    async move {
        let db = native_store::prepare_opencode(NativeStore::Isolated)?;
        let lease = crate::local::opencode::prepare_database(&binary, &db).await?;
        let mut cmd = crate::process::tokio_command(&binary.path);
        crate::local::local_models::prepare_generation_env(&mut cmd, Some(&model))?;
        cmd.env("OPENCODE_DB", lease.path())
            .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1")
            .current_dir(std::env::temp_dir());
        let (mut child, endpoint) = crate::local::opencode::start_server(&binary, cmd).await?;
        if let Err(error) =
            crate::local::opencode::verify_generation_models(&endpoint, &model).await
        {
            let _ = child.kill().await;
            return Err(error);
        }
        let result = tokio::time::timeout(timeout, async {
            let mut cold = 0;
            let response = loop {
                let response = endpoint
                    .client
                    .post(format!(
                        "{}{}",
                        endpoint.base_url,
                        endpoint.v2_generate_path()
                    ))
                    .json(&body)
                    .send()
                    .await?;
                let status = response.status();
                if status.is_success() {
                    break response;
                }
                // The body carries the provider's reason; the URL is a throwaway local port.
                let body = response.text().await.unwrap_or_default();
                // A fresh server loads its models on its first generation, which finds none yet.
                if status == reqwest::StatusCode::NOT_FOUND
                    && body.contains("ModelNotFoundError")
                    && cold < 5
                {
                    cold += 1;
                    tokio::time::sleep(crate::config::paced(Duration::from_millis(500))).await;
                    continue;
                }
                return Err(anyhow!(
                    "{status}: {}",
                    crate::local::harness::one_line(&body, 300)
                ));
            };
            let response: Value = response.json().await?;
            response
                .pointer("/data/text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("returned no reply"))
        })
        .await
        .map_err(|_| anyhow!("timed out after {}s", timeout.as_secs()))
        .and_then(|result| result);
        let _ = child.kill().await;
        result
    }
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_question_step_is_not_a_provider_failure() {
        assert!(is_interrupted_step(&json!({
            "type":"aborted", "message":"Step interrupted"
        })));
        assert!(!is_interrupted_step(&json!({
            "type":"provider.error", "message":"Step interrupted"
        })));
        assert!(!is_interrupted_step(&json!({
            "type":"aborted", "message":"Unexpected execution error"
        })));
    }

    #[tokio::test]
    async fn auto_review_approves_only_a_plain_safe_and_never_on_failure() {
        use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let reply = Arc::new(Mutex::new(None::<String>));
        let prompt = Arc::new(Mutex::new(Value::Null));
        let app = Router::new().route(
            "/api/experimental/generate",
            post({
                let (calls, reply, prompt) = (calls.clone(), reply.clone(), prompt.clone());
                move |Json(body): Json<Value>| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    *prompt.lock().unwrap() = body;
                    let reply = reply.lock().unwrap().clone();
                    async move {
                        match reply {
                            Some(text) => Json(json!({"data":{"text":text}})).into_response(),
                            None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = AgentEndpoint {
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            client: crate::net::loopback_client().build().unwrap(),
            legacy_v2_api: false,
        };
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let push = json!({
            "action":"shell", "resources":["git push origin main"],
            "metadata":{"input":{"command":"git push origin main"}}
        });

        for (text, approved) in [
            (Some("SAFE"), true),
            (Some(" safe. "), true),
            (Some("UNSAFE"), false),
            (Some("SAFE, but only after a backup"), false),
            (None, false),
        ] {
            *reply.lock().unwrap() = text.map(str::to_owned);
            let verdict = review(
                &endpoint,
                &["cloud/selected".into()],
                "push my branch",
                &push,
            )
            .await;
            assert_eq!(verdict, approved, "{text:?}");
        }
        let body = prompt.lock().unwrap().clone();
        assert_eq!(body["model"], json!({"providerID":"cloud","id":"selected"}));
        let text = body["prompt"].as_str().unwrap();
        assert!(text.contains("push my branch") && text.contains("git push origin main"));

        let asked = calls.load(Ordering::SeqCst);
        let status = json!({"action":"shell", "metadata":{"input":{"command":"git status"}}});
        assert!(review(&endpoint, &[], "", &status).await);
        assert!(!review(&endpoint, &[], "push my branch", &push).await);
        assert_eq!(calls.load(Ordering::SeqCst), asked);
        server.abort();
    }

    #[tokio::test]
    async fn a_request_too_long_to_read_whole_is_never_approved_and_remote_means_a_known_remote_url(
    ) {
        use axum::{routing::get, routing::post, Json, Router};
        let app = Router::new()
            .route(
                "/api/experimental/generate",
                post(|| async { Json(json!({"data":{"text":"SAFE"}})) }),
            )
            .route(
                "/api/provider",
                get(|| async {
                    Json(json!({"data":[
                        {"id":"cloud","settings":{"baseURL":"https://api.example.com/v1"}},
                        {"id":"local","settings":{"baseURL":"http://127.0.0.1:1234/v1"}},
                        {"id":"bare","settings":{}}
                    ]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = AgentEndpoint {
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            client: crate::net::loopback_client().build().unwrap(),
            legacy_v2_api: false,
        };
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let long = json!({"action":"shell", "resources":[format!("make {}", "x".repeat(REVIEW_TARGET_LIMIT))]});
        assert!(!review(&endpoint, &["cloud/m".into()], "build", &long).await);
        let short = json!({"action":"shell", "resources":["make all"]});
        assert!(review(&endpoint, &["cloud/m".into()], "build", &short).await);

        assert!(serves_remotely(&endpoint, "cloud/model").await);
        for model in ["local/model", "bare/model", "unlisted/model", "nomodel"] {
            assert!(!serves_remotely(&endpoint, model).await, "{model}");
        }
        server.abort();
    }

    #[test]
    fn the_reviewer_is_the_setting_then_the_chat_and_a_local_chat_keeps_its_own() {
        let list = |chat, configured, local| reviewers(chat, configured, local);
        assert_eq!(list(Some("a/chat"), None, false), ["a/chat"]);
        assert_eq!(
            list(Some("a/chat"), Some("b/small"), false),
            ["b/small", "a/chat"]
        );
        assert_eq!(list(Some("a/chat"), Some("a/chat"), false), ["a/chat"]);
        assert_eq!(
            list(Some("orx-local-x/qwen"), Some("b/small"), true),
            ["orx-local-x/qwen"]
        );
        assert_eq!(list(None, Some("b/small"), false), ["b/small"]);
        assert!(list(None, None, false).is_empty());
    }

    #[tokio::test]
    async fn auto_review_falls_back_to_the_chats_model_when_the_setting_fails() {
        use axum::{
            extract::Json as Body, http::StatusCode, response::IntoResponse, routing::post, Json,
            Router,
        };
        let app = Router::new().route(
            "/api/experimental/generate",
            post(|Body(body): Body<Value>| async move {
                if body["model"]["providerID"] == "gone" {
                    StatusCode::BAD_GATEWAY.into_response()
                } else {
                    Json(json!({"data":{"text":"SAFE"}})).into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = AgentEndpoint {
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            client: crate::net::loopback_client().build().unwrap(),
            legacy_v2_api: false,
        };
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let push = json!({"action":"shell", "resources":["git push origin main"]});
        let models = ["gone/small".to_owned(), "cloud/chat".to_owned()];
        assert!(review(&endpoint, &models, "push", &push).await);
        assert!(!review(&endpoint, &models[..1], "push", &push).await);
        server.abort();
    }

    #[test]
    fn v2_permission_card_shows_command() {
        let request = json!({
            "action":"shell",
            "resources":["printf test-marker"],
            "metadata":null
        });
        let card = permission_card(&request, "permission");
        assert_eq!(card.tool.as_deref(), Some("shell"));
        assert_eq!(card.tool_input.unwrap()["command"], "printf test-marker");

        let request = json!({
            "action":"shell",
            "resources":["printf *"],
            "metadata":{"input":{"command":"printf exact-command"}}
        });
        assert_eq!(
            permission_card(&request, "permission").tool_input.unwrap()["command"],
            "printf exact-command"
        );
    }

    #[tokio::test]
    async fn v2_permission_replies_match_native_schema_and_scope() {
        use axum::{http::StatusCode, routing, Json, Router};
        use std::sync::{Arc, Mutex};

        for legacy in [false, true] {
            let bodies = Arc::new(Mutex::new(Vec::new()));
            let captured = bodies.clone();
            let app = Router::new().route(
                "/api/session/session/permission/permission/reply",
                routing::post(move |Json(body): Json<Value>| {
                    captured.lock().unwrap().push(body);
                    async { StatusCode::NO_CONTENT }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = AgentEndpoint {
                base_url: format!("http://{}", listener.local_addr().unwrap()),
                client: crate::net::loopback_client().build().unwrap(),
                legacy_v2_api: legacy,
            };
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let ctx = ResumeCtx {
                host: TurnCtx::test_stub().host,
                session_id: "session".into(),
                native_session_id: Some("session".into()),
            };
            let prompt = WirePrompt {
                kind: "permission".into(),
                native_id: Some("permission".into()),
                ..Default::default()
            };
            for approve in [true, false] {
                let answer = PromptAnswer {
                    session_id: "session".into(),
                    prompt_id: "permission".into(),
                    approve,
                    resume_mode: None,
                    answers: Vec::new(),
                    note: None,
                    annotations: Vec::new(),
                    from: None,
                    env: Vec::new(),
                };
                reply(&ctx, &endpoint, &prompt, &answer).await.unwrap();
            }
            server.abort();
            let key = if legacy { "reply" } else { "decision" };
            assert_eq!(
                *bodies.lock().unwrap(),
                vec![json!({key:"once"}), json!({key:"reject"})]
            );
        }
    }

    #[tokio::test]
    async fn skipped_form_cancels_despite_display_note() {
        use axum::{http::StatusCode, routing, Router};

        for legacy in [false, true] {
            let form_path = "/api/session/session/form/form";
            let app = if legacy {
                Router::new().route(
                    &format!("{form_path}/cancel"),
                    routing::post(|| async { StatusCode::NO_CONTENT }),
                )
            } else {
                Router::new().route(
                    form_path,
                    routing::delete(|| async { StatusCode::NO_CONTENT }),
                )
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = AgentEndpoint {
                base_url: format!("http://{}", listener.local_addr().unwrap()),
                client: crate::net::loopback_client().build().unwrap(),
                legacy_v2_api: legacy,
            };
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let ctx = ResumeCtx {
                host: TurnCtx::test_stub().host,
                session_id: "session".into(),
                native_session_id: Some("session".into()),
            };
            let prompt = WirePrompt {
                kind: "question".into(),
                native_id: Some(serde_json::to_string(&("form", "count")).unwrap()),
                ..Default::default()
            };
            let answer = PromptAnswer {
                session_id: "session".into(),
                prompt_id: "question".into(),
                approve: false,
                resume_mode: None,
                answers: Vec::new(),
                note: Some("Skipped".into()),
                annotations: Vec::new(),
                from: None,
                env: Vec::new(),
            };
            let result = reply(&ctx, &endpoint, &prompt, &answer).await;
            server.abort();
            result.unwrap();
        }
    }

    async fn catalog_server(
        app: axum::Router,
        legacy: bool,
    ) -> (AgentEndpoint, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = AgentEndpoint {
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            client: crate::net::loopback_client().build().unwrap(),
            legacy_v2_api: legacy,
        };
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (endpoint, server)
    }

    const RETRY: Duration = Duration::from_millis(1);

    fn plugin_pending() -> Value {
        json!({"_tag":"ServiceUnavailableError", "service":"plugin", "message":"fixture", "extra":true})
    }

    #[tokio::test]
    async fn discovery_retries_plugin_activation_on_both_catalog_routes() {
        use axum::{http::StatusCode, routing::get, routing::post, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        for legacy in [false, true] {
            for pending_path in ["/api/model", "/api/integration"] {
                let polls = Arc::new(AtomicUsize::new(0));
                let activations = Arc::new(AtomicUsize::new(0));
                let count = activations.clone();
                let mut app = Router::new().route(
                    "/api/plugin/await-activation",
                    post(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        async { Json(json!({})) }
                    }),
                );
                for path in ["/api/model", "/api/integration"] {
                    let count = polls.clone();
                    app = app.route(
                        path,
                        get(move || {
                            let count = count.clone();
                            async move {
                                if path == pending_path && count.fetch_add(1, Ordering::SeqCst) == 0
                                {
                                    (StatusCode::SERVICE_UNAVAILABLE, Json(plugin_pending()))
                                } else {
                                    (StatusCode::OK, Json(json!({"data":[{"id":path}]})))
                                }
                            }
                        }),
                    );
                }
                let (endpoint, server) = catalog_server(app, legacy).await;
                let result = discover_models_within(&endpoint, Duration::from_secs(2), RETRY).await;
                server.abort();
                let (models, integrations) = result.unwrap();
                assert_eq!(models["data"][0]["id"], "/api/model");
                assert_eq!(integrations["data"][0]["id"], "/api/integration");
                assert_eq!(polls.load(Ordering::SeqCst), 2);
                assert_eq!(activations.load(Ordering::SeqCst), usize::from(legacy));
            }
        }
    }

    #[tokio::test]
    async fn discovery_does_not_retry_other_errors_or_invalid_catalogs() {
        use axum::{http::StatusCode, routing::get, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        for (status, body, diagnostic) in [
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "{\"_tag\":\"ServiceUnavailableError\",\"service\":\"storage\"}",
                "storage",
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "{\"_tag\":\"OtherError\",\"service\":\"plugin\"}",
                "OtherError",
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "not JSON\nserver unavailable",
                "not JSON server unavailable",
            ),
            (StatusCode::UNAUTHORIZED, "not signed in", "not signed in"),
            (StatusCode::OK, "not JSON", "expected"),
            (StatusCode::OK, "{\"data\":null}", "invalid catalog"),
        ] {
            for path in ["/api/model", "/api/integration"] {
                let polls = Arc::new(AtomicUsize::new(0));
                let count = polls.clone();
                let mut app = Router::new();
                if path == "/api/integration" {
                    app = app.route("/api/model", get(|| async { "{\"data\":[]}" }));
                }
                app = app.route(
                    path,
                    get(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        async move { (status, body) }
                    }),
                );
                let (endpoint, server) = catalog_server(app, false).await;
                let result = discover_models_within(&endpoint, Duration::from_secs(1), RETRY).await;
                server.abort();
                let error = result.unwrap_err().to_string();
                assert!(error.contains(diagnostic), "{error}");
                if !status.is_success() {
                    assert!(error.contains(&status.to_string()), "{error}");
                }
                assert_eq!(polls.load(Ordering::SeqCst), 1);
            }
        }
    }

    #[tokio::test]
    async fn discovery_deadline_bounds_pending_plugins_and_stalled_requests() {
        use axum::{http::StatusCode, routing::get, Json, Router};

        for stalled in [false, true] {
            let app = Router::new().route(
                "/api/model",
                get(move || async move {
                    if stalled {
                        std::future::pending::<()>().await;
                    }
                    (StatusCode::SERVICE_UNAVAILABLE, Json(plugin_pending()))
                }),
            );
            let (endpoint, server) = catalog_server(app, false).await;
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                discover_models_within(&endpoint, Duration::from_millis(100), RETRY),
            )
            .await;
            server.abort();
            let error = result.unwrap().unwrap_err().to_string();
            if stalled {
                assert_eq!(error, "OpenCode model discovery timed out");
            } else {
                assert!(error.starts_with("OpenCode plugins did not finish activating"));
            }
        }
    }

    /// The first `/api/plugin` boots OpenCode's location; on a busy machine it answers after
    /// seconds, and only the discovery deadline may give up on it.
    #[tokio::test]
    async fn discovery_waits_for_a_slow_first_plugin_answer() {
        use axum::{routing::get, Json, Router};

        for (answer_after, budget) in [
            (Duration::from_millis(2500), Duration::from_secs(10)),
            (Duration::from_secs(60), Duration::from_millis(200)),
        ] {
            let app = Router::new()
                .route(
                    "/api/plugin",
                    get(move || async move {
                        tokio::time::sleep(answer_after).await;
                        Json(json!({"data":[{"id":"fixture"}]}))
                    }),
                )
                .route(
                    "/api/model",
                    get(|| async { Json(json!({"data":[{"id":"model"}]})) }),
                )
                .route(
                    "/api/integration",
                    get(|| async { Json(json!({"data":[]})) }),
                );
            let (endpoint, server) = catalog_server(app, false).await;
            let result = discover_models_within(&endpoint, budget, RETRY).await;
            server.abort();
            if answer_after < budget {
                assert_eq!(result.unwrap().0["data"][0]["id"], "model");
            } else {
                let error = result.unwrap_err().to_string();
                assert!(
                    error.starts_with("OpenCode plugins did not finish activating"),
                    "{error}"
                );
            }
        }
    }

    #[tokio::test]
    async fn discovery_polls_cold_catalogs_and_keeps_the_last_complete_pair() {
        use axum::{http::StatusCode, routing::get, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        for plugins_reload in [false, true] {
            let polls = Arc::new(AtomicUsize::new(0));
            let count = polls.clone();
            let seen = polls.clone();
            let reloaded = Arc::new(tokio::sync::Notify::new());
            let observed = reloaded.clone();
            let app = Router::new()
                .route(
                    "/api/model",
                    get(move || {
                        let count = count.clone();
                        let observed = observed.clone();
                        async move {
                            let poll = count.fetch_add(1, Ordering::SeqCst);
                            if poll == 2 {
                                observed.notify_one();
                            }
                            Json(if poll == 0 {
                                json!({"data":[]})
                            } else {
                                json!({"data":[{"id":"fixture"}]})
                            })
                        }
                    }),
                )
                .route(
                    "/api/integration",
                    get(move || {
                        let seen = seen.clone();
                        async move {
                            if plugins_reload && seen.load(Ordering::SeqCst) > 1 {
                                (StatusCode::SERVICE_UNAVAILABLE, Json(plugin_pending()))
                            } else {
                                (StatusCode::OK, Json(json!({"data":[{"id":"complete"}]})))
                            }
                        }
                    }),
                );
            let (endpoint, server) = catalog_server(app, false).await;
            let timeout = Duration::from_secs(30);
            let discover = discover_models_within(&endpoint, timeout, RETRY);
            tokio::pin!(discover);
            if plugins_reload {
                tokio::select! {
                    _ = reloaded.notified() => {}
                    result = &mut discover => panic!("discovery ended before its reload: {result:?}"),
                }
                tokio::time::pause();
                tokio::time::advance(timeout).await;
                tokio::time::resume();
            }
            let result = discover.await;
            server.abort();
            let (models, integrations) = result.unwrap();
            let expected = if plugins_reload {
                json!([])
            } else {
                json!([{"id":"fixture"}])
            };
            assert_eq!(models["data"], expected);
            assert_eq!(integrations["data"][0]["id"], "complete");
            assert!(polls.load(Ordering::SeqCst) >= 2);
        }
    }

    #[test]
    fn free_text_and_numeric_notes_survive_form_replies_and_reload() {
        let note = "a custom answer".to_string();
        let submitted = submitted_answers(&[], Some(&note));
        assert_eq!(
            field_answer(&json!({"type":"string"}), &submitted).unwrap(),
            json!(note)
        );
        assert!(submitted_answers(&[], Some(&"  ".to_string())).is_empty());
        let chosen = vec!["Selected option".to_string()];
        assert_eq!(
            submitted_answers(&chosen, Some(&note)),
            [chosen[0].clone(), note.clone()]
        );

        let fields = vec![
            json!({"key":"count","type":"integer"}),
            json!({"key":"text","type":"string"}),
        ];
        let make_part = |key: &str, resolved, note: Option<&str>| {
            WirePart::prompt(
                key,
                WirePrompt {
                    kind: "question".into(),
                    native_id: Some(serde_json::to_string(&("frm_test", key)).unwrap()),
                    resolved,
                    note: note.map(str::to_owned),
                    ..Default::default()
                },
            )
        };
        let parts = vec![
            make_part("count", true, Some("42")),
            make_part("text", false, Some("not submitted")),
            make_part("text", true, None),
        ];
        let mut saved = serde_json::Map::new();
        saved_form_answers("frm_test", &fields, &parts, &mut saved).unwrap();
        assert_eq!(saved.get("count"), Some(&json!(42)));
        assert!(!saved.contains_key("text"));
    }

    #[test]
    fn filtered_content_keeps_its_subagent_metadata() {
        let message = json!({"id":"msg_test","content":[{"type":"future-part"},{"type":"text","text":"working"},{"type":"tool","name":"subagent","state":{"status":"running","metadata":{"sessionID":"ses_child"}}}]});
        let parts = projected_parts(&message);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].0.id, "msg_test:2");
        assert_eq!(
            parts[1]
                .1
                .pointer("/state/metadata/sessionID")
                .and_then(Value::as_str),
            Some("ses_child")
        );
    }

    #[tokio::test]
    async fn the_step_in_flight_renders_as_the_export_later_does() {
        use axum::{routing::get, Router};
        let app = Router::new().route(
            "/api/event",
            get(|| async {
                (
                    [("content-type", "text/event-stream")],
                    include_str!("fixtures/opencode-turn-events.sse"),
                )
            }),
        );
        let (endpoint, server) = catalog_server(app, false).await;
        let export: Value =
            serde_json::from_str(include_str!("fixtures/opencode-turn-export.json")).unwrap();
        let native_id = export
            .pointer("/data/info/id")
            .and_then(Value::as_str)
            .unwrap();

        let mut events = subscribe(&endpoint).await.unwrap();
        let mut steps = Vec::new();
        let mut streamed = std::collections::HashMap::<String, String>::new();
        while let Some(event) = events.next().await {
            let event = event.unwrap();
            if let Some(Folded::Text { part, delta }) = fold(&mut steps, native_id, &event) {
                streamed.entry(part).or_default().push_str(delta);
            }
        }
        server.abort();

        let shown = |message: &Value| -> Vec<Value> {
            projected_parts(message)
                .into_iter()
                .map(|(part, _)| serde_json::to_value(part).unwrap())
                .collect()
        };
        let finished: Vec<_> = messages(&export).unwrap()[1..3]
            .iter()
            .flat_map(shown)
            .collect();
        let live: Vec<_> = steps.iter().flat_map(shown).collect();
        assert_eq!(live, finished);
        for part in finished.iter().filter(|part| part["kind"] == "text") {
            assert_eq!(part["text"], streamed[part["id"].as_str().unwrap()]);
        }
    }

    #[test]
    fn error_retry_after_ms_reads_the_provider_wait() {
        // A free-limit failure carries the provider's wait inside the error payload.
        assert_eq!(
            error_retry_after_ms(&json!({
                "type":"provider.free-limit",
                "data":{"message":"Free usage limit reached.","retryAfterMs":86_400_000}
            })),
            Some(86_400_000)
        );
        // A flat retry-after on the error itself also counts.
        assert_eq!(
            error_retry_after_ms(
                &json!({"type":"provider.rate-limit","message":"Slow down","retryAfterMs":250})
            ),
            Some(250)
        );
        // Negative or absent waits are not times.
        assert_eq!(
            error_retry_after_ms(
                &json!({"type":"provider.quota","message":"Quota exceeded","retryAfterMs":-5})
            ),
            None
        );
        assert_eq!(
            error_retry_after_ms(&json!({"name":"APIError","data":{"message":"Invalid API key"}})),
            None
        );
    }

    #[test]
    fn conditional_forms_wait_for_dependencies_and_preserve_types() {
        let field = json!({"key":"count","type":"integer","when":[{"key":"continue","op":"eq","value":true}]});
        let mut answers = serde_json::Map::new();
        assert!(!field_active(&field, &answers));
        answers.insert("continue".into(), json!(true));
        assert!(field_active(&field, &answers));
        assert_eq!(field_answer(&field, &["3".into()]).unwrap(), json!(3));
        assert!(field_answer(&field, &["3.5".into()]).is_err());
        assert_eq!(
            field_answer(&json!({"type":"boolean"}), &["Yes".into()]).unwrap(),
            json!(true)
        );
        assert!(field_answer(&json!({"type":"number"}), &["NaN".into()]).is_err());
    }
    #[test]
    fn code_mode_alphaxiv_calls_keep_inputs_without_duplicating_execute_output() {
        let parts = wire_parts(&json!({"id":"message", "content":[{
            "type":"tool", "name":"execute", "state":{
                "status":"completed", "content":[{"type":"text","text":"library output"}],
                "metadata":{"toolCalls":[
                    {"tool":"orx_alphaxiv.list_library", "status":"completed", "input":{"include_papers":true}},
                    {"tool":"other.read", "status":"completed", "input":{}},
                    {"tool":"orx_alphaxiv.get_paper_notes", "status":"completed", "input":{"paper_id_or_url":"2502.16982"}}
                ]}
            }
        }]}));
        let execute = &parts[0];
        assert_eq!(
            execute.state.as_ref().unwrap().output.as_deref(),
            Some("library output")
        );
        assert_eq!(execute.children.len(), 2);
        let library = &execute.children[0];
        assert_eq!(library.tool.as_deref(), Some("orx_alphaxiv_list_library"));
        assert_eq!(
            library.state.as_ref().unwrap().input.as_ref().unwrap()["include_papers"],
            true
        );
        assert!(library.state.as_ref().unwrap().output.is_none());
        assert_eq!(
            execute.children[1].tool.as_deref(),
            Some("orx_alphaxiv_get_paper_notes")
        );
    }

    #[test]
    fn dismissed_question_is_a_completed_interaction() {
        for (tool, message, status) in [
            ("question", "The user dismissed this question", "completed"),
            ("question", "Unexpected question failure", "error"),
            ("shell", "The user dismissed this question", "error"),
        ] {
            let parts = wire_parts(&json!({
                "id":"message",
                "content":[{"type":"tool","name":tool,"state":{
                    "status":"error","error":{"message":message}
                }}]
            }));
            let state = parts[0].state.as_ref().unwrap();
            assert_eq!(state.status, status);
            if status == "completed" {
                assert_eq!(state.output.as_deref(), Some(message));
                assert!(state.error.is_none());
            } else {
                assert_eq!(state.error.as_deref(), Some(message));
            }
        }
    }

    #[test]
    fn projected_parts_and_form_values_preserve_native_shapes() {
        let parts = wire_parts(
            &json!({"id":"msg_one","content":[{"type":"text","text":"hello"},{"type":"tool","id":"call","name":"read","state":{"status":"completed","input":{},"content":[{"type":"text","text":"file"}]}}]}),
        );
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].text.as_deref(), Some("hello"));
        assert_eq!(
            parts[1].state.as_ref().unwrap().output.as_deref(),
            Some("file")
        );
        let form = json!({"fields":[{"key":"answer","type":"string","options":[{"label":"Continue","value":"yes"}]}]});
        assert_eq!(
            field_answer(&form["fields"][0], &["Continue".into()]).unwrap(),
            json!("yes")
        );
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn anonymous_zen_requires_explicitly_free_enabled_models() {
        let mut model =
            json!({"providerID":"opencode", "enabled":true, "cost":[{"input":0,"output":0}]});
        let anonymous = HashSet::new();
        assert!(selectable_model(&model, &anonymous));
        model["cost"][0]["output"] = json!(1);
        assert!(!selectable_model(&model, &anonymous));
        assert!(selectable_model(&model, &HashSet::from(["opencode"])));
        model["cost"] = json!([]);
        assert!(!selectable_model(&model, &anonymous));
        model["providerID"] = json!("custom-local-provider");
        assert!(selectable_model(&model, &anonymous));
        model["enabled"] = json!(false);
        assert!(!selectable_model(&model, &anonymous));
    }

    #[test]
    fn image_input_reads_catalog_modalities() {
        let with = |input: Value| image_input(&json!({"capabilities":{"input":input}}));
        assert_eq!(with(json!(["text", "image"])), Some(true));
        assert_eq!(with(json!(["text"])), Some(false));
        assert_eq!(with(json!([])), None);
        assert_eq!(image_input(&json!({})), None);
    }
}

#[cfg(all(test, unix))]
mod restart_tests {
    use super::*;
    use axum::{
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use std::sync::{Arc, Mutex};

    fn permission(id: &str) -> Value {
        json!({"id":id,"action":"shell","resources":["ls"],"metadata":{"input":{}}})
    }

    #[test]
    fn a_resumed_turn_finishes_from_the_export_without_prompting_again() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let prompted = Arc::new(Mutex::new(false));
            let running = Arc::new(Mutex::new(true));
            let listed = Arc::new(tokio::sync::Notify::new());
            let (send, events) = tokio::sync::mpsc::unbounded_channel::<String>();
            let events = Arc::new(Mutex::new(Some(events)));
            let export = json!({"data":{"info":{"outcome":"succeeded"},"messages":[
                {"id":"msg_earlier","type":"assistant","time":{"completed":1},
                    "content":[{"type":"text","text":"an earlier answer"}]},
                {"id":"msg_prompt","type":"user"},
                {"id":"msg_reply","type":"assistant","time":{"completed":2},
                    "content":[{"type":"text","text":"the answer"}]}
            ]}});
            let empty = || async { Json(json!({"data":[]})) };
            let app = Router::new()
                .route(
                    "/api/event",
                    get(move || {
                        let rx = events.lock().unwrap().take().expect("one subscription");
                        let connected = r#"{"type":"server.connected"}"#.to_owned();
                        let stream = futures::stream::once(async move { connected })
                            .chain(futures::stream::unfold(rx, |mut rx| async move {
                                Some((rx.recv().await?, rx))
                            }))
                            .map(|line| Ok::<_, std::io::Error>(format!("data: {line}\n\n")));
                        async move { axum::body::Body::from_stream(stream) }
                    }),
                )
                .route(
                    "/api/experimental/session/native/export",
                    get(move || {
                        let body = export.clone();
                        async move { Json(body) }
                    }),
                )
                .route("/api/session/native/inbox", get(empty))
                .route(
                    "/api/session/native/permission",
                    get({
                        let listed = listed.clone();
                        move || {
                            listed.notify_one();
                            async {
                                Json(json!({"data":[permission("per_before"), permission("per_gap")]}))
                            }
                        }
                    }),
                )
                .route("/api/session/native/form", get(empty))
                .route(
                    "/api/session/active",
                    get({
                        let running = running.clone();
                        move || {
                            let running = *running.lock().unwrap();
                            async move {
                                Json(if running {
                                    json!({"data":{"native":{}}})
                                } else {
                                    json!({"data":{}})
                                })
                            }
                        }
                    }),
                )
                .route(
                    "/api/session/native/prompt",
                    post({
                        let prompted = prompted.clone();
                        move || {
                            *prompted.lock().unwrap() = true;
                            async { Json(json!({})) }
                        }
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let root = crate::local::git::TemporaryDirectory::new("orx-opencode-resume").unwrap();
            let mut ctx = TurnCtx::test_stub();
            ctx.host
                .opencode
                .serve_for_test(&ctx.session_id, port, root.path())
                .await;
            let (_, keeper) = ctx.host.opencode.server_for(&ctx.session_id).await.unwrap();
            // What the previous server had streamed and committed before it restarted.
            ctx.native_session_id = Some("native".into());
            ctx.upsert_part(WirePart::text("msg_reply:0", "the ans"));
            ctx.upsert_part(WirePart::prompt(
                "per_before",
                permission_card(&permission("per_before"), "per_before"),
            ));
            ctx.resume = Some(Resume {
                keep_id: keeper.id().into(),
                offset: keeper.start(),
                state: json!({"promptId": "msg_prompt"}),
            });

            let driver = async {
                listed.notified().await;
                *running.lock().unwrap() = false;
                send.send(
                    json!({"type":"session.execution.succeeded","data":{"sessionID":"native"}})
                        .to_string(),
                )
                .unwrap();
            };
            let (result, ()) = tokio::join!(super::super::run_turn(&mut ctx), driver);
            result.unwrap();

            assert!(!*prompted.lock().unwrap(), "the prompt is never sent again");
            let ids: Vec<&str> = ctx.assistant.parts.iter().map(|p| p.id.as_str()).collect();
            assert_eq!(ids, ["msg_reply:0", "per_before", "per_gap"]);
            assert_eq!(ctx.assistant.parts[0].text.as_deref(), Some("the answer"));
            let checkpoint = crate::store::Store::open()
                .unwrap()
                .chat_turn_checkpoint(&ctx.turn_id)
                .unwrap()
                .expect("the card the resumed turn surfaced is checkpointed");
            assert_eq!(checkpoint.keep_id, keeper.id());
            let state: Value = serde_json::from_str(&checkpoint.state_json).unwrap();
            assert_eq!(state["harness"]["promptId"], "msg_prompt");
        });
    }

    #[test]
    fn a_steer_joins_the_running_execution_or_goes_back_to_the_queue_once() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            // Delivered as the stream says, delivered while the stream missed it, never read, or
            // refused with an error.
            for case in ["event", "export", "unread", "refused"] {
                let delivered = matches!(case, "event" | "export");
                let steer_id = format!("q_{}", uuid::Uuid::new_v4().simple());
                let (posted_tx, mut posted) = tokio::sync::mpsc::unbounded_channel::<String>();
                let cancelled = Arc::new(Mutex::new(Vec::<String>::new()));
                let running = Arc::new(Mutex::new(true));
                let listed = Arc::new(tokio::sync::Notify::new());
                let (send, events) = tokio::sync::mpsc::unbounded_channel::<String>();
                let events = Arc::new(Mutex::new(Some(events)));
                let mut export = json!({"data":{"info":{"outcome":"succeeded"},"messages":[
                    {"id":"msg_prompt","type":"user"},
                    {"id":"msg_reply","type":"assistant","time":{"completed":2},
                        "content":[{"type":"text","text":"the answer"}]}
                ]}});
                if case == "export" {
                    export["data"]["messages"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"id": steer_inbox_id(&steer_id), "type": "user"}));
                }
                let empty = || async { Json(json!({"data":[]})) };
                let app = Router::new()
                    .route(
                        "/api/event",
                        get(move || {
                            let rx = events.lock().unwrap().take().expect("one subscription");
                            let connected = r#"{"type":"server.connected"}"#.to_owned();
                            let stream = futures::stream::once(async move { connected })
                                .chain(futures::stream::unfold(rx, |mut rx| async move {
                                    Some((rx.recv().await?, rx))
                                }))
                                .map(|line| Ok::<_, std::io::Error>(format!("data: {line}\n\n")));
                            async move { axum::body::Body::from_stream(stream) }
                        }),
                    )
                    .route(
                        "/api/experimental/session/native/export",
                        get(move || {
                            let body = export.clone();
                            async move { Json(body) }
                        }),
                    )
                    .route("/api/session/native/inbox", get(empty))
                    .route(
                        "/api/session/native/inbox/{id}",
                        axum::routing::delete({
                            let cancelled = cancelled.clone();
                            move |axum::extract::Path(id): axum::extract::Path<String>| {
                                cancelled.lock().unwrap().push(id);
                                async {}
                            }
                        }),
                    )
                    .route(
                        "/api/session/native/permission",
                        get({
                            let listed = listed.clone();
                            move || {
                                listed.notify_one();
                                async { Json(json!({"data":[]})) }
                            }
                        }),
                    )
                    .route("/api/session/native/form", get(empty))
                    .route(
                        "/api/session/active",
                        get({
                            let running = running.clone();
                            move || {
                                let running = *running.lock().unwrap();
                                async move {
                                    Json(if running {
                                        json!({"data":{"native":{}}})
                                    } else {
                                        json!({"data":{}})
                                    })
                                }
                            }
                        }),
                    )
                    .route(
                        "/api/session/native/prompt",
                        post(move |Json(body): Json<Value>| {
                            posted_tx.send(body["id"].as_str().unwrap().to_owned()).unwrap();
                            async move {
                                if case == "refused" {
                                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                                } else {
                                    Json(json!({})).into_response()
                                }
                            }
                        }),
                    );
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = listener.local_addr().unwrap().port();
                tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
                let root = crate::local::git::TemporaryDirectory::new("orx-opencode-steer").unwrap();
                let mut ctx = TurnCtx::test_stub();
                ctx.session_id = format!("steer-{case}");
                let store = crate::store::Store::open().unwrap();
                crate::local::chat::steering_tests::add_chat_session(&store, &ctx.session_id);
                ctx.host
                    .opencode
                    .serve_for_test(&ctx.session_id, port, root.path())
                    .await;
                let (_, keeper) = ctx.host.opencode.server_for(&ctx.session_id).await.unwrap();
                ctx.native_session_id = Some("native".into());
                ctx.resume = Some(Resume {
                    keep_id: keeper.id().into(),
                    offset: keeper.start(),
                    state: json!({"promptId": "msg_prompt"}),
                });
                let (steer_tx, steering) =
                    crate::local::chat::SteerReceiver::for_test(ctx.host.clone(), &ctx.session_id);
                ctx.steering = Some(steering);
                let steer = SteerMessage {
                    id: steer_id.clone(),
                    display: "use the smaller model".into(),
                    text: "use the smaller model".into(),
                    client_turn_id: None,
                    orx: None,
                    sent_at: 1,
                };
                ctx.host.queue_steer(&ctx.session_id, &steer, &store).unwrap();
                let host = ctx.host.clone();
                let session_id = ctx.session_id.clone();

                let driver = async {
                    listed.notified().await;
                    steer_tx.send(steer.clone()).unwrap();
                    let inbox_id = posted.recv().await.unwrap();
                    if case == "event" {
                        send.send(
                            json!({"type":"session.inbox.delivered","data":{"sessionID":"native","inboxID":inbox_id}})
                                .to_string(),
                        )
                        .unwrap();
                    }
                    *running.lock().unwrap() = false;
                    send.send(
                        json!({"type":"session.execution.succeeded","data":{"sessionID":"native"}})
                            .to_string(),
                    )
                    .unwrap();
                    inbox_id
                };
                let (result, inbox_id) = tokio::join!(super::super::run_turn(&mut ctx), driver);
                result.unwrap();

                let read = ctx
                    .assistant
                    .parts
                    .iter()
                    .any(|part| part.kind == "steer" && part.text.as_deref() == Some("use the smaller model"));
                assert_eq!(read, delivered, "only a delivered steer is part of the turn");
                let withdrawn = cancelled.lock().unwrap().clone();
                let still_steering = host.steer_pending(&session_id, &steer_id);
                let queued = !store
                    .list_queued_chat_messages_for_session(&session_id)
                    .unwrap()
                    .is_empty();
                if case == "event" {
                    assert!(withdrawn.is_empty(), "a steer read in the stream needs no settling");
                } else if !delivered {
                    assert_eq!(withdrawn, [inbox_id]);
                    assert!(
                        !still_steering && queued,
                        "a steer the execution never read runs next from orx's queue"
                    );
                }
            }
        });
    }

    #[test]
    fn stopping_reports_the_steers_opencode_already_read() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let read = format!("q_{}", uuid::Uuid::new_v4());
            let waiting = format!("q_{}", uuid::Uuid::new_v4());
            let cancelled = Arc::new(Mutex::new(Vec::<String>::new()));
            let export = json!({"data":{"messages":[
                {"id":"msg_prompt","type":"user"},
                {"id": steer_inbox_id(&read), "type":"user"}
            ]}});
            let inbox = json!({"data":[{"id": steer_inbox_id(&waiting)}]});
            let app = Router::new()
                .route(
                    "/api/session/native/interrupt",
                    post(|| async { Json(json!({})) }),
                )
                .route(
                    "/api/session/native/inbox",
                    get(move || {
                        let body = inbox.clone();
                        async move { Json(body) }
                    }),
                )
                .route(
                    "/api/session/native/inbox/{id}",
                    axum::routing::delete({
                        let cancelled = cancelled.clone();
                        move |axum::extract::Path(id): axum::extract::Path<String>| {
                            cancelled.lock().unwrap().push(id);
                            // A failed cancel must not hide which steers were read.
                            async { StatusCode::INTERNAL_SERVER_ERROR }
                        }
                    }),
                )
                .route(
                    "/api/experimental/session/native/export",
                    get(move || {
                        let body = export.clone();
                        async move { Json(body) }
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let root = crate::local::git::TemporaryDirectory::new("orx-opencode-stop").unwrap();
            let ctx = TurnCtx::test_stub();
            ctx.host
                .opencode
                .serve_for_test(&ctx.session_id, port, root.path())
                .await;

            let delivered = ctx
                .host
                .opencode
                .interrupt(&ctx.session_id, "native", &[read.clone(), waiting.clone()])
                .await
                .unwrap();

            assert_eq!(
                delivered,
                [read],
                "only the steer OpenCode read is reported"
            );
            assert_eq!(*cancelled.lock().unwrap(), [steer_inbox_id(&waiting)]);
        });
    }

    #[test]
    fn a_step_finalized_late_stays_with_the_turn_it_started_in() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let earlier = json!({"id":"msg_earlier","type":"assistant","time":{"completed":1},
                "content":[{"type":"text","text":"an earlier answer"}]});
            let prompt = json!({"id":"msg_prompt","type":"user"});
            let export = Arc::new(Mutex::new(json!({"data":{"info":{"outcome":"succeeded"},
                "messages":[earlier.clone(), prompt.clone()]}})));
            let running = Arc::new(Mutex::new(true));
            let read = Arc::new(tokio::sync::Notify::new());
            let (send, events) = tokio::sync::mpsc::unbounded_channel::<String>();
            let events = Arc::new(Mutex::new(Some(events)));
            let empty = || async { Json(json!({"data":[]})) };
            let app = Router::new()
                .route(
                    "/api/event",
                    get(move || {
                        let rx = events.lock().unwrap().take().expect("one subscription");
                        let connected = r#"{"type":"server.connected"}"#.to_owned();
                        let stream = futures::stream::once(async move { connected })
                            .chain(futures::stream::unfold(rx, |mut rx| async move {
                                Some((rx.recv().await?, rx))
                            }))
                            .map(|line| Ok::<_, std::io::Error>(format!("data: {line}\n\n")));
                        async move { axum::body::Body::from_stream(stream) }
                    }),
                )
                .route(
                    "/api/experimental/session/native/export",
                    get({
                        let export = export.clone();
                        let read = read.clone();
                        move || {
                            let body = export.lock().unwrap().clone();
                            read.notify_one();
                            async move { Json(body) }
                        }
                    }),
                )
                .route("/api/session/native/inbox", get(empty))
                .route("/api/session/native/permission", get(empty))
                .route("/api/session/native/form", get(empty))
                .route(
                    "/api/session/active",
                    get({
                        let running = running.clone();
                        move || {
                            let running = *running.lock().unwrap();
                            async move {
                                Json(if running {
                                    json!({"data":{"native":{}}})
                                } else {
                                    json!({"data":{}})
                                })
                            }
                        }
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let root = crate::local::git::TemporaryDirectory::new("orx-opencode-late").unwrap();
            let mut ctx = TurnCtx::test_stub();
            ctx.host
                .opencode
                .serve_for_test(&ctx.session_id, port, root.path())
                .await;
            let (_, keeper) = ctx.host.opencode.server_for(&ctx.session_id).await.unwrap();
            ctx.native_session_id = Some("native".into());
            ctx.resume = Some(Resume {
                keep_id: keeper.id().into(),
                offset: keeper.start(),
                state: json!({"promptId": "msg_prompt"}),
            });

            // A step of an earlier turn, settled only once this prompt ran, keeps its place.
            let late = json!({"id":"msg_late","type":"assistant",
                "time":{"completed":3},"error":{"type":"aborted","message":"Step interrupted"},
                "content":[{"type":"reasoning","text":"run the command"},
                    {"type":"tool","id":"call_shell","name":"shell","state":{"status":"error",
                        "input":{"command":"sleep 60"},
                        "error":{"message":"Tool execution interrupted: shell"}}}]});
            let reply = json!({"id":"msg_reply","type":"assistant","time":{"completed":4},
                "content":[{"type":"text","text":"the answer"}]});
            let driver = async {
                read.notified().await;
                export.lock().unwrap()["data"]["messages"] = json!([earlier, late, prompt, reply]);
                *running.lock().unwrap() = false;
                send.send(
                    json!({"type":"session.execution.succeeded","data":{"sessionID":"native"}})
                        .to_string(),
                )
                .unwrap();
            };
            let (result, ()) = tokio::join!(super::super::run_turn(&mut ctx), driver);
            result.unwrap();

            let ids: Vec<&str> = ctx.assistant.parts.iter().map(|p| p.id.as_str()).collect();
            assert_eq!(ids, ["msg_reply:0"]);
        });
    }

    #[test]
    fn a_turn_whose_server_died_in_the_gap_fails_for_recovery() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let mut ctx = TurnCtx::test_stub();
            ctx.native_session_id = Some("native".into());
            ctx.resume = Some(Resume {
                keep_id: "gone".into(),
                offset: 0,
                state: json!({"promptId": "msg_prompt"}),
            });
            let error = super::super::run_turn(&mut ctx).await.unwrap_err();
            assert!(
                error.to_string().contains("stopped while orx restarted"),
                "{error}"
            );
        });
    }
}
