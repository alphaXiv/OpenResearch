"""Run the unmodified application's real WS, PTY, verification, outbox and HTTP sender. No external account/network."""
import base64,hashlib,http.client,json,os,platform,shutil,signal,socket,subprocess,sys,tempfile,threading,time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path

binary,mode=sys.argv[1:3]
binary=str(Path(binary).resolve())
assert mode in ('before','after')
events=[]
class Receiver(BaseHTTPRequestHandler):
    def do_POST(self):
        raw=self.rfile.read(int(self.headers.get('Content-Length','0')))
        if self.path=='/analytics/v1/cli-events':
            for e in json.loads(raw)['events']:
                if e['name']=='cli_harness_setup': events.append(e['properties'])
            self.send_response(202)
        else: self.send_response(404)
        self.end_headers()
    def do_GET(self):self.send_response(404);self.end_headers()
    def log_message(self,*args):pass
collector=ThreadingHTTPServer(('127.0.0.1',0),Receiver);threading.Thread(target=collector.serve_forever,daemon=True).start()
origin='http://127.0.0.1:'+str(collector.server_port)
def await_until(check, label, timeout=55):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        result=check()
        if result:return result
        time.sleep(.05)
    raise RuntimeError('timed out: '+label)
def ws(port):
    s=socket.create_connection(('127.0.0.1',port)); f=s.makefile('rb')
    key=base64.b64encode(os.urandom(16)).decode()
    req=f'GET /api/harnesses/setup?harness=opencode&action=login HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'
    s.sendall(req.encode()); status=f.readline(); assert b' 101 ' in status,status
    while f.readline().strip(): pass
    return s,f
def unused_port():
    with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
def alive(port):
    try:
        with socket.create_connection(('127.0.0.1',port),.2):return True
    except OSError:return False
def terminal(p):return p['outcome'] in ('failed','succeeded','interrupted')
def show(**items):print(json.dumps(items),flush=True)

with tempfile.TemporaryDirectory() as d:
    root=Path(d); (root/'bin').mkdir()
    exe=root/'bin/opencode'; shutil.copyfile(Path(__file__).with_name('opencode-fixture.py'),exe);exe.chmod(0o755)
    env={'HOME':d,'PATH':str(root/'bin')+':/usr/bin:/bin:/usr/sbin:/sbin','XDG_CONFIG_HOME':d+'/config','XDG_CACHE_HOME':d+'/cache','XDG_DATA_HOME':d+'/data','TMPDIR':d,
        'OPENRESEARCH_API_URL':origin,'ORX_TELEMETRY_HOST':origin,'HTTPS_PROXY':origin,'HTTP_PROXY':origin,'ALL_PROXY':origin,'NO_PROXY':'127.0.0.1,localhost','CI':'true','LANG':'C.UTF-8'}
    assert subprocess.check_output([binary,'version','--build-channel'],env=env,text=True).strip()=='production'
    show(revision=mode,platform=platform.system()+' '+platform.machine(),app='unmodified orx',transport='setup WebSocket and localhost telemetry HTTP',external_fixture='OpenCode 2.3; free model, no authenticated providers')
    servers=[]; sockets=[]
    def start():
        port=unused_port(); log=open(root/f'{port}.log','w')
        p=subprocess.Popen([binary,'up','--no-browser','--port',str(port)],env=env,cwd=d,stdin=subprocess.DEVNULL,stdout=log,stderr=log)
        servers.append(p);log.close();await_until(lambda: alive(port),'app startup');return p,port
    try:
        p,port=start();sockets.extend(ws(port))
        failure=await_until(lambda:next((e for e in events if terminal(e)),None),'login failure')
        show(scenario='free model, login returned without credentials',outcome=failure['outcome'],stage=failure['stage'],reason=failure['reason'],diagnostic=failure['errorExcerpt'])
        assert (failure['outcome'],failure['stage'],failure['reason'])==('failed','verify','not_ready'),failure
        assert failure['errorExcerpt']==('agent reported no usable state' if mode=='before' else 'agent is not signed in'),failure
        (root/'hold-login').touch()
        sockets.extend(ws(port))
        await_until(lambda:(root/'login-running').exists(),'PTY login started')
        command=await_until(lambda:next((e for e in events if e['outcome']=='command_started' and e['attemptId']!=failure['attemptId']),None),'second command_started')
        aid=command['attemptId'];p.send_signal(signal.SIGTERM);p.wait(30)
        # Relaunch the same private home to drain any event durably queued during shutdown.
        p2,port2=start()
        # The fixed command-drop can be delivered before exit, on relaunch, or still be in the outbox.
        # Inspect persisted events after a measured clean shutdown; never infer failure from missing evidence.
        def traces():
            rows=[e for e in events if e['attemptId']==aid]
            for path in (root/'config').rglob('*.json'):
                try: payload=json.loads(path.read_text())
                except (ValueError,OSError): continue
                if isinstance(payload,dict):
                    for e in payload.get('events',[]):
                        props=e.get('properties',{})
                        if e.get('name')=='cli_harness_setup' and props.get('attemptId')==aid and props not in rows:rows.append(props)
            return rows
        if mode=='after':
            await_until(lambda:any(terminal(e) for e in traces()),'observed interruption')
        rows=traces(); ends=[e for e in rows if terminal(e)]
        show(scenario='SIGTERM while PTY login still in flight; app restarted',last_stage='command',terminal_events=[(e['outcome'],e['errorExcerpt']) for e in ends])
        if mode=='after':assert [(e['outcome'],e['errorExcerpt']) for e in ends]==[('interrupted','setup command task ended before a result')],ends
        else: assert not ends,ends
        show(result='PASS',production_traffic=False)
    finally:
        for obj in sockets:
            try:obj.close()
            except OSError:pass
        for p in servers:
            if p.poll() is None:
                p.terminate()
                try:p.wait(15)
                except subprocess.TimeoutExpired:p.kill();p.wait()
        collector.shutdown()
