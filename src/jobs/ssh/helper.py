import base64
import concurrent.futures
from collections import OrderedDict
import hashlib
import json
import os
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time

if sys.version_info < (3, 6):
    raise RuntimeError("OpenResearch SSH monitoring requires Python 3.6 or newer")

VERSION = 1
LIMIT = 1024 * 1024
CHUNK = 256 * 1024
output_lock = threading.Lock()
upload_lock = threading.Lock()
uploads = {}
log_cache = OrderedDict()
log_cache_lock = threading.Lock()
lanes = {name: concurrent.futures.ThreadPoolExecutor(max_workers=count)
         for name, count in (("control", 2), ("logs", 2), ("status", 2), ("commands", 4))}
slots = {name: threading.BoundedSemaphore(count) for name, count in
         (("control", 2), ("logs", 32), ("status", 2), ("commands", 4))}


def send(value):
    data = json.dumps(value, separators=(",", ":")).encode()
    if len(data) > LIMIT:
        raise ValueError("helper response exceeds frame limit")
    with output_lock:
        sys.stdout.buffer.write(struct.pack("!I", len(data)) + data)
        sys.stdout.buffer.flush()


def read_exact(size):
    data = bytearray()
    while len(data) < size:
        part = sys.stdin.buffer.read(size - len(data))
        if not part:
            raise EOFError()
        data.extend(part)
    return bytes(data)


def encode(data):
    return base64.b64encode(data).decode("ascii")


def execute(command, source=None, timeout=None):
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        child = subprocess.Popen(command, shell=True, executable="/bin/sh",
                                 stdin=source or subprocess.DEVNULL,
                                 stdout=stdout, stderr=stderr, start_new_session=True)
        try:
            child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            return {"code": 124, "stdout": "", "stderr": encode(b"Remote command timed out; its outcome may be uncertain.")}
        stdout.seek(0)
        stderr.seek(0)
        result = stdout.read(CHUNK + 1)
        error = stderr.read(16384)
        if len(result) > CHUNK:
            return {"code": 125, "stdout": "", "stderr": encode(b"Remote command output exceeds 256 KiB.")}
        return {"code": child.returncode, "stdout": encode(result), "stderr": encode(error)}


def prefix_digest(file, length):
    file.seek(0)
    digest = hashlib.sha256()
    while length:
        chunk = file.read(min(CHUNK, length))
        if not chunk:
            raise RuntimeError("Remote log changed during collection; retrying.")
        digest.update(chunk)
        length -= len(chunk)
    return digest


def log_fingerprint(stat):
    # ctime also changes when a writer restores the old mtime.
    return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns


def log_chunk(request):
    path = os.path.join(os.path.expanduser("~"), request["dir"], "log")
    try:
        file = open(path, "rb", buffering=0)
    except FileNotFoundError:
        return {"missing": True}
    with file:
        stat = os.fstat(file.fileno())
        identity = "%s:%s" % (stat.st_dev, stat.st_ino)
        offset = request.get("offset", 0)
        reset = request.get("identity") != identity or stat.st_size < offset
        fingerprint = log_fingerprint(stat)
        with log_cache_lock:
            cached = log_cache.get(path)
        digest = hashlib.sha256()
        if not reset and offset:
            if cached and cached[0] == fingerprint and cached[1] == offset:
                digest = cached[2].copy()
            else:
                digest = prefix_digest(file, offset)
            reset = digest.hexdigest() != request.get("boundary")
        if reset:
            offset = 0
            digest = hashlib.sha256()
        file.seek(offset)
        data = file.read(CHUNK)
        end = offset + len(data)
        digest.update(data)
        boundary = digest.hexdigest()
        after_read = log_fingerprint(os.fstat(file.fileno()))
        if after_read != fingerprint and prefix_digest(file, end).hexdigest() != boundary:
            raise RuntimeError("Remote log changed during collection; retrying.")
        final = os.fstat(file.fileno())
        if log_fingerprint(final) == after_read:
            with log_cache_lock:
                log_cache[path] = (after_read, end, digest.copy())
                log_cache.move_to_end(path)
                while len(log_cache) > 32:
                    log_cache.popitem(last=False)
        return {"identity": identity, "reset": reset, "offset": offset,
                "data": encode(data), "boundary": boundary, "size": stat.st_size, "eof": end >= final.st_size}


def slurm_status(jobs):
    result = {}
    pending = {}
    for job in jobs:
        run, job_id = job["run"], job["job"]
        if not job_id.isdigit() or not all(c.isalnum() or c == "-" for c in run):
            raise ValueError("Invalid Slurm job identity")
        try:
            directory = job.get("dir") or os.path.join(".orx", "runs", run)
            if not os.path.isabs(directory):
                directory = os.path.join(os.path.expanduser("~"), directory)
            with open(os.path.join(directory, "exit_code")) as file:
                result[run] = "EXIT " + file.read(64).strip()
        except (OSError, UnicodeError):
            pending[job_id] = run
    if not pending:
        return result
    def probe(args):
        try:
            completed = subprocess.run(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                       stderr=subprocess.DEVNULL, timeout=15)
            return completed.returncode == 0, completed.stdout[:LIMIT].decode("utf-8", "replace")
        except (OSError, subprocess.TimeoutExpired):
            return False, ""
    available, queue = probe(["squeue", "-h", "--all", "--states=all", "-j", ",".join(pending), "-o", "%i|%T"])
    if not available:
        available, queue = probe(["squeue", "-h", "--all", "--states=all", "-u", str(os.getuid()), "-o", "%i|%T"])
    missing = dict(pending)
    if available:
        for line in queue.splitlines():
            job_id, separator, status = line.strip().partition("|")
            if separator and job_id in pending:
                result[pending[job_id]] = "SQ " + status
                missing.pop(job_id, None)
    if missing:
        accounted, accounting = probe(["sacct", "-nPX", "-j", ",".join(missing), "-o", "JobIDRaw,State", "--parsable2"])
        if accounted:
            for line in accounting.splitlines():
                job_id, separator, status = line.strip().partition("|")
                if separator and job_id in missing:
                    result[missing[job_id]] = "SA " + status.strip("|")
        for job_id, run in missing.items():
            result.setdefault(run, "GONE" if available else "UNAVAILABLE")
    return result


def dispatch(request):
    op = request["op"]
    if op == "ping":
        return {"version": VERSION}
    if op == "slurm":
        return slurm_status(request["jobs"])
    if op == "log":
        return log_chunk(request)
    if op == "exec":
        data = base64.b64decode(request.get("input", ""), validate=True)
        with tempfile.TemporaryFile() as source:
            source.write(data)
            source.seek(0)
            return execute(request["command"], source,
                           120 if request.get("lane") in ("control", "status") else None)
    if op == "upload_start":
        with upload_lock:
            for expired in [token for token, (_, touched) in uploads.items()
                            if time.monotonic() - touched > 300]:
                uploads.pop(expired)[0].close()
            if len(uploads) >= 4:
                raise ValueError("Too many pending uploads")
            token = os.urandom(24).hex()
            uploads[token] = (tempfile.TemporaryFile(), time.monotonic())
        return {"token": token}
    if op == "upload_chunk":
        data = base64.b64decode(request["data"], validate=True)
        if len(data) > CHUNK:
            raise ValueError("Upload chunk too large")
        with upload_lock:
            file, _ = uploads[request["token"]]
            file.write(data)
            uploads[request["token"]] = (file, time.monotonic())
        return {}
    if op == "upload_abort":
        with upload_lock:
            upload = uploads.pop(request["token"], None)
        if upload:
            upload[0].close()
        return {}
    if op == "upload_commit":
        with upload_lock:
            file, _ = uploads.pop(request["token"])
        with file:
            file.seek(0)
            return execute(request["command"], file,
                           120 if request.get("lane") in ("control", "status") else None)
    raise ValueError("Unsupported helper operation")


def handle(request, lane):
    try:
        send({"id": request["id"], "result": dispatch(request)})
    except Exception as error:
        send({"id": request["id"], "error": str(error)[:4096]})
    finally:
        slots[lane].release()


def main():
    sys.stdout.buffer.write(b"__ORX_SSH_HELPER_V1__\n")
    sys.stdout.buffer.flush()
    send({"version": VERSION})
    while True:
        size = struct.unpack("!I", read_exact(4))[0]
        if size > LIMIT:
            raise ValueError("Frame exceeds limit")
        request = json.loads(read_exact(size))
        op = request.get("op")
        if op == "ping":
            send({"id": request["id"], "result": {"version": VERSION}})
            continue
        lane = "logs" if op == "log" else "status" if op == "slurm" else request.get("lane", "commands")
        if lane not in slots or not slots[lane].acquire(blocking=False):
            send({"id": request["id"], "error": "Helper request queue is full"})
            continue
        lanes[lane].submit(handle, request, lane)


if __name__ == "__main__":
    try:
        main()
    except EOFError:
        pass
    except Exception as error:
        print("OpenResearch SSH helper: " + str(error), file=sys.stderr, flush=True)
    finally:
        # A lost controller must not keep the helper alive behind blocked commands.
        os._exit(0)
