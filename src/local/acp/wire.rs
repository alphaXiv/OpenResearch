use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{Notify, Semaphore};

use crate::error::Result;
use crate::keep::{Kept, KeptHandle};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct State {
    pub outbound: HashMap<String, String>,
    pub inbound: HashMap<String, Value>,
    pub answered: HashMap<String, Value>,
    pub prompt_result: Option<Value>,
}

pub(crate) struct Wire {
    pub keeper: KeptHandle,
    pub state: Mutex<State>,
    pub offset: std::sync::atomic::AtomicU64,
    pub owner: Mutex<Value>,
    io: Arc<super::host::SessionIo>,
    pub(super) gate: Semaphore,
    waiting: std::sync::atomic::AtomicBool,
    pub durable: std::sync::atomic::AtomicBool,
    pub response: Notify,
    checkpoint: Notify,
    /// The newest stdout that was not a message, for a failure's diagnostics.
    pub discarded: Mutex<String>,
}

impl Wire {
    pub fn persist(&self) -> Result<()> {
        let mut owner = self.owner.lock().unwrap();
        owner["state"]["rpc"] = serde_json::to_value(&*self.state.lock().unwrap())?;
        self.keeper.set_owner(&owner)
    }

    pub fn advance(&self) {
        if self
            .waiting
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.gate.add_permits(1);
        }
    }

    pub fn processed(&self) {
        self.waiting
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if !self.durable.load(std::sync::atomic::Ordering::SeqCst)
            || !self.io.emit(super::host::Event::Checkpoint)
        {
            self.advance();
        }
    }

    pub async fn resumed_response(&self) -> Result<Value> {
        loop {
            let notified = self.response.notified();
            if let Some(value) = self.state.lock().unwrap().prompt_result.clone() {
                if let Some(error) = value.get("error") {
                    let error: agent_client_protocol::Error =
                        serde_json::from_value(error.clone())?;
                    return Err(error.into());
                }
                return Ok(value["result"].clone());
            }
            notified.await;
        }
    }
}

pub(super) fn connect(
    mut kept: Kept,
    owner: Value,
    io: Arc<super::host::SessionIo>,
    resume_at: Option<u64>,
) -> Result<(
    Arc<Wire>,
    tokio::io::DuplexStream,
    tokio::sync::oneshot::Receiver<()>,
)> {
    let keeper = kept.handle();
    let mut lines = kept.take_stdout().expect("keeper stdout").lines();
    if let Some(offset) = resume_at {
        lines.skip_to(offset)?;
        keeper.ack(offset);
    }
    let state: State = owner["state"]
        .get("rpc")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let wire = Arc::new(Wire {
        offset: std::sync::atomic::AtomicU64::new(resume_at.unwrap_or(keeper.start())),
        keeper,
        state: Mutex::new(state),
        owner: Mutex::new(owner),
        io,
        gate: Semaphore::new(usize::from(resume_at.is_none())),
        waiting: std::sync::atomic::AtomicBool::new(resume_at.is_some()),
        durable: std::sync::atomic::AtomicBool::new(false),
        response: Notify::new(),
        checkpoint: Notify::new(),
        discarded: Mutex::new(String::new()),
    });
    let (library, bridge) = tokio::io::duplex(1024 * 1024);
    let (reader, mut writer) = tokio::io::split(bridge);
    let stdin = Arc::new(tokio::sync::Mutex::new(
        kept.take_stdin().expect("keeper stdin"),
    ));
    let replay_stdin = stdin.clone();
    let outgoing = wire.clone();
    let base = wire.owner.lock().unwrap()["state"]["id_base"]
        .as_u64()
        .ok_or_else(|| crate::error::anyhow!("ACP keeper has no request id base"))?
        .checked_add(1)
        .ok_or_else(|| crate::error::anyhow!("ACP request id base is exhausted"))?;
    wire.owner.lock().unwrap()["state"]["id_base"] = Value::from(base);
    wire.persist()?;
    let ids = Arc::new(Mutex::new(HashMap::<String, Value>::new()));
    let incoming_ids = ids.clone();
    tokio::spawn(async move {
        let result: Result<()> = async {
            let mut messages = tokio::io::BufReader::new(reader).lines();
            while let Some(line) = messages.next_line().await? {
                let mut value: Value = serde_json::from_str(&line)?;
                let mut answer = false;
                let mut answered_id = None;
                if let Some(id) = value.get("id").cloned() {
                    if value.get("method").is_some() {
                        let remapped = format!("{base}:{id}");
                        ids.lock().unwrap().insert(remapped.clone(), id);
                        value["id"] = Value::String(remapped.clone());
                        outgoing.state.lock().unwrap().outbound.insert(
                            remapped,
                            value["method"]
                                .as_str()
                                .expect("library request method")
                                .to_owned(),
                        );
                    } else {
                        let key = id.to_string();
                        let mut state = outgoing.state.lock().unwrap();
                        if let Some(request) = state.inbound.get(&key).cloned() {
                            answered_id = Some((key, request["_orxOffset"].clone()));
                            answer = !matches!(
                                request["method"].as_str(),
                                Some("session/request_permission" | "terminal/wait_for_exit")
                            );
                            state
                                .answered
                                .insert(request["_orxOffset"].to_string(), value.clone());
                        }
                    }
                    outgoing.persist()?;
                }
                let mut bytes = serde_json::to_vec(&value)?;
                bytes.push(b'\n');
                {
                    let mut stdin = stdin.lock().await;
                    stdin.write_all(&bytes).await?;
                    stdin.flush().await?;
                }
                if let Some((id, offset)) = answered_id {
                    {
                        let mut state = outgoing.state.lock().unwrap();
                        if state
                            .inbound
                            .get(&id)
                            .is_some_and(|request| request["_orxOffset"] == offset)
                        {
                            state.inbound.remove(&id);
                        }
                    }
                    outgoing.persist()?;
                    if answer {
                        outgoing.processed();
                    } else {
                        outgoing.checkpoint.notify_one();
                    }
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            eprintln!(
                "orx up: ACP keeper {} input failed: {error}",
                outgoing.keeper.id()
            );
            outgoing.gate.close();
            outgoing
                .keeper
                .stop(std::time::Duration::from_secs(5))
                .await;
        }
    });
    let incoming = wire.clone();
    let (ended, receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result: Result<()> = async {
            let mut injected = None;
            loop {
                let Ok(permit) = incoming.gate.acquire().await else {
                    break;
                };
                permit.forget();
                let injected = injected.get_or_insert_with(|| {
                    incoming
                        .state
                        .lock()
                        .unwrap()
                        .inbound
                        .values()
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                });
                let (mut value, offset) = if let Some(value) = injected.next() {
                    (
                        value,
                        incoming.offset.load(std::sync::atomic::Ordering::SeqCst),
                    )
                } else {
                    let next = tokio::select! {
                        biased;
                        _ = incoming.checkpoint.notified() => {
                            incoming.processed();
                            continue;
                        }
                        next = next_message(&mut lines, &incoming.discarded) => next?,
                    };
                    let Some(next) = next else {
                        break;
                    };
                    next
                };
                incoming
                    .offset
                    .store(offset, std::sync::atomic::Ordering::SeqCst);
                if let Some(id) = value.get("id").cloned() {
                    if value.get("method").is_some() {
                        let key = id.to_string();
                        let request_offset = value
                            .as_object_mut()
                            .and_then(|value| value.remove("_orxOffset"))
                            .unwrap_or_else(|| Value::from(offset));
                        let answered = incoming
                            .state
                            .lock()
                            .unwrap()
                            .answered
                            .get(&request_offset.to_string())
                            .cloned();
                        if let Some(answer) = answered {
                            // Replayed requests reuse their durable answer without repeating the tool.
                            {
                                let mut stdin = replay_stdin.lock().await;
                                stdin.write_all(format!("{answer}\n").as_bytes()).await?;
                                stdin.flush().await?;
                            }
                            incoming.state.lock().unwrap().inbound.remove(&key);
                            incoming.processed();
                            continue;
                        }
                        let mut pending = value.clone();
                        pending["_orxOffset"] = request_offset;
                        incoming.state.lock().unwrap().inbound.insert(key, pending);
                    } else {
                        let key = id
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| id.to_string());
                        let pending = incoming.state.lock().unwrap().outbound.remove(&key);
                        if pending
                            .as_ref()
                            .is_some_and(|method| method == "session/prompt")
                        {
                            incoming.state.lock().unwrap().prompt_result = Some(value.clone());
                            incoming.response.notify_one();
                        }
                        if let Some(original) = incoming_ids.lock().unwrap().remove(&key) {
                            value["id"] = original;
                        } else {
                            incoming.processed();
                            continue;
                        }
                        writer.write_all(format!("{value}\n").as_bytes()).await?;
                        incoming.processed();
                        continue;
                    }
                }
                writer.write_all(format!("{value}\n").as_bytes()).await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            eprintln!(
                "orx up: ACP keeper {} output failed: {error}",
                incoming.keeper.id()
            );
        }
        let _ = ended.send(());
    });
    Ok((wire, library, receiver))
}

/// The agent's next JSON-RPC message and the offset after it. The SDK answers every line that
/// is not JSON with a parse error, and some agents exit on it, so other lines are set aside.
async fn next_message(
    lines: &mut crate::keep::KeptLines,
    discarded: &Mutex<String>,
) -> Result<Option<(Value, u64)>> {
    while let Some((mut line, offset)) = lines.next_line().await? {
        // Windows PowerShell starts a redirected stream with a byte-order mark.
        if line.starts_with('\u{feff}') {
            line.drain(..'\u{feff}'.len_utf8());
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str(&line) {
            return Ok(Some((value, offset)));
        }
        let mut tail = discarded.lock().unwrap();
        tail.push_str(&line);
        tail.push('\n');
        let mut start = tail
            .len()
            .saturating_sub(super::host::MAX_OUTPUT_TAIL_BYTES);
        while !tail.is_char_boundary(start) {
            start += 1;
        }
        tail.drain(..start);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    async fn peer(script: &str, mut state: Value) -> (Arc<Wire>, tokio::io::DuplexStream) {
        let mut command = crate::process::command(super::super::fake_agent_interpreter("python3"));
        command.args(["-u", "-c", script]);
        state["id_base"] = Value::from(0);
        let owner = crate::keep::Owner::Agent {
            harness: "acp:wire-test".into(),
            session_id: "wire-test".into(),
            state,
        }
        .to_value();
        let kept = crate::keep::spawn(crate::keep::Keep {
            command: &command,
            io: crate::keep::Io::Pipes { stderr: None },
            retain: crate::keep::LOSSLESS,
            owner: owner.clone(),
        })
        .await
        .unwrap();
        let (wire, transport, _) = connect(
            kept,
            owner,
            Arc::new(super::super::host::SessionIo::default()),
            None,
        )
        .unwrap();
        (wire, transport)
    }

    #[tokio::test]
    async fn resuming_acknowledges_a_checkpoint_whose_acknowledgement_was_lost() {
        let limit = 1024;
        let mut command = crate::process::command(super::super::fake_agent_interpreter("python3"));
        command.args(["-u", "-c", "import sys,json;sys.stdout.buffer.write(b'x'*(int(sys.argv[1])-1)+b'\\n');sys.stdout.flush();sys.stdin.readline();print(json.dumps({'jsonrpc':'2.0','method':'done','params':{}}),flush=True);sys.stdin.read()", &limit.to_string()]);
        let owner = crate::keep::Owner::Agent {
            harness: "acp:wire-test".into(),
            session_id: "lost-ack".into(),
            state: json!({"id_base":0}),
        }
        .to_value();
        let mut original = crate::keep::spawn(crate::keep::Keep {
            command: &command,
            io: crate::keep::Io::Pipes { stderr: None },
            retain: crate::keep::Retain::Unacked { limit },
            owner: owner.clone(),
        })
        .await
        .unwrap();
        let keeper = original.handle();
        let (_, offset) = original
            .take_stdout()
            .unwrap()
            .lines()
            .next_line()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(offset, limit);
        keeper.detach();
        let (wire, transport, _) = connect(
            crate::keep::attach(keeper.id()).await.unwrap(),
            owner,
            Arc::new(super::super::host::SessionIo::default()),
            Some(offset),
        )
        .unwrap();
        wire.advance();
        let (reader, mut writer) = tokio::io::split(transport);
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"release\",\"params\":{}}\n")
            .await
            .unwrap();
        let mut lines = tokio::io::BufReader::new(reader).lines();
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(message["method"], "done");
        wire.keeper.stop(std::time::Duration::from_secs(2)).await;
    }

    #[tokio::test]
    async fn remaps_request_ids_and_restores_library_ids() {
        let (wire, transport) = peer(
            r#"import sys,json
for line in sys.stdin:
 r=json.loads(line)
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{'wireId':r['id']}}),flush=True)
"#,
            json!({}),
        )
        .await;
        let (reader, mut writer) = tokio::io::split(transport);
        writer
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/prompt\",\"params\":{}}\n",
            )
            .await
            .unwrap();
        let line = tokio::io::BufReader::new(reader)
            .lines()
            .next_line()
            .await
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 1);
        assert_ne!(value["result"]["wireId"], 1);
        assert!(wire.state.lock().unwrap().outbound.is_empty());
        wire.keeper.stop(std::time::Duration::from_secs(2)).await;
    }

    #[tokio::test]
    async fn intercepts_the_predecessors_prompt_response() {
        for expected in [
            Ok(json!({"stopReason":"end_turn"})),
            Err(agent_client_protocol::Error::resource_not_found(None)),
        ] {
            let message = match &expected {
                Ok(result) => json!({"jsonrpc":"2.0","id":"old:1","result":result}),
                Err(error) => json!({"jsonrpc":"2.0","id":"old:1","error":error}),
            };
            let script = format!(
                "import sys\nprint({:?},flush=True)\nsys.stdin.read()\n",
                message.to_string()
            );
            let (wire, _transport) = peer(&script, json!({"rpc": State {
                outbound: HashMap::from([("old:1".into(), "session/prompt".into())]), ..Default::default()
            }})).await;
            let response =
                tokio::time::timeout(std::time::Duration::from_secs(5), wire.resumed_response())
                    .await
                    .unwrap();
            match expected {
                Ok(expected) => assert_eq!(response.unwrap(), expected),
                Err(expected) => assert_eq!(
                    response
                        .unwrap_err()
                        .downcast_ref::<agent_client_protocol::Error>()
                        .unwrap()
                        .code,
                    expected.code
                ),
            }
            wire.keeper.stop(std::time::Duration::from_secs(2)).await;
        }
    }

    #[tokio::test]
    async fn injects_pending_requests_and_replays_cached_answers() {
        let request = json!({"jsonrpc":"2.0","id":"file-1","method":"fs/write_text_file","params":{"content":"once"}});
        let answer = json!({"jsonrpc":"2.0","id":"file-1","result":{}});
        let mut pending = request.clone();
        pending["_orxOffset"] = json!(10);
        let (wire, transport) = peer(
            r#"import sys,json
for line in sys.stdin:
 print(json.dumps({'jsonrpc':'2.0','method':'seen','params':json.loads(line)}),flush=True)
"#,
            json!({"rpc": State {
                inbound: HashMap::from([("\"file-1\"".into(), pending)]),
                answered: HashMap::from([("10".into(), answer.clone())]),
                ..Default::default()
            }}),
        )
        .await;
        let mut lines = tokio::io::BufReader::new(transport).lines();
        let seen: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(seen["method"], "seen");
        assert_eq!(seen["params"], answer);
        wire.keeper.stop(std::time::Duration::from_secs(2)).await;
    }
}
