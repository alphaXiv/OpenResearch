#!/usr/bin/env python3
"""Verify native auxiliary routing with real V1/V2 binaries and a loopback provider."""
import argparse
import json
from pathlib import Path
import runpy
import tempfile
import threading

native = runpy.run_path(str(Path(__file__).with_name('test-opencode-compat.py')))
Provider, request, wait_for = native['Provider'], native['request'], native['wait_for']
SELECTED = 'claude-haiku-4-5'
DEFAULT = 'claude-sonnet-4-5'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--v1', required=True)
    parser.add_argument('--v2', required=True)
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix='orx-routing-'))
    Provider.read_file = root / 'fixture.txt'
    Provider.read_file.write_text('routing fixture')
    fake = native['http'].server.ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=fake.serve_forever, daemon=True).start()
    provider = f'http://127.0.0.1:{fake.server_port}/v1'
    try:
        for generation, binary in [(1, args.v1), (2, args.v2)]:
            key = 'agent' if generation == 1 else 'agents'
            inline = {key: {name: {'model': f'anthropic/{SELECTED}'} for name in ['title', 'compaction']}}
            if generation == 1:
                inline['small_model'] = f'anthropic/{SELECTED}'
            configured = {
                'provider': {'anthropic': {
                    'models': {model: {'limit': {'context': 200000, 'output': 4096}} for model in [DEFAULT, SELECTED]},
                    'options': {'baseURL': provider, 'apiKey': 'fixture-key'},
                }},
                key: {name: {'model': f'anthropic/{DEFAULT}'} for name in ['title', 'compaction']},
            }
            with native['server'](str(Path(binary).resolve()), root / f'v{generation}', generation, provider,
                                  config_overrides=configured, inline=inline) as (url, process, log):
                def agents():
                    body = request(url + ('/agent' if generation == 1 else '/api/agent'))
                    return body if generation == 1 else body['data']
                def configured_agents():
                    listed = agents()
                    selected = [agent for agent in listed if agent['name' if generation == 1 else 'id'] in ['title', 'compaction']]
                    return listed if len(selected) == 2 and all(agent.get('model') for agent in selected) else None
                listed = wait_for(configured_agents, process, log)
                for name in ['title', 'compaction']:
                    agent = next(agent for agent in listed if agent['name' if generation == 1 else 'id'] == name)
                    assert agent['model']['providerID'] == 'anthropic', agent
                    assert agent['model']['modelID' if generation == 1 else 'id'] == SELECTED, agent
                Provider.calls.clear()
                Provider.reply = '## Objective\n- Preserve the selected model routing fixture.'
                text = 'routing fixture ' * 6000
                if generation == 1:
                    session = request(url + '/session', 'POST', {})['id']
                    request(f'{url}/session/{session}/message', 'POST', {
                        'parts': [{'type': 'text', 'text': text}],
                        'model': {'providerID': 'anthropic', 'modelID': SELECTED},
                    })
                    wait_for(lambda: len(Provider.calls) >= 2, process, log)
                    before = len(Provider.calls)
                    request(f'{url}/session/{session}/summarize', 'POST', {'providerID': 'anthropic', 'modelID': SELECTED})
                else:
                    session = request(url + '/api/session', 'POST', {
                        'model': {'providerID': 'anthropic', 'id': SELECTED},
                        'location': {'directory': str(root / 'v2' / 'project')},
                    })['data']['id']
                    native['prompt_v2'](url, session, text, process, log)
                    wait_for(lambda: len(Provider.calls) >= 2, process, log)
                    before = len(Provider.calls)
                    item = request(f'{url}/api/session/{session}/compact', 'POST', {})['data']['id']
                    def compacted():
                        try:
                            message = request(f'{url}/api/session/{session}/message/{item}')['data']
                        except AssertionError:
                            return None
                        if message.get('status') == 'failed':
                            raise RuntimeError(json.dumps(message))
                        return message if message.get('status') == 'completed' else None
                    wait_for(compacted, process, log)
                assert len(Provider.calls) > before, 'compaction made no generation request'
                models = [call.get('model') for call in Provider.calls]
                assert models and set(models) == {SELECTED}, models
                (root / f'v{generation}-request-models.json').write_text(json.dumps(models))
                print(f'PASS V{generation}: chat, native title, compaction all used {SELECTED}; {len(models)} observed requests', flush=True)
                Provider.reply = 'fixture reply'
    finally:
        fake.shutdown()
        fake.server_close()
    print(f'Evidence: {root}', flush=True)


if __name__ == '__main__':
    main()
