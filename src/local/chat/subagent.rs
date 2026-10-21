use std::io::{BufRead, BufReader};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use super::{WirePart, WireToolState};
use crate::error::Result;
use crate::local::native_store;
use crate::store::StoredChatSession;

pub fn snapshot(session: &StoredChatSession, spawn: &WirePart) -> Result<Option<WirePart>> {
    if let Some(id) = spawn
        .state
        .as_ref()
        .and_then(|state| state.input.as_ref())
        .and_then(|input| input.pointer("/nativeTranscript/antigravityConversationId"))
        .and_then(Value::as_str)
    {
        return antigravity(spawn, id, session.native_session_id.as_deref()).map(Some);
    }
    let Some(child) = opencode_child(spawn) else {
        return Ok(None);
    };
    let Some(parent) = session.native_session_id.as_deref() else {
        return Ok(None);
    };
    let Some(location) = native_store::opencode_session(&child)? else {
        return Ok(Some(with_children(spawn, Vec::new(), true)));
    };
    let db = Connection::open_with_flags(location.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(std::time::Duration::from_secs(1))?;
    let v2: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='session_v2')",
        [],
        |row| row.get(0),
    )?;
    let table = if v2 { "session_v2" } else { "session" };
    let owner: Option<String> = db
        .query_row(
            &format!("SELECT parent_id FROM {table} WHERE id=?1"),
            [&child],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if owner.as_deref() != Some(parent) {
        return Ok(None);
    }
    let mut children = Vec::new();
    let mut running = true;
    if v2 {
        let mut statement = db
            .prepare("SELECT id,type,data FROM session_message WHERE session_id=?1 ORDER BY seq")?;
        let rows = statement.query_map([&child], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, kind, data) = row?;
            let message: Value = serde_json::from_str(&data)?;
            if kind == "idle" {
                running = false;
            }
            if kind != "assistant" {
                continue;
            }
            running = opencode_message_running(&message);
            if let Some(content) = message["content"].as_array() {
                for (index, item) in content.iter().enumerate() {
                    let id = format!("{}:{id}:{index}", spawn.id);
                    match item["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = item["text"].as_str() {
                                children.push(WirePart::text(id, text));
                            }
                        }
                        Some("tool") => {
                            let mut part = WirePart::tool(
                                id,
                                item["name"].as_str().unwrap_or("tool"),
                                item["state"]["status"].as_str().unwrap_or("running"),
                                None,
                            );
                            if let Some(state) = part.state.as_mut() {
                                state.input = item["state"].get("input").cloned();
                                state.output = item["state"]["content"].as_array().map(|content| {
                                    content
                                        .iter()
                                        .filter_map(|block| block["text"].as_str())
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                });
                            }
                            children.push(part);
                        }
                        _ => {}
                    }
                }
            }
        }
    } else {
        let mut statement = db.prepare("SELECT p.id,p.data FROM part p JOIN message m ON m.id=p.message_id WHERE p.session_id=?1 AND json_extract(m.data,'$.role')='assistant' ORDER BY p.time_created,p.id")?;
        let rows = statement.query_map([&child], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, data) = row?;
            let mut value: Value = serde_json::from_str(&data)?;
            value["id"] = Value::String(format!("{}:{id}", spawn.id));
            if let Some(part) = crate::local::harness::opencode::to_wire_part(&value) {
                children.push(part);
            }
        }
        let last: Option<String> = db.query_row(
            "SELECT data FROM message WHERE session_id=?1 AND json_extract(data,'$.role')='assistant' ORDER BY time_created DESC,id DESC LIMIT 1",
            [&child], |row| row.get(0),
        ).optional()?;
        running = match last {
            Some(data) => opencode_message_running(&serde_json::from_str(&data)?),
            None => true,
        };
    }
    Ok(Some(with_children(spawn, children, running)))
}

fn opencode_message_running(message: &Value) -> bool {
    message
        .pointer("/time/completed")
        .and_then(Value::as_i64)
        .is_none()
        || message["finish"] == "tool-calls"
}

fn opencode_child(spawn: &WirePart) -> Option<String> {
    let state = spawn.state.as_ref()?;
    let tool = spawn.tool.as_deref()?;
    if !matches!(tool, "subagent" | "task") && state.title.as_deref() != Some("subagent") {
        return None;
    }
    let output = state.output.as_deref()?;
    let id = output.split_once("(sessionID: ")?.1.split_once(')')?.0;
    (id.starts_with("ses_")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then(|| id.to_owned())
}

fn antigravity(spawn: &WirePart, id: &str, parent: Option<&str>) -> Result<WirePart> {
    let id = uuid::Uuid::parse_str(id)?;
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".gemini/antigravity-cli/brain")
        .join(id.to_string())
        .join(".system_generated/logs/transcript.jsonl");
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(with_children(spawn, Vec::new(), true))
        }
        Err(error) => return Err(error.into()),
    };
    let mut children = Vec::new();
    let mut running = true;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        apply_antigravity_record(spawn, &record, parent, &mut children, &mut running);
    }
    Ok(with_children(spawn, children, running))
}

fn apply_antigravity_record(
    spawn: &WirePart,
    record: &Value,
    parent: Option<&str>,
    children: &mut Vec<WirePart>,
    running: &mut bool,
) {
    let kind = record["type"].as_str().unwrap_or("");
    let status = record["status"].as_str().unwrap_or("");
    let index = record["step_index"].as_i64().unwrap_or(0);
    if kind == "PLANNER_RESPONSE" {
        if let Some(text) = record["content"].as_str().filter(|text| !text.is_empty()) {
            children.push(WirePart::text(format!("{}:{index}", spawn.id), text));
        }
        let calls = record["tool_calls"].as_array();
        *running = status != "DONE" || calls.is_some_and(|calls| !calls.is_empty());
        for (offset, call) in calls.into_iter().flatten().enumerate() {
            let name = call["name"].as_str().unwrap_or("tool");
            let mut args = call["args"].clone();
            if let Some(object) = args.as_object_mut() {
                for value in object.values_mut() {
                    if let Some(decoded) = value
                        .as_str()
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                    {
                        *value = decoded;
                    }
                }
            }
            let handback =
                name == "send_message" && parent.is_some() && args["Recipient"].as_str() == parent;
            let (tool, input) = if handback {
                (
                    "SubagentHandback",
                    Some(serde_json::json!({"message": args["Message"]})),
                )
            } else {
                crate::local::harness::antigravity::normalize_tool(name, Some(&args))
            };
            let mut part = WirePart::tool(
                format!("{}:{index}:{offset}", spawn.id),
                tool,
                "running",
                None,
            );
            if let Some(state) = part.state.as_mut() {
                state.input = input;
            }
            children.push(part);
        }
    } else if kind == "GENERIC" {
        if let Some(state) = children.last_mut().and_then(|part| part.state.as_mut()) {
            state.status = if status == "DONE" {
                "completed"
            } else if status == "ERROR" {
                "error"
            } else {
                "running"
            }
            .into();
            state.output = record["content"].as_str().map(str::to_owned);
        }
    }
}

fn with_children(spawn: &WirePart, children: Vec<WirePart>, running: bool) -> WirePart {
    let mut result = spawn.clone();
    result.children = children;
    let state = result.state.get_or_insert_with(|| WireToolState {
        status: String::new(),
        input: None,
        output: None,
        error: None,
        title: None,
    });
    state.status = if running { "running" } else { "completed" }.into();
    state.output = None;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opencode_polls_until_the_assistant_finishes_its_response() {
        for message in [
            serde_json::json!({"time":{"created":1},"content":[]}),
            serde_json::json!({"time":{"created":1,"completed":null}}),
            serde_json::json!({"time":{"created":1},"content":[{"type":"text","text":"Still writing"}]}),
            serde_json::json!({"time":{"completed":2},"finish":"tool-calls"}),
        ] {
            assert!(opencode_message_running(&message));
        }
        for finish in ["stop", "length", "error"] {
            assert!(!opencode_message_running(
                &serde_json::json!({"time":{"completed":3},"finish":finish})
            ));
        }
    }
    #[test]
    fn antigravity_children_preserve_tools_and_parent_update_without_reasoning() {
        let spawn = WirePart::tool("spawn", "task", "completed", None);
        let mut children = Vec::new();
        let mut running = true;
        apply_antigravity_record(
            &spawn,
            &serde_json::json!({"type":"PLANNER_RESPONSE","status":"DONE","step_index":1,"thinking":"hidden","tool_calls":[{"name":"run_command","args":{"CommandLine":"\"pwd\""}}]}),
            Some("parent"),
            &mut children,
            &mut running,
        );
        assert_eq!(children[0].tool.as_deref(), Some("Bash"));
        assert_eq!(
            children[0].state.as_ref().unwrap().input.as_ref().unwrap()["command"],
            "pwd"
        );
        apply_antigravity_record(
            &spawn,
            &serde_json::json!({"type":"PLANNER_RESPONSE","status":"DONE","step_index":3,"tool_calls":[{"name":"send_message","args":{"Recipient":"parent","Message":"done"}}]}),
            Some("parent"),
            &mut children,
            &mut running,
        );
        assert_eq!(children[1].tool.as_deref(), Some("SubagentHandback"));
        assert_eq!(
            children[1].state.as_ref().unwrap().input.as_ref().unwrap()["message"],
            "done"
        );
        assert!(children
            .iter()
            .all(|part| part.text.as_deref() != Some("hidden")));
    }

    #[test]
    fn background_receipt_identifies_only_child_sessions() {
        let mut part = WirePart::tool("spawn", "subagent", "completed", None);
        part.state.as_mut().unwrap().output =
            Some("The subagent is working in the background (sessionID: ses_child123).".into());
        assert_eq!(opencode_child(&part).as_deref(), Some("ses_child123"));
        part.tool = Some("bash".into());
        assert!(opencode_child(&part).is_none());
    }
}
