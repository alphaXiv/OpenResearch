import base64
import hashlib
import json
import os
import re
import struct
import sys
import time
import threading
from pathlib import Path

home = Path(os.environ['HOME'])
size = int(re.search(r'sys.stdin.buffer.read\((\d+)\)', sys.argv[1])[1])
sys.stdin.buffer.read(size)

output_lock = threading.Lock()

def send(value):
    data = json.dumps(value).encode()
    with output_lock:
        sys.stdout.buffer.write(struct.pack('!I', len(data)) + data)
        sys.stdout.buffer.flush()

def encoded(value):
    return base64.b64encode(value.encode()).decode()

sys.stdout.buffer.write(b'__ORX_SSH_HELPER_V1__\n')
send({'version': 1})
while True:
    header = sys.stdin.buffer.read(4)
    if not header:
        break
    request = json.loads(sys.stdin.buffer.read(struct.unpack('!I', header)[0]))
    op = request['op']
    with (home / 'monitor-events.jsonl').open('a') as events:
        events.write(json.dumps({'op': op, 'lane': request.get('lane'),
                                 'pid': os.getpid(), 'at': time.monotonic()}) + '\n')
    phase = (home / 'phase').read_text()
    with (home / 'calls').open('a') as calls:
        calls.write(('exit_code' if op == 'slurm' else request.get('command', op)) + '\n')
    if phase == 'slow-status' and request.get('lane') == 'status':
        def delayed(request=request):
            while not (home / 'release-status').exists():
                time.sleep(0.02)
            send({'id': request['id'], 'result': {'code': 0, 'stdout': encoded('EXIT 143'), 'stderr': ''}})
        threading.Thread(target=delayed, daemon=True).start()
        continue
    error = None
    if op == 'ping':
        result = {'version': 1}
    elif phase == 'outage':
        error = 'Connection unavailable; Reconnect'
    elif request.get('lane') == 'control':
        if phase != 'done':
            (home / 'phase').write_text('cancelled')
        if (home / 'hold-control').exists():
            def delayed_control(request=request):
                while not (home / 'release-control').exists():
                    time.sleep(0.02)
                send({'id': request['id'], 'result': {'code': 0, 'stdout': '', 'stderr': ''}})
            threading.Thread(target=delayed_control, daemon=True).start()
            continue
        result = {'code': 0, 'stdout': '', 'stderr': ''}
    elif op == 'log':
        if phase in ('missing', 'log_failure'):
            error = 'unexpected SSH log output: proxy dial error'
        else:
            data = b'x' * (1536 * 1024) if (home / 'large-log').exists() else b'first\nsecond\n'
            offset = request.get('offset', 0)
            reset = request.get('identity') != 'fixture'
            if reset:
                offset = 0
            end = min(offset + 256 * 1024, len(data))
            result = {'data': base64.b64encode(data[offset:end]).decode(), 'offset': offset,
                      'identity': 'fixture', 'boundary': hashlib.sha256(data[-64:]).hexdigest(),
                      'reset': reset, 'size': len(data), 'eof': end >= len(data)}
    elif op == 'slurm':
        state = {'missing': 'GONE', 'running': 'SQ RUNNING', 'done': 'SA COMPLETED'}[phase]
        result = {job['run']: state for job in request['jobs']}
    else:
        value = {'missing': 'cannot assign requested address', 'running': 'RUNNING',
                 'log_failure': 'RUNNING', 'done': 'EXIT 0', 'cancelled': 'EXIT 143', 'slow-status': 'RUNNING', 'dir-missing': 'MISSING'}[phase]
        result = {'code': 0, 'stdout': encoded(value), 'stderr': ''}
    send({'id': request['id'], 'error': error} if error else {'id': request['id'], 'result': result})
