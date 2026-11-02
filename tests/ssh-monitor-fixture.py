import base64
import hashlib
import json
import os
import re
import struct
import sys
from pathlib import Path

home = Path(os.environ['HOME'])
size = int(re.search(r'sys.stdin.buffer.read\((\d+)\)', sys.argv[1])[1])
sys.stdin.buffer.read(size)

def send(value):
    data = json.dumps(value).encode()
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
    phase = (home / 'phase').read_text()
    with (home / 'calls').open('a') as calls:
        calls.write(('exit_code' if op == 'slurm' else request.get('command', op)) + '\n')
    error = None
    if op == 'ping':
        result = {'version': 1}
    elif phase == 'outage':
        error = 'Connection unavailable; Reconnect'
    elif op == 'log':
        if phase in ('missing', 'log_failure'):
            error = 'unexpected SSH log output: proxy dial error'
        else:
            data = b'first\nsecond\n'
            offset = request.get('offset', 0)
            reset = request.get('identity') != 'fixture'
            if reset:
                offset = 0
            result = {'data': base64.b64encode(data[offset:]).decode(), 'offset': offset,
                      'identity': 'fixture', 'boundary': hashlib.sha256(data[-64:]).hexdigest(),
                      'reset': reset, 'eof': True}
    elif op == 'slurm':
        state = {'missing': 'GONE', 'running': 'SQ RUNNING', 'done': 'SA COMPLETED'}[phase]
        result = {job['run']: state for job in request['jobs']}
    else:
        value = {'missing': 'cannot assign requested address', 'running': 'RUNNING',
                 'log_failure': 'RUNNING', 'done': 'EXIT 0'}[phase]
        result = {'code': 0, 'stdout': encoded(value), 'stderr': ''}
    send({'id': request['id'], 'error': error} if error else {'id': request['id'], 'result': result})
