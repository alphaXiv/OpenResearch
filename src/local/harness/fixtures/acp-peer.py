import json
import os
import socket
import sys

sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")

# A mode may carry a port, such as "slow-delete:4000" for the test's connection that holds the delete.
mode, _, port = (sys.argv[1] if len(sys.argv) > 1 else "normal").partition(":")
record = sys.argv[2] if len(sys.argv) > 2 else None
session = "peer-session"
count = 0
hanging = None
startup = "pi v1.5.0\n---\nSkills: none\n"
# Like pi-acp, the startup modes send a chunk after session/new, before the agent's turn.
# It goes out when the prompt arrives, so it always reaches the client inside its turn.
pending = None


def send(value):
    print(json.dumps(dict(jsonrpc="2.0", **value)), flush=True)


def update(value):
    send(dict(method="session/update", params=dict(sessionId=session, update=value)))


def reply(request, result):
    send(dict(id=request["id"], result=result))


def reverse(method, params):
    send(dict(id="reverse", method=method, params=dict(sessionId=session, **params)))
    response = json.loads(sys.stdin.readline())
    assert response["id"] == "reverse", response
    return response


def note(method, params):
    if record:
        with open(record, "a", encoding="utf-8") as file:
            file.write(json.dumps(dict(method=method, params=params, cwd=os.getcwd())) + "\n")


def config(model="first"):
    return [dict(id="opaque-model", name="Model", category="model", type="select", currentValue=model,
                 options=[dict(value="first", name="First"), dict(value="second", name="Second"), dict(value="reject", name="Rejected model")]),
            dict(id="opaque-thought", name="Thinking", category="thought_level", type="select",
                 currentValue="high" if model == "second" else "low",
                 options=[dict(group="levels", name="Levels", options=[dict(value="high" if model == "second" else "low", name="Native effort")])])]


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    params = request.get("params", {})
    if method:
        note(method, params)
    if mode.startswith("alphaxiv-") and method in ["session/new", "session/load", "session/resume"]:
        [server] = params["mcpServers"]
        assert server["name"] == "alphaxiv", server
        assert os.path.isabs(server["command"]), server
        assert server["args"] == ["alphaxiv-mcp"], server
        assert server["env"] == [], server
    if method == "initialize":
        if mode == "environment":
            assert os.environ["ORX_CHAT_SESSION_ID"] == "environment-chat"
            assert os.environ["ORX_LOCAL_SESSION"] == "1"
            assert os.environ["ORX_UP_PORT"] == "4901"
            assert os.environ["ORX_CHAT_HARNESS"].startswith("acp:")
        assert params["clientCapabilities"]["fs"] == {"readTextFile": True, "writeTextFile": True}
        assert params["clientCapabilities"]["terminal"] is True
        if mode == "crash-init":
            sys.exit(3)
        if mode == "stalled-init":
            continue
        if mode == "auth-init":
            send(dict(id=request["id"], error=dict(code=-32000, message="Authentication required")))
        else:
            reply(request, dict(protocolVersion=2 if mode == "incompatible" else 1,
                                agentInfo=dict(name="Deterministic ACP peer", version="1.0"),
                                agentCapabilities=dict(loadSession=mode != "no-resume", **({"sessionCapabilities": {"resume": {}}} if mode in ["resume", "alphaxiv-resume"] else {"sessionCapabilities": {"resume": {}, "list": {}}} if mode == "unlisted" else {}),
                                                       **({"sessionCapabilities": {"delete": {}}} if mode in ["delete", "slow-delete"] else {}))))
    elif method == "session/new":
        if mode == "stalled-setup":
            continue
        if mode == "legacy-models":
            reply(request, dict(sessionId=session, models=dict(currentModelId="first", availableModels=[
                dict(modelId="first", name="First"), dict(modelId="second", name="Second")])))
        elif mode in ["startup", "startup-other"]:
            reply(request, dict(sessionId=session, configOptions=config(), _meta=dict(piAcp=dict(startupInfo=startup))))
            pending = startup if mode == "startup" else "Update available\n"
        else:
            reply(request, dict(sessionId=session, configOptions=config()))
    elif method in ["session/load", "session/resume"]:
        if mode == "missing":
            send(dict(id=request["id"], error=dict(code=-32002, message="Session not found")))
        elif mode == "unlisted":
            send(dict(id=request["id"], error=dict(code=-32603, message="Failed to read session")))
        elif mode == "auth-load":
            send(dict(id=request["id"], error=dict(code=-32000, message="Authentication required")))
        else:
            update(dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="replayed")))
            reply(request, dict(configOptions=config()))
    elif method == "session/set_config_option":
        if params["value"] == "reject":
            send(dict(id=request["id"], error=dict(code=-32602, message="Rejected selection")))
        else:
            update(dict(sessionUpdate="current_mode_update", currentModeId="agent-changed"))
            reply(request, dict(configOptions=config(params["value"])))
    elif method == "session/set_model" and mode == "legacy-models":
        reply(request, {})
    elif method == "session/delete" and mode in ["delete", "slow-delete"]:
        if mode == "slow-delete":
            # The test answers the delete by closing this connection.
            with socket.create_connection(("127.0.0.1", int(port))) as hold:
                hold.recv(1)
            note("answered session/delete", params)
        reply(request, {})
    elif method == "session/prompt":
        if pending:
            update(dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=pending)))
            pending = None
        full = params["prompt"][0]["text"]
        text = full.split("ACP_TEST:", 1)[-1]
        if text == "crash":
            sys.exit(4)
        if text == "error":
            send(dict(id=request["id"], error=dict(code=-32603, message="Model overloaded")))
            continue
        if text == "empty":
            reply(request, dict(stopReason="end_turn"))
            continue
        count += 1
        update(dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=f"turn {count}")))
        update(dict(sessionUpdate="tool_call_update", toolCallId="tool-one", status="in_progress", rawOutput="partial"))
        update(dict(sessionUpdate="agent_thought_chunk", content=dict(type="text", text="thinking")))
        update(dict(sessionUpdate="tool_call", toolCallId="tool-one", title="Inspect file", kind="read", status="completed"))
        if text.startswith("permission"):
            send(dict(id="permission", method="session/request_permission", params=dict(sessionId=session,
                toolCall=dict(toolCallId="tool-one", title="Inspect file"), options=[
                    dict(optionId="opaque-once", name="Allow this action", kind="allow_once"),
                    dict(optionId="opaque-deny", name="Reject this action", kind="reject_once")])))
            update(dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="while waiting")))
            answer = json.loads(sys.stdin.readline())
            assert answer["id"] == "permission", answer
            update(dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=json.dumps(answer["result"]))))
        if text == "echo":
            update(dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=full[:len(full) // 2])))
            update(dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=full[len(full) // 2:])))
        if text.startswith("denied-write:"):
            response = reverse("fs/write_text_file", dict(path=text[13:], content="written"))
            update(dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=json.dumps(response.get("error")))))
        if text == "hang":
            hanging = request
            continue
        if text.startswith("files:"):
            path = text[6:]
            response = reverse("fs/write_text_file", dict(path=path, content="one\ntwo\nthree\n"))
            assert "error" not in response, response
            response = reverse("fs/read_text_file", dict(path=path, line=2, limit=1))
            assert response["result"]["content"] == "two\n", response
            response = reverse("fs/read_text_file", dict(path="relative"))
            assert "error" in response
        if text == "terminal":
            response = reverse("terminal/create", dict(command=sys.executable, args=["-X", "utf8", "-c", "print('héllo' * 1000)"], outputByteLimit=100))
            terminal = response["result"]["terminalId"]
            response = reverse("terminal/wait_for_exit", dict(terminalId=terminal))
            assert response["result"]["exitCode"] == 0, response
            response = reverse("terminal/output", dict(terminalId=terminal))
            assert response["result"]["truncated"], response
            assert len(response["result"]["output"].encode()) <= 100
            reverse("terminal/release", dict(terminalId=terminal))
            assert "error" in reverse("terminal/output", dict(terminalId=terminal))
        reply(request, dict(stopReason="refusal" if text == "refuse" else "end_turn"))
    elif method == "session/list":
        if params.get("cursor") is None:
            reply(request, dict(sessions=[dict(sessionId="first-page", cwd="/")], nextCursor="page-2"))
        else:
            reply(request, dict(sessions=[dict(sessionId="kept", cwd="/")]))
    elif method == "session/cancel":
        if hanging:
            reply(hanging, dict(stopReason="cancelled"))
            hanging = None
        else:
            sys.exit(0)
    elif "id" in request:
        send(dict(id=request["id"], error=dict(code=-32601, message="Unknown method")))
