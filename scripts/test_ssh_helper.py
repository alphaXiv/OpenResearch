import base64
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "src/jobs/ssh/helper.py"


class HelperTest(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        self.bin = Path(self.home.name) / "bin"
        self.bin.mkdir()
        self.process = subprocess.Popen([sys.executable, "-u", str(HELPER)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env={**os.environ, "HOME": self.home.name, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"]})
        self.assertEqual(self.process.stdout.readline(), b"__ORX_SSH_HELPER_V1__\n")
        self.assertEqual(self.receive(), {"version": 1})
        self.sequence = 0

    def tearDown(self):
        self.process.stdin.close()
        self.process.wait(timeout=5)
        self.process.stdout.close()
        self.process.stderr.close()
        self.home.cleanup()

    def send(self, **request):
        self.sequence += 1
        request["id"] = self.sequence
        data = json.dumps(request).encode()
        self.process.stdin.write(struct.pack("!I", len(data)) + data)
        self.process.stdin.flush()
        return self.sequence

    def receive(self):
        size = struct.unpack("!I", self.process.stdout.read(4))[0]
        self.assertLessEqual(size, 1024 * 1024)
        return json.loads(self.process.stdout.read(size))

    def rpc(self, **request):
        identifier = self.send(**request)
        response = self.receive()
        self.assertEqual(response["id"], identifier)
        self.assertNotIn("error", response)
        return response["result"]

    def test_slow_command_does_not_block_heartbeat_or_logs(self):
        slow = self.send(op="exec", command="sleep 1; printf finished")
        ping = self.send(op="ping")
        response = self.receive()
        self.assertEqual(response["id"], ping)
        self.assertEqual(response["result"]["version"], 1)
        self.assertEqual(self.receive()["id"], slow)

    def test_heartbeat_bypasses_busy_control_workers(self):
        requests = {self.send(op="exec", lane="control", command="sleep 1") for _ in range(2)}
        ping = self.send(op="ping")
        self.assertEqual(self.receive()["id"], ping)
        self.assertEqual({self.receive()["id"] for _ in range(2)}, requests)

    def test_command_failure_keeps_transport_usable(self):
        result = self.rpc(op="exec", command="printf failed >&2; exit 7")
        self.assertEqual(result["code"], 7)
        self.assertEqual(base64.b64decode(result["stderr"]), b"failed")
        self.assertEqual(self.rpc(op="ping")["version"], 1)

    def test_log_bytes_partial_lines_and_replacement(self):
        folder = Path(self.home.name) / ".orx/runs/test"
        folder.mkdir(parents=True)
        path = folder / "log"
        original = b"\xff\x00partial \xf0\x9f"
        path.write_bytes(original)
        first = self.rpc(op="log", dir=".orx/runs/test")
        self.assertEqual(base64.b64decode(first["data"]), original)
        self.assertTrue(first["reset"])
        with path.open("ab") as file:
            file.write(b"\x98\x80\n")
        second = self.rpc(op="log", dir=".orx/runs/test", offset=len(original),
                          identity=first["identity"], boundary=first["boundary"])
        self.assertEqual(base64.b64decode(second["data"]), b"\x98\x80\n")
        self.assertFalse(second["reset"])
        path.write_bytes(b"replacement larger than the previously collected log")
        replaced = self.rpc(op="log", dir=".orx/runs/test", offset=len(original)+3,
                            identity=second["identity"], boundary=second["boundary"])
        self.assertTrue(replaced["reset"])
        self.assertEqual(replaced["offset"], 0)
        path.unlink()
        self.assertTrue(self.rpc(op="log", dir=".orx/runs/test")["missing"])

    def test_large_logs_are_bounded_and_uploads_do_not_mix(self):
        folder = Path(self.home.name) / ".orx/runs/test"
        folder.mkdir(parents=True)
        (folder / "log").write_bytes(b"a" * (256 * 1024 + 10))
        chunk = self.rpc(op="log", dir=".orx/runs/test")
        self.assertEqual(len(base64.b64decode(chunk["data"])), 256 * 1024)
        self.assertFalse(chunk["eof"])
        one = self.rpc(op="upload_start")["token"]
        two = self.rpc(op="upload_start")["token"]
        for token, value in ((one, b"alpha"), (two, b"beta")):
            self.rpc(op="upload_chunk", token=token, data=base64.b64encode(value).decode())
        result = self.rpc(op="upload_commit", token=two, command="cat")
        self.assertEqual(base64.b64decode(result["stdout"]), b"beta")
        result = self.rpc(op="upload_commit", token=one, command="cat")
        self.assertEqual(base64.b64decode(result["stdout"]), b"alpha")

    def test_slurm_batches_and_distinguishes_hidden_from_unavailable(self):
        queue = self.bin / "squeue"
        queue.write_text("#!/bin/sh\necho \"$*\" >> \"$HOME/queue-calls\"\ncase \"$*\" in\n *'-j '*) exit 1;;\n *--all*--states=all*) echo '42|SUSPENDED';;\nesac\n")
        queue.chmod(0o700)
        accounting = self.bin / "sacct"
        accounting.write_text("#!/bin/sh\nexit 1\n")
        accounting.chmod(0o700)
        jobs = [{"run": "run-one", "job": "42"}, {"run": "run-two", "job": "43"}]
        states = self.rpc(op="slurm", jobs=jobs)
        self.assertEqual(states, {"run-one": "SQ SUSPENDED", "run-two": "GONE"})
        calls = (Path(self.home.name) / "queue-calls").read_text().splitlines()
        self.assertEqual(len(calls), 2)
        self.assertIn("42,43", calls[0])
        queue.write_text("#!/bin/sh\nexit 1\n")
        self.assertEqual(self.rpc(op="slurm", jobs=jobs), {"run-one": "UNAVAILABLE", "run-two": "UNAVAILABLE"})

    def test_invalid_operation_is_not_a_disconnect(self):
        self.send(op="unknown")
        self.assertIn("error", self.receive())
        self.assertEqual(self.rpc(op="ping")["version"], 1)


if __name__ == "__main__":
    unittest.main()
