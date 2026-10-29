import base64
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile


def peer():
    options = [{"id": "mode", "name": "Mode", "type": "select", "currentValue": "one",
                "options": [{"value": "one", "name": "One"}, {"value": "two", "name": "Two"}]}]
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request["method"]
        if method in ("initialize", "session/new", "session/load"):
            cwd = Path(request.get("params", {}).get("cwd", os.getcwd()))
            if (cwd / ".openresearch").exists():
                skills = cwd / ".agents/skills"
                for relative in ("orx-compute/SKILL.md", "orx-compute/references/local.md",
                                 "uploaded-check/SKILL.md", "uploaded-check/reference.txt"):
                    assert (skills / relative).is_file(), relative
                (Path(os.environ["ORX_DATA_DIR"]) / "skill-check.json").write_text(
                    json.dumps({"cwd": str(cwd), "method": method}))
        if method == "initialize":
            result = {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}}
        elif method in ("session/new", "session/load"):
            result = {"sessionId": "skill-test", "configOptions": options}
        elif method == "session/prompt":
            print(json.dumps({"jsonrpc": "2.0", "method": "session/update", "params": {
                "sessionId": "skill-test", "update": {"sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "NATIVE_SKILLS_OK"}}}}), flush=True)
            result = {"stopReason": "end_turn"}
        elif method == "session/set_config_option":
            options[0]["currentValue"] = request["params"]["value"]
            result = {"configOptions": options}
        else:
            raise AssertionError(method)
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
        if method in ("session/prompt", "session/set_config_option"):
            break


def wait_for(condition):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(0.1)
    raise AssertionError("Timed out waiting for ACP skill check")


def test(binary):
    with tempfile.TemporaryDirectory(prefix="orx-acp-skills-") as directory:
        root = Path(directory)
        data = root / "data"
        env = dict(os.environ, HOME=directory, XDG_CONFIG_HOME=str(root / "config"),
                   XDG_DATA_HOME=str(root / "share"), ORX_DATA_DIR=str(data),
                   ORX_NO_UPDATE_CHECK="1", PATH="/usr/bin:/bin")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

        def api(route, body=None, method=None):
            request = urllib.request.Request(
                "http://127.0.0.1:" + str(port) + route,
                data=None if body is None else json.dumps(body).encode(), method=method,
                headers={"Content-Type": "application/json"})
            try:
                with opener.open(request, timeout=30) as response:
                    return json.load(response)
            except urllib.error.HTTPError as error:
                raise AssertionError(route + ": " + error.read().decode()) from error

        with (root / "backend.log").open("w+") as log:
            up = subprocess.Popen([binary, "--no-telemetry", "up", "--no-browser", "--port", str(port)],
                                  env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
            session = None
            try:
                def ready():
                    assert up.poll() is None, (root / "backend.log").read_text()
                    try:
                        api("/api/projects")
                        return True
                    except OSError:
                        return False
                wait_for(ready)
                archive = io.BytesIO()
                with zipfile.ZipFile(archive, "w") as package:
                    package.writestr("uploaded-check/SKILL.md",
                                     "---\nname: uploaded-check\ndescription: Test native uploads\n---\nTest skill.\n")
                    package.writestr("uploaded-check/reference.txt", "Uploaded resource")
                api("/api/user-skills", {"filename": "uploaded-check.zip",
                    "contentBase64": base64.b64encode(archive.getvalue()).decode()})
                harness = api("/api/harnesses/acp", {"name": "Native skills test", "executable": sys.executable,
                              "arguments": ["-u", str(Path(__file__).resolve()), "--peer"]})
                project = api("/api/projects", {"name": "Native skills test", "path": str(root / "repo"),
                              "createFolder": True, "initializeGit": True, "githubSyncEnabled": False})["project"]
                session = api("/api/chat/sessions", {"projectId": project["id"], "harness": harness["id"]})["session"]["id"]
                route = "/api/chat/sessions/" + session
                api(route + "/message", {"text": "Check native skills"})
                wait_for(lambda: "NATIVE_SKILLS_OK" in json.dumps(api(route + "/messages")))
                marker = data / "skill-check.json"
                startup = json.loads(marker.read_text())
                assert startup["method"] == "session/new", startup
                skills = Path(startup["cwd"]) / ".agents/skills"
                for relative in ("orx-compute/SKILL.md", "orx-compute/references/local.md",
                                 "uploaded-check/SKILL.md", "uploaded-check/reference.txt"):
                    (skills / relative).unlink()
                wait_for(lambda: not api(route)["session"]["busy"])
                api(route + "/configuration", {"optionId": "mode", "value": "two"})
                assert json.loads(marker.read_text())["method"] == "session/load"
            finally:
                try:
                    if session:
                        api("/api/chat/sessions/" + session, method="DELETE")
                finally:
                    up.terminate()
                    up.wait(timeout=10)


if __name__ == "__main__":
    if sys.argv[1] == "--peer":
        peer()
    else:
        test(sys.argv[1])
        print("ACP startup and configuration restore install bundled and uploaded skill resources")
