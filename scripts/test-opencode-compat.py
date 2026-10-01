#!/usr/bin/env python3
"""Exercise installed V1/V2 binaries against disposable data and a local fake provider.

Usage: python3 scripts/test-opencode-compat.py --v1 /path/to/v1 --v2 /path/to/v2
No user configuration, credentials, or databases are inherited.
"""
import argparse
import base64
import contextlib
import http.server
import json
import hashlib
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request


def request(url, method='GET', body=None):
    req = urllib.request.Request(url, method=method, data=None if body is None else json.dumps(body).encode(), headers={
        'Content-Type': 'application/json',
        'Authorization': 'Basic ' + base64.b64encode(b'opencode:fixture-password').decode(),
    })
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            content = response.read()
            return json.loads(content) if content else None
    except urllib.error.HTTPError as error:
        raise AssertionError(f'{method} {url}: {error.code} {error.read().decode()}') from error


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        Provider.calls.append(body)
        if 'provider failure fixture' in json.dumps(body.get('messages', [])[-1:]):
            self.send_response(400)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'type': 'error', 'error': {'type': 'invalid_request_error', 'message': 'fixture provider failure'}}).encode())
            return
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        events = [
            ('message_start', {'type': 'message_start', 'message': {'id': 'msg_fixture', 'type': 'message', 'role': 'assistant', 'model': body.get('model'), 'content': [], 'stop_reason': None, 'stop_sequence': None, 'usage': {'input_tokens': 10, 'output_tokens': 0}}}),
            ('content_block_start', {'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'text', 'text': ''}}),
            ('content_block_delta', {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'text_delta', 'text': 'fixture reply'}}),
            ('content_block_stop', {'type': 'content_block_stop', 'index': 0}),
            ('message_delta', {'type': 'message_delta', 'delta': {'stop_reason': 'end_turn', 'stop_sequence': None}, 'usage': {'output_tokens': 3}}),
            ('message_stop', {'type': 'message_stop'}),
        ]
        serialized = json.dumps(body.get('messages', []))
        if 'read' in [tool.get('name') for tool in body.get('tools', [])] and 'tool_result' not in serialized and 'old fixture prompt' in serialized:
            events[1] = ('content_block_start', {'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'tool_use', 'id': 'tool_fixture', 'name': 'read', 'input': {}}})
            events[2] = ('content_block_delta', {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'input_json_delta', 'partial_json': json.dumps({'filePath': str(Provider.read_file)})}})
            events[4][1]['delta']['stop_reason'] = 'tool_use'
        if 'question' in [tool.get('name') for tool in body.get('tools', [])] and 'typed form fixture' in json.dumps(body.get('messages', [])[-1:]):
            events[1] = ('content_block_start', {'type': 'content_block_start', 'index': 0, 'content_block': {'type': 'tool_use', 'id': 'tool_fixture_question', 'name': 'question', 'input': {}}})
            events[2] = ('content_block_delta', {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'input_json_delta', 'partial_json': json.dumps({'questions': [{'header': 'Fixture', 'question': 'Enter fixture answer', 'options': []}]})}})
            events[4][1]['delta']['stop_reason'] = 'tool_use'
        for kind, event in events:
            self.wfile.write(f'event: {kind}\ndata: {json.dumps(event)}\n\n'.encode())
        self.wfile.flush()


Provider.calls = []


def wait_for(check, process, log):
    until = time.monotonic() + 60
    last = None
    while time.monotonic() < until:
        if process.poll() is not None:
            raise AssertionError(f'server exited: {log.read_text()}')
        try:
            value = check()
            if value:
                return value
        except (OSError, AssertionError) as error:
            last = error
        time.sleep(0.1)
    raise AssertionError(f'timeout: {last}\n{log.read_text()[-8000:]}')


@contextlib.contextmanager
def server(binary, root, generation, provider):
    for name in ['home', 'data', 'config', 'state', 'cache', 'project']:
        (root / name).mkdir(parents=True, exist_ok=True)
    config = root / 'config' / 'opencode'
    config.mkdir(exist_ok=True)
    (config / 'opencode.json').write_text(json.dumps({
        'model': 'anthropic/claude-sonnet-4-5', 'small_model': 'anthropic/claude-sonnet-4-5',
        'provider': {'anthropic': {'models': {'claude-sonnet-4-5': {'name': 'Fixture Sonnet', 'limit': {'context': 200000, 'output': 4096}}}, 'options': {'baseURL': provider, 'apiKey': 'fixture-key'}}},
        'permission': {'*': 'allow'}, 'snapshot': False,
    }))
    env = {'PATH': os.environ['PATH'], 'HOME': str(root / 'home'),
           'XDG_DATA_HOME': str(root / 'data'), 'XDG_CONFIG_HOME': str(root / 'config'),
           'XDG_STATE_HOME': str(root / 'state'), 'XDG_CACHE_HOME': str(root / 'cache'),
           'OPENCODE_DB': str(root / 'opencode.db'), 'OPENCODE_PASSWORD': 'fixture-password',
           'OPENCODE_SERVER_PASSWORD': 'fixture-password', 'ANTHROPIC_API_KEY': 'fixture-key',
           'OPENCODE_DISABLE_AUTOUPDATE': '1', 'OPENCODE_DISABLE_MODELS_FETCH': '1',
           'OPENCODE_CONFIG_PROJECT_DISABLE': '1', 'OPENCODE_DISABLE_PROJECT_CONFIG': '1',
           'OPENCODE_DISABLE_DEFAULT_PLUGINS': '1'}
    version = subprocess.run([binary, '--version'], env=env, cwd=root / 'project', capture_output=True, text=True, check=True, timeout=30).stdout.strip()
    with socket.socket() as port_socket:
        port_socket.bind(('127.0.0.1', 0))
        port = port_socket.getsockname()[1]
    log = root / f'{generation}-server.log'
    with log.open('w') as output:
        process = subprocess.Popen([binary, 'serve', '--hostname', '127.0.0.1', '--port', str(port)] + (['--stdio'] if generation == 2 else []), env=env, cwd=root / 'project', stdout=output, stderr=output, stdin=subprocess.PIPE)
        try:
            url = f'http://127.0.0.1:{port}'
            # V2's readiness route moved: /api/health (<=2.0.3) -> /api/status
            # (2.0.4-2.0.5) -> /api/info (2.0.6+). The SPA fallback can serve
            # HTML with a 200 on unknown routes, so require a JSON `version`.
            def healthy():
                if generation == 1:
                    return request(url + '/global/health')
                last = None
                for path in ['/api/info', '/api/status', '/api/health']:
                    try:
                        body = request(url + path)
                    except (AssertionError, ValueError) as error:
                        last = error
                        continue
                    if isinstance(body, dict) and isinstance(body.get('version'), str):
                        return body
                if last:
                    raise last
            wait_for(healthy, process, log)
            print(f'V{generation} ready: {version}', flush=True)
            if generation == 1:
                original_sessions = request(url + '/session')
                result = subprocess.run([binary, 'run', '--agent', 'plan', '--model', 'anthropic/claude-sonnet-4-5', 'title fixture prompt'], env={**env, 'OPENCODE_DB': ':memory:'}, cwd=root / 'project', capture_output=True, text=True, check=True, timeout=60)
                assert 'fixture reply' in result.stdout, result
                assert request(url + '/session') == original_sessions
                print('PASS V1 memory-only plan leaves native sessions unchanged', flush=True)
            if generation == 2:
                assert request(url + '/api/session/active') == {'data': {}}
                if root.name == 'fresh':
                    help_result = subprocess.run([binary, 'auth', 'login', '--help'], env=env, cwd=root / 'project', capture_output=True, text=True, check=True, timeout=30)
                    assert 'Login' in help_result.stdout or 'Connect' in help_result.stdout or 'authenticate' in help_result.stdout.lower(), help_result.stdout
                    result = subprocess.run([binary, 'run', '--server', url, '--agent', 'plan', '--model', 'anthropic/claude-sonnet-4-5', 'title fixture prompt'], env=env, cwd=root / 'project', capture_output=True, text=True, check=True, timeout=60)
                    print('CLI plan output:', repr(result.stdout), flush=True)
                    assert 'fixture reply' in result.stdout, result
            yield url, process, log
        finally:
            if generation == 2:
                process.stdin.close()
            else:
                process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def messages(url, session):
    return request(f'{url}/api/session/{session}/message?limit=200&order=asc')['data']


def prompt_v2(url, session, text, process, log):
    before = len(messages(url, session))
    request(f'{url}/api/session/{session}/prompt', 'POST', {'text': text})
    # 2.0.6+ appends a synthetic {"type": "idle"} entry when a run finishes, so
    # the completed assistant reply is not necessarily the last item.
    return wait_for(lambda: next((m for m in messages(url, session)[before:] if m['type'] == 'assistant' and m['time'].get('completed')), None), process, log)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--v1', required=True)
    parser.add_argument('--v2', required=True)
    args = parser.parse_args()
    args.v1, args.v2 = str(Path(args.v1).resolve()), str(Path(args.v2).resolve())
    root = Path(tempfile.mkdtemp(prefix='orx-opencode-compat-', dir='/tmp'))
    print(f'Isolated artifacts: {root}', flush=True)
    fake = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=fake.serve_forever, daemon=True).start()
    provider = f'http://127.0.0.1:{fake.server_port}/v1'
    try:
        attachments = root / 'chat-attachments'
        attachments.mkdir()
        image = attachments / 'fixture.png'
        image.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='))
        pdf = attachments / 'fixture.pdf'
        pdf.write_bytes(b'%PDF-1.4\n% attachment retention fixture\n%%EOF\n')
        Provider.read_file = attachments / 'read-result.txt'
        Provider.read_file.write_text('retained tool output fixture')
        hashes = {str(file): hashlib.sha256(file.read_bytes()).hexdigest() for file in attachments.iterdir()}
        old_text = f'old fixture prompt\n<attached-files>\n{pdf}\n{image}\n</attached-files>'
        with server(args.v1, root / 'upgrade', 1, provider) as (url, process, log):
            empty_session = request(url + '/session', 'POST', {'title': 'empty migration fixture'})['id']
            session = request(url + '/session', 'POST', {'title': 'compatibility fixture'})['id']
            request(f'{url}/session/{session}/message', 'POST', {'parts': [{'type': 'text', 'text': old_text}], 'model': {'providerID': 'anthropic', 'modelID': 'claude-sonnet-4-5'}})
            original = request(f'{url}/session/{session}/message')
            old_ids = [message['info']['id'] for message in original]
            assert len(old_ids) >= 2, original
            assert 'retained tool output fixture' in json.dumps(original), original
        with server(args.v2, root / 'upgrade', 2, provider) as (url, process, log):
            wait_for(lambda: request(url + '/api/experimental/migration/v1')['status'] == 'completed', process, log)
            assert request(f'{url}/api/session/{empty_session}')['data']['id'] == empty_session
            assert messages(url, empty_session) == []
            print('PASS empty V1 session preserved by native migration', flush=True)
            assert request(f'{url}/api/session/{session}')['data']['id'] == session
            migrated = messages(url, session)
            assert [message['id'] for message in migrated] == old_ids, migrated
            assert migrated[0]['text'] == old_text
            assert 'retained tool output fixture' in json.dumps(migrated), migrated
            prompt_v2(url, session, 'continued fixture prompt', process, log)
            created = request(url + '/api/session', 'POST', {'title': 'new V2 fixture', 'location': {'directory': str(root / 'upgrade' / 'project')}})['data']['id']
            prompt_v2(url, created, 'new fixture prompt', process, log)
            # Export moved under /api/experimental when /api/status arrived
            # (2.0.4+); the /api/health-era layout serves it at /api. The SPA
            # fallback can answer the wrong prefix with HTML (ValueError).
            try:
                exported = request(f'{url}/api/experimental/session/{session}/export')['data']
            except (AssertionError, ValueError):
                exported = request(f'{url}/api/session/{session}/export')['data']
            assert exported['info']['id'] == session
            assert [message['id'] for message in exported['messages']] == [message['id'] for message in messages(url, session)]
            assert request(f'{url}/api/session/{session}/inbox') == {'data': []}
            final_ids = [message['id'] for message in messages(url, session)]
        with server(args.v2, root / 'upgrade', 2, provider) as (url, process, log):
            assert request(url + '/api/experimental/migration/v1')['status'] == 'completed'
            assert [message['id'] for message in messages(url, session)] == final_ids
        with server(args.v2, root / 'fresh', 2, provider) as (url, process, log):
            for route in ['model', 'provider', 'integration']:
                catalog = request(url + '/api/' + route)
                (root / (route + '.json')).write_text(json.dumps(catalog))
            created = request(url + '/api/session', 'POST', {'location': {'directory': str(root / 'fresh' / 'project')}})['data']['id']
            prompt_v2(url, created, 'fresh fixture prompt', process, log)
        assert len(Provider.calls) >= 4
        assert hashes == {file: hashlib.sha256(Path(file).read_bytes()).hexdigest() for file in hashes}
        assert any('retained tool output fixture' in json.dumps(call) and 'continued fixture prompt' in json.dumps(call) for call in Provider.calls)
        print('PASS: V1 history, in-place migration, V2 continuation/new/fresh sessions, migrated restart')
    finally:
        fake.shutdown()
        fake.server_close()


if __name__ == '__main__':
    main()
