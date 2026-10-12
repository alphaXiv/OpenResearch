import json
import os
import sys

sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")

mode = sys.argv[1] if len(sys.argv) > 1 else "normal"
session = "peer-session"
count = 0


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
    if method == "initialize":
        assert params["clientCapabilities"]["fs"] == {"readTextFile": True, "writeTextFile": True}
        assert params["clientCapabilities"]["terminal"] is True
        if mode == "crash-init":
            sys.exit(3)
        if mode == "auth-init":
            send(dict(id=request["id"], error=dict(code=-32000, message="Authentication required")))
        else:
            reply(request, dict(protocolVersion=2 if mode == "incompatible" else 1,
                                agentInfo=dict(name="Deterministic ACP peer", version="1.0"),
                                agentCapabilities=dict(loadSession=mode != "no-resume", **({"sessionCapabilities": {"resume": {}}} if mode == "resume" else {}))))
    elif method == "session/new":
        reply(request, dict(sessionId=session, configOptions=config()))
    elif method in ["session/load", "session/resume"]:
        if mode == "missing":
            send(dict(id=request["id"], error=dict(code=-32002, message="Session not found")))
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
    elif method == "session/prompt":
        text = params["prompt"][0]["text"].split("ACP_TEST:", 1)[-1]
        if text == "crash":
            sys.exit(4)
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
        reply(request, dict(stopReason="end_turn"))
    elif method == "session/cancel":
        sys.exit(0)
    else:
        send(dict(id=request["id"], error=dict(code=-32601, message="Unknown method")))
