"""Exercise the ACP adapter against a disposable dev slot: python3 scripts/test_acp.py URL."""

import contextlib
import json
import pathlib
import sys
import tempfile
import time
import urllib.error
import urllib.request


def main():
    base = sys.argv[1].rstrip("/")
    assert base.startswith(("http://127.0.0.1:49", "http://localhost:49")), "Use an isolated dev slot"

    def api(method, path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(base + path, data=data, method=method, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)

    def settled(sid, previous=None):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            messages = api("GET", f"/api/chat/sessions/{sid}/messages")["messages"]
            if messages and messages[-1]["id"] != previous and messages[-1]["role"] == "assistant" and messages[-1].get("completedAt"):
                return messages
            time.sleep(0.05)
        raise AssertionError("ACP turn did not settle")

    def send(sid, text):
        messages = api("GET", f"/api/chat/sessions/{sid}/messages")["messages"]
        previous = messages[-1]["id"] if messages else None
        api("POST", f"/api/chat/sessions/{sid}/message", {"text": "ACP_TEST:" + text})
        return settled(sid, previous)

    def cleanup(path):
        try:
            api("DELETE", path)
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise

    with contextlib.ExitStack() as resources:
        directory = resources.enter_context(tempfile.TemporaryDirectory(prefix="or-378-peer-project-"))
        pathlib.Path(directory, "sample.txt").write_text("original\n")
        project = api("POST", "/api/projects", {"name": "ACP protocol verification", "path": directory, "initializeGit": True, "githubSyncEnabled": False})["project"]
        resources.callback(cleanup, f"/api/projects/{project['id']}")
        definition = api("POST", "/api/harnesses/acp", {"name": "Protocol peer", "executable": sys.executable, "arguments": [str(pathlib.Path(__file__).resolve().parents[1] / "src/local/harness/fixtures/acp-peer.py"), "normal"]})
        resources.callback(cleanup, f"/api/harnesses/acp/{definition['id']}")
        sid = api("POST", "/api/chat/sessions", {"projectId": project["id"], "harness": definition["id"]})["session"]["id"]
        messages = send(sid, "hello")
        parts = messages[-1]["parts"]
        assert [part["type"] for part in parts] == ["text", "tool", "reasoning"], parts
        assert parts[1]["state"]["output"] == "partial", parts
        assert parts[1]["state"]["status"] == "completed", parts
        changed = api("POST", f"/api/chat/sessions/{sid}/configuration", {"optionId": "opaque-model", "value": "second"})
        assert changed["nativeConfiguration"]["configOptions"][1]["currentValue"] == "high", changed
        assert changed["nativeConfiguration"]["modes"]["currentModeId"] == "agent-changed", changed
        try:
            api("POST", f"/api/chat/sessions/{sid}/configuration", {"optionId": "opaque-model", "value": "reject"})
            raise AssertionError("Rejected model was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 400
        for selected in ["opaque-once", "opaque-deny"]:
            api("POST", f"/api/chat/sessions/{sid}/message", {"text": "ACP_TEST:permission"})
            deadline = time.monotonic() + 10
            while True:
                messages = api("GET", f"/api/chat/sessions/{sid}/messages")["messages"]
                cards = [part for part in messages[-1]["parts"] if part.get("prompt", {}).get("nativeChoices") and not part["prompt"].get("resolved")]
                if cards:
                    card = cards[0]
                    break
                assert time.monotonic() < deadline, "Permission card did not arrive"
                time.sleep(0.05)
            assert [choice["id"] for choice in card["prompt"]["nativeChoices"]] == ["opaque-once", "opaque-deny"]
            answer = {"promptId": card["id"], "answers": [selected], "approve": selected == "opaque-once"}
            api("POST", f"/api/chat/sessions/{sid}/respond", answer)
            assert selected in json.dumps(settled(sid)[-1])
            try:
                api("POST", f"/api/chat/sessions/{sid}/respond", answer)
                raise AssertionError("Duplicate approval accepted")
            except urllib.error.HTTPError as error:
                assert error.code == 400
        assert "turn 4" in json.dumps(send(sid, "hello"))
        send(sid, "files:" + str(pathlib.Path(directory, "sample.txt")))
        assert pathlib.Path(directory, "sample.txt").read_text() == "one\ntwo\nthree\n"
        send(sid, "terminal")
        api("POST", f"/api/chat/sessions/{sid}/message", {"text": "ACP_TEST:permission"})
        deadline = time.monotonic() + 10
        while True:
            messages = api("GET", f"/api/chat/sessions/{sid}/messages")["messages"]
            if any(part.get("prompt", {}).get("nativeChoices") and not part["prompt"].get("resolved") for part in messages[-1]["parts"]):
                break
            assert time.monotonic() < deadline, "Permission card did not arrive"
            time.sleep(0.05)
        api("POST", f"/api/chat/sessions/{sid}/interrupt", {})
        messages = api("GET", f"/api/chat/sessions/{sid}/messages")["messages"]
        assert all(part["prompt"].get("resolved") for message in messages for part in message["parts"] if part.get("prompt", {}).get("nativeChoices")), messages
        send(sid, "crash")
        restored = send(sid, "hello")
        assert not any(part.get("text", "").startswith("replayed") for message in restored for part in message["parts"]), restored
        assert "turn 1" in json.dumps(restored[-1])
        api("PUT", f"/api/harnesses/acp/{definition['id']}", {"name": "Changed definition", "executable": "missing-executable", "arguments": []})
        api("DELETE", f"/api/harnesses/acp/{definition['id']}")
        messages = send(sid, "hello")
        assert "turn 2" in json.dumps(messages)
        api("POST", f"/api/chat/sessions/{sid}/fork", {"messageId": messages[-1]["id"]})
        assert "turn 1" in json.dumps(settled(sid, messages[-1]["id"])[-1])
        side = api("POST", f"/api/chat/sessions/{sid}/side", {})["session"]
        assert side["harnessName"] == "Protocol peer", side
        assert "turn 1" in json.dumps(send(side["id"], "hello"))
        for mode in ["resume", "missing", "no-resume", "auth-load"]:
            definition = api("POST", "/api/harnesses/acp", {"name": "Protocol " + mode, "executable": sys.executable, "arguments": [str(pathlib.Path(__file__).resolve().parents[1] / "src/local/harness/fixtures/acp-peer.py"), mode]})
            resources.callback(cleanup, f"/api/harnesses/acp/{definition['id']}")
            sid = api("POST", "/api/chat/sessions", {"projectId": project["id"], "harness": definition["id"]})["session"]["id"]
            send(sid, "hello")
            send(sid, "crash")
            restored = json.dumps(send(sid, "hello")[-1])
            if mode == "auth-load":
                assert "Authentication required" in restored and "turn 1" not in restored, restored
            else:
                assert "turn 1" in restored and "replayed" not in restored, restored
                assert ("saved conversation transcript" in restored) == (mode in ["missing", "no-resume"]), restored
            api("DELETE", f"/api/chat/sessions/{sid}")
            api("DELETE", f"/api/harnesses/acp/{definition['id']}")
        api("DELETE", f"/api/projects/{project['id']}")
    print("ACP adapter: streaming, tool merge, configuration, files, terminals, cancellation, continuation, restoration, launch snapshot, fork, side chat, deletion passed")


if __name__ == "__main__":
    main()
