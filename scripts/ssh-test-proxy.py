#!/usr/bin/env python3
"""Disposable fixture: lose one launch response after the remote command executes."""
import json
import os
import re
import struct
import subprocess
import sys
import threading
from pathlib import Path

command = os.environ['SSH_ORIGINAL_COMMAND']
bootstrap = re.search(r'sys.stdin.buffer.read\((\d+)\)', command)
if not bootstrap:
    os.execl('/bin/sh', 'sh', '-c', command)
child = subprocess.Popen(command, shell=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
launches = set()


def exact(source, count):
    result = bytearray()
    while len(result) < count:
        chunk = source.read(count - len(result))
        if not chunk:
            raise EOFError()
        result.extend(chunk)
    return bytes(result)


def frame(source):
    header = exact(source, 4)
    body = exact(source, struct.unpack('!I', header)[0])
    return header + body, json.loads(body)


def requests():
    try:
        child.stdin.write(exact(sys.stdin.buffer, int(bootstrap[1])))
        child.stdin.flush()
        while True:
            data, request = frame(sys.stdin.buffer)
            if 'date +%s > launch_time' in request.get('command', ''):
                launches.add(request['id'])
            child.stdin.write(data)
            child.stdin.flush()
    except (EOFError, BrokenPipeError):
        child.stdin.close()


threading.Thread(target=requests, daemon=True).start()
try:
    sys.stdout.buffer.write(child.stdout.readline())
    sys.stdout.buffer.flush()
    while True:
        data, response = frame(child.stdout)
        marker = Path('/tmp/drop-launch-ack')
        if response.get('id') in launches and marker.exists():
            marker.unlink()
            child.kill()
            child.wait()
            os._exit(255)
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
except (EOFError, BrokenPipeError):
    child.kill()
    child.wait()
