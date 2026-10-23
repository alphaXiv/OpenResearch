import base64
import json
import hashlib
import importlib.util
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HELPER = Path(__file__).resolve().parents[1] / "src/jobs/ssh/helper.py"


class DeadlineTest(unittest.TestCase):
    def test_setup_and_upload_are_not_limited_by_monitoring_deadline(self):
        spec = importlib.util.spec_from_file_location("ssh_helper", HELPER)
        helper = importlib.util.module_from_spec(spec)
        with patch.object(sys, "dont_write_bytecode", True):
            spec.loader.exec_module(helper)
        with patch.object(helper, "execute", return_value={}) as execute:
            helper.dispatch({"op": "exec", "command": "setup"})
            self.assertIsNone(execute.call_args[0][2])
            helper.dispatch({"op": "exec", "command": "status", "lane": "status"})
            self.assertEqual(execute.call_args[0][2], 120)
            token = helper.dispatch({"op": "upload_start"})["token"]
            helper.dispatch({"op": "upload_commit", "command": "extract", "token": token})
            self.assertIsNone(execute.call_args[0][2])
            for lane in ("control", "status"):
                token = helper.dispatch({"op": "upload_start"})["token"]
                helper.dispatch({"op": "upload_commit", "command": "check", "token": token, "lane": lane})
                self.assertEqual(execute.call_args[0][2], 120)


class LogReadRaceTest(unittest.TestCase):
    def test_unchanged_catchup_and_idle_polls_do_not_rescan_collected_bytes(self):
        spec = importlib.util.spec_from_file_location("ssh_helper", HELPER)
        helper = importlib.util.module_from_spec(spec)
        with patch.object(sys, "dont_write_bytecode", True):
            spec.loader.exec_module(helper)
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "log"
            data = b"a" * (helper.CHUNK * 5 + 10)
            path.write_bytes(data)
            read_bytes = []
            real_open = open

            class CountingFile:
                def __init__(self, file):
                    self.file = file

                def __enter__(self):
                    return self

                def __exit__(self, *args):
                    self.file.close()

                def __getattr__(self, name):
                    return getattr(self.file, name)

                def read(self, size):
                    result = self.file.read(size)
                    read_bytes.append(len(result))
                    return result

            request = {"dir": folder}
            collected = b""
            with patch.object(helper, "open", side_effect=lambda *args, **kwargs:
                              CountingFile(real_open(*args, **kwargs))):
                for _ in range(10):
                    chunk = helper.log_chunk(request)
                    collected += base64.b64decode(chunk["data"])
                    request.update(offset=chunk["offset"] + len(base64.b64decode(chunk["data"])),
                                   identity=chunk["identity"], boundary=chunk["boundary"])
            self.assertEqual(collected, data)
            self.assertLessEqual(sum(read_bytes), len(data) * 2)

    def test_rewrite_during_collection_is_retried_but_append_is_allowed(self):
        spec = importlib.util.spec_from_file_location("ssh_helper", HELPER)
        helper = importlib.util.module_from_spec(spec)
        with patch.object(sys, "dont_write_bytecode", True):
            spec.loader.exec_module(helper)
        with tempfile.TemporaryDirectory() as home:
            path = Path(home) / "log"
            original = b"a" * helper.CHUNK
            replacement = b"b" * (helper.CHUNK * 2)
            real_open = open

            class ChangingFile:
                def __init__(self, file, change):
                    self.file = file
                    self.change = change

                def __enter__(self):
                    return self

                def __exit__(self, *args):
                    self.file.close()

                def __getattr__(self, name):
                    return getattr(self.file, name)

                def read(self, size):
                    data = self.file.read(size)
                    if self.change:
                        change, self.change = self.change, None
                        with real_open(path, change) as writer:
                            writer.write(replacement)
                    return data

            with patch.dict(os.environ, {"HOME": home}):
                for mode in ("wb", "ab"):
                    with self.subTest(mode=mode):
                        path.write_bytes(original)
                        with patch.object(helper, "open", side_effect=lambda *args, **kwargs:
                                          ChangingFile(real_open(*args, **kwargs), mode), create=True):
                            if mode == "wb":
                                with self.assertRaisesRegex(RuntimeError, "changed during collection"):
                                    helper.log_chunk({"dir": ""})
                            else:
                                chunk = helper.log_chunk({"dir": ""})
                                self.assertEqual(base64.b64decode(chunk["data"]), original)
                                self.assertFalse(chunk["eof"])
                        retry = helper.log_chunk({"dir": ""})
                        expected = replacement if mode == "wb" else original
                        self.assertEqual(base64.b64decode(retry["data"]), expected[:helper.CHUNK])


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

    def test_rewrite_with_unchanged_suffix_replaces_the_collected_prefix(self):
        folder = Path(self.home.name) / ".orx/runs/test"
        folder.mkdir(parents=True)
        path = folder / "log"
        original = b"a" * (256 * 1024) + b"z" * 64
        path.write_bytes(original)
        first = self.rpc(op="log", dir=str(folder))
        second = self.rpc(op="log", dir=str(folder), offset=256 * 1024,
                          identity=first["identity"], boundary=first["boundary"])
        for replacement in (b"b" * (256 * 1024) + b"z" * 64,
                            b"c" * (256 * 1024) + b"z" * 64 + b"appended"):
            before = path.stat()
            path.write_bytes(replacement)
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
            changed = self.rpc(op="log", dir=str(folder), offset=len(original),
                               identity=second["identity"], boundary=second["boundary"])
            self.assertTrue(changed["reset"])
            self.assertEqual(changed["offset"], 0)
            self.assertEqual(base64.b64decode(changed["data"]), replacement[:256 * 1024])

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

    def test_multiframe_command_input_preserves_all_bytes(self):
        data = bytes(range(251)) * 9000
        token = self.rpc(op="upload_start")["token"]
        for offset in range(0, len(data), 256 * 1024):
            self.rpc(op="upload_chunk", token=token,
                     data=base64.b64encode(data[offset:offset + 256 * 1024]).decode())
        result = self.rpc(op="upload_commit", token=token, command=
                          "python3 -c 'import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())'")
        self.assertEqual(result["code"], 0)
        self.assertEqual(base64.b64decode(result["stdout"]).decode().strip(),
                         hashlib.sha256(data).hexdigest())
        self.assertEqual(base64.b64decode(result["stderr"]), b"")

    def test_slurm_reads_exit_code_from_custom_remote_directory(self):
        directory = Path(self.home.name) / "scratch" / "run-one"
        directory.mkdir(parents=True)
        (directory / "exit_code").write_text("7")
        for path in (str(directory), "scratch/run-one"):
            self.assertEqual(self.rpc(op="slurm", jobs=[{
                "run": "run-one", "job": "42", "dir": path,
            }]), {"run-one": "EXIT 7"})

    def test_slurm_exit_file_error_does_not_block_other_jobs(self):
        home = Path(self.home.name)
        broken = home / ".orx" / "runs" / "run-one"
        healthy = home / ".orx" / "runs" / "run-two"
        (broken / "exit_code").mkdir(parents=True)
        healthy.mkdir(parents=True)
        (healthy / "exit_code").write_text("0")
        queue = self.bin / "squeue"
        queue.write_text("#!/bin/sh\necho '42|RUNNING'\n")
        queue.chmod(0o700)
        self.assertEqual(self.rpc(op="slurm", jobs=[
            {"run": "run-one", "job": "42"}, {"run": "run-two", "job": "43"},
        ]), {"run-one": "SQ RUNNING", "run-two": "EXIT 0"})
        queue.write_text("#!/bin/sh\nexit 1\n")
        accounting = self.bin / "sacct"
        accounting.write_text("#!/bin/sh\nexit 1\n")
        accounting.chmod(0o700)
        self.assertEqual(self.rpc(op="slurm", jobs=[
            {"run": "run-one", "job": "42"}, {"run": "run-two", "job": "43"},
        ]), {"run-one": "UNAVAILABLE", "run-two": "EXIT 0"})
        (broken / "exit_code").rmdir()
        (broken / "exit_code").write_bytes(b"\xff")
        queue.write_text("#!/bin/sh\necho '42|RUNNING'\n")
        self.assertEqual(self.rpc(op="slurm", jobs=[
            {"run": "run-one", "job": "42"}, {"run": "run-two", "job": "43"},
        ]), {"run-one": "SQ RUNNING", "run-two": "EXIT 0"})

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
