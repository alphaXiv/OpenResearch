#!/usr/bin/env python3
"""Run ORX chat API compatibility through its dev-slot helper using isolated auth.

Requires a reserved empty dev slot for this worktree and downloaded V1/V2 binaries.
"""
import argparse
import json
import os
from pathlib import Path
import runpy
import subprocess
import sqlite3
import tempfile
import threading
import time

native = runpy.run_path(str(Path(__file__).with_name('test-opencode-compat.py')))
Provider, request = native['Provider'], native['request']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--v1', required=True)
    parser.add_argument('--v2', required=True)
    parser.add_argument('--v2-only', action='store_true', help='Test fresh V2 and admitted provider failure only')
    parser.add_argument('--reset-native-fixture', action='store_true', help='Archive the task slot native database before running')
    args = parser.parse_args()
    worktree = Path(__file__).resolve().parent.parent
    root = Path(tempfile.mkdtemp(prefix='orx-host-compat-', dir='/tmp'))
    fake = native['http'].server.ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    threading.Thread(target=fake.serve_forever, daemon=True).start()
    print(f'Host fixture: {root}', flush=True)
    fixture_bin = root / 'bin'
    fixture_bin.mkdir()
    for name in ['home', 'data', 'state', 'cache', 'project']:
        (root / name).mkdir()
    Provider.read_file = root / 'project' / 'fixture.txt'
    Provider.read_file.write_text('retained tool output fixture')
    opencode = fixture_bin / 'opencode'
    opencode.symlink_to(Path(args.v1).resolve())
    # The helper retains its registry HOME; only its backend gets the fixture HOME.
    cargo = fixture_bin / 'cargo'
    cargo.write_text('#!/bin/sh\nexport HOME=' + str(root / 'home') + '\nexec ' + str(Path.home() / '.cargo/bin/cargo') + ' "$@"\n')
    cargo.chmod(0o755)
    env = {'PATH': str(fixture_bin) + ':' + os.environ['PATH'], 'HOME': str(Path.home()),
           'CARGO_INCREMENTAL': '0', 'CARGO_PROFILE_DEV_DEBUG': '0', 'CARGO_PROFILE_TEST_DEBUG': '0',
           'CARGO_HOME': str(Path.home() / '.cargo'), 'RUSTUP_HOME': str(Path.home() / '.rustup'),
           'XDG_DATA_HOME': str(root / 'data'), 'XDG_STATE_HOME': str(root / 'state'), 'XDG_CACHE_HOME': str(root / 'cache'),
           'OPENCODE_CONFIG_CONTENT': json.dumps({'permission': {'question': 'allow'}, 'tools': {'question': True}, 'agent': {'build': {'tools': {'question': True}, 'permission': {'question': 'allow'}}}}),
           'OPENCODE_DB': str(root / 'native.db'), 'ANTHROPIC_API_KEY': 'fixture-key',
           'OPENCODE_DISABLE_AUTOUPDATE': '1', 'OPENCODE_DISABLE_MODELS_FETCH': '1',
           'OPENCODE_DISABLE_DEFAULT_PLUGINS': '1', 'OPENCODE_ENABLE_QUESTION_TOOL': '1', 'ORX_NO_UPDATE_CHECK': '1', 'DO_NOT_TRACK': '1'}
    helper = ['node', str(worktree / 'scripts/dev-slot.mjs')]
    def control(action):
        cmd = helper + [action, '--worktree', str(worktree)]
        if action == 'start':
            cmd += ['--db', 'empty']
        result = subprocess.run(cmd, cwd=worktree, env=env, text=True, capture_output=True, timeout=360)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        return result.stdout
    def start(binary):
        control('stop')
        opencode.unlink()
        opencode.symlink_to(Path(binary).resolve())
        output = control('start')
        print(output, flush=True)
    try:
        status = control('status')
        data = Path(next(line.split('Data:', 1)[1].strip() for line in status.splitlines() if 'Data:' in line))
        native_db=data/'agents/opencode/opencode.db'
        if native_db.exists():
            if not args.reset_native_fixture:
                raise AssertionError('Existing native fixture: pass --reset-native-fixture to archive it first')
            control('stop')
            prior=root/'prior-native'
            prior.mkdir()
            for file in native_db.parent.iterdir():
                if file.name.startswith(native_db.name):
                    file.rename(prior/file.name)
        config = Path(str(data) + '-config') / 'opencode'
        config.mkdir(parents=True, exist_ok=True)
        (config / 'opencode.json').write_text(json.dumps({'model':'anthropic/claude-sonnet-4-5','small_model':'anthropic/claude-sonnet-4-5','provider':{'anthropic':{'models':{'claude-sonnet-4-5':{'name':'Fixture Sonnet','limit':{'context':200000,'output':4096}}},'options':{'baseURL':f'http://127.0.0.1:{fake.server_port}/v1','apiKey':'fixture-key'}}},'snapshot':False}))
        port = int(next(line.rsplit(' ',1)[1] for line in status.splitlines() if 'Backend:' in line))
        url = f'http://127.0.0.1:{port}'
        start(args.v2 if args.v2_only else args.v1)
        project = request(url + '/api/projects','POST',{'name':'compat fixture','path':str(root/'project'),'githubSyncEnabled':False,'initializeGit':True})['project']['id']
        def v2_ready():
            harness = next(item for item in request(url+'/api/harnesses?refresh=1')['harnesses'] if item['id']=='opencode')
            assert harness.get('agentReady') is True, harness
            assert any(model['id']=='anthropic/claude-sonnet-4-5' for model in harness.get('models', [])), harness
            print('PASS host V2 preflight and fixture model catalog',flush=True)
        def create():
            return request(url + '/api/chat/sessions','POST',{'projectId':project,'harness':'opencode','model':'anthropic/claude-sonnet-4-5'})['session']['id']
        def history(session):
            return request(url + f'/api/chat/sessions/{session}/messages')['messages']
        def turn(session,text,expect_error=False,images=None,provider_failure=False):
            prior_count=len(history(session))
            prior_calls=len(Provider.calls)
            request(url + f'/api/chat/sessions/{session}/message','POST',{'text':text,'images':images or []})
            until=time.monotonic()+90
            while time.monotonic()<until:
                state=next(row for row in request(url+f'/api/chat/sessions?projectId={project}')['sessions'] if row['id']==session)
                rows=history(session)
                if not state.get('busy') and len(rows)>prior_count and rows[-1].get('completedAt'):
                    encoded=json.dumps(rows)
                    if provider_failure:
                        assert 'fixture provider failure' in json.dumps(rows[-1]), rows[-1]
                    elif expect_error:
                        assert 'upgraded' in json.dumps(rows[-1]).lower() or 'v2' in json.dumps(rows[-1]).lower(), rows
                        assert len(Provider.calls)==prior_calls, 'downgrade sent a model request'
                    else:
                        assert 'fixture reply' in encoded,rows
                    return rows
                time.sleep(.2)
            raise AssertionError('chat did not finish')
        def typed_form(session):
            call_start=len(Provider.calls)
            request(url+f'/api/chat/sessions/{session}/message','POST',{'text':'typed form fixture'})
            def parts(items):
                for item in items:
                    yield item
                    yield from parts(item.get('children', []))
            deadline=time.monotonic()+60
            answered=False
            while time.monotonic()<deadline:
                rows=history(session)
                pending=next((part for row in rows for part in parts(row['parts']) if part.get('prompt',{}).get('kind')=='question' and not part['prompt'].get('resolved')),None)
                if pending and not answered:
                    request(url+f'/api/chat/sessions/{session}/respond','POST',{'promptId':pending['id'],'answers':[],'note':'fixture typed answer'})
                    answered=True
                if not answered and rows and rows[-1].get('completedAt'):
                    advertised=[tool.get('name') for call in Provider.calls[call_start:] for tool in call.get('tools', [])]
                    raise AssertionError(f'native question fixture completed without prompt; advertised tools: {advertised}')
                if answered and rows[-1].get('completedAt'):
                    assert 'fixture reply' in json.dumps(rows[-1]), rows[-1]
                    assert any('fixture typed answer' in json.dumps(call.get('messages', [])) for call in Provider.calls)
                    print('PASS host native question receives typed note and continues',flush=True)
                    return
                time.sleep(.2)
            raise AssertionError('native typed question did not complete')
        session=create()
        if args.v2_only:
            v2_ready()
            turn(session,'fresh V2-only fixture prompt')
            typed_form(create())
            turn(session,'provider failure fixture',provider_failure=True)
            with sqlite3.connect('file:'+str(native_db)+'?mode=ro',uri=True) as db:
                assert db.execute("select count(*) from session_message where type='user' and json_extract(data,'$.text')='provider failure fixture'").fetchone()[0]==1
            print('PASS host fresh V2-only and admitted failure without duplicate prompt',flush=True)
            return
        before=turn(session,'old fixture prompt',images=[{'mediaType':'application/pdf','dataBase64':native['base64'].b64encode(b'%PDF-1.4\n%%EOF\n').decode(),'name':'fixture.pdf'},{'mediaType':'image/png','dataBase64':'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=','name':'fixture.png'}])
        attachments={str(file):file.read_bytes() for file in (data/'chat-attachments').iterdir()}
        assert len(attachments)>=2
        assert 'retained tool output fixture' in json.dumps(before)
        print('PASS host V1 chat',flush=True)
        typed_form(create())
        opencode.unlink()
        opencode.symlink_to(Path(args.v2).resolve())
        preflight_started=time.monotonic()
        refreshed=request(url+'/api/harnesses?refresh=1')
        elapsed=time.monotonic()-preflight_started
        assert elapsed<12, f'preflight blocked behind V1 for {elapsed:.1f}s'
        assert any(item['id']=='opencode' for item in refreshed['harnesses'])
        print(f'PASS host pre-upgrade refresh returned in {elapsed:.1f}s',flush=True)
        assert history(session)==before
        after=turn(session,'continued fixture prompt')
        assert len(after)>len(before)
        assert attachments=={file:Path(file).read_bytes() for file in attachments}
        assert 'retained tool output fixture' in json.dumps(after)
        assert any('retained tool output fixture' in json.dumps(call) and 'continued fixture prompt' in json.dumps(call) for call in Provider.calls)
        print('PASS host migration and continuation',flush=True)
        v2_ready()
        turn(create(),'new fixture prompt')
        typed_form(create())
        print('PASS host new V2 chat',flush=True)
        native_db=data/'agents/opencode/opencode.db'
        def native_state():
            with sqlite3.connect('file:'+str(native_db)+'?mode=ro',uri=True) as db:
                return db.execute('select id from session_v2 order by id').fetchall(), db.execute('select id,session_id,data from session_message order by id').fetchall()
        saved_state=native_state()
        backups=set(native_db.parent.rglob('*backup*'))
        start(args.v2)
        v2_ready()
        assert history(session)==after
        assert native_state()==saved_state
        assert set(native_db.parent.rglob('*backup*'))==backups
        print('PASS host migrated restart',flush=True)
        turn(session,'provider failure fixture',provider_failure=True)
        with sqlite3.connect('file:'+str(native_db)+'?mode=ro',uri=True) as db:
            admitted=db.execute("select count(*) from session_message where type='user' and json_extract(data,'$.text')='provider failure fixture'").fetchone()[0]
            assert admitted==1, f'provider failure submitted {admitted} times'
        saved_state=native_state()
        print('PASS host admitted provider failure without duplicate prompt',flush=True)
        opencode.unlink()
        opencode.symlink_to(Path(args.v1).resolve())
        turn(session,'downgrade fixture prompt',expect_error=True)
        assert native_state()==saved_state
        print('PASS host downgrade guard',flush=True)
        control('stop')
        archive=root/'migrated-native'
        archive.mkdir()
        for file in native_db.parent.iterdir():
            if file.name.startswith(native_db.name):
                file.rename(archive/file.name)
        start(args.v2)
        v2_ready()
        turn(create(),'fresh V2-only fixture prompt')
        print('PASS host fresh V2-only database',flush=True)
    finally:
        control('stop')
        fake.shutdown()
        fake.server_close()


if __name__=='__main__':
    main()
