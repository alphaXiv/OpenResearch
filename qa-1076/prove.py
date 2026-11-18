"""Run the unmodified application's real WS, PTY, verification, outbox and HTTP sender. No external account/network."""
import base64,hashlib,http.client,json,os,platform,shutil,signal,socket,subprocess,sys,tempfile,threading,time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path

if len(sys.argv)>1 and sys.argv[1]=='--console':
    # This controller and orx share a fresh console. Runner shells often inherit
    # the ignore-Ctrl+C bit: clear it before the child is created, and keep the
    # controller alive with an ordinary callback (callbacks are not inherited).
    import ctypes,ctypes.wintypes as w
    binary,port,home=sys.argv[2:5]; root=Path(home); k=ctypes.WinDLL('kernel32',use_last_error=True)
    assert k.SetConsoleCtrlHandler(None,False),ctypes.get_last_error()
    @ctypes.WINFUNCTYPE(w.BOOL,w.DWORD)
    def keep_controller(_):return True
    assert k.SetConsoleCtrlHandler(keep_controller,True),ctypes.get_last_error()
    with open('CONIN
binary=str(Path(binary).resolve())
assert mode in ('before','after')
win=os.name=='nt'
events=[]
received_names=[]
queue_reported=threading.Event()
class Receiver(BaseHTTPRequestHandler):
    def do_POST(self):
        raw=self.rfile.read(int(self.headers.get('Content-Length','0')))
        if self.path=='/analytics/v1/cli-events':
            for e in json.loads(raw)['events']:
                received_names.append(e['name'])
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
def ws(port,harness='opencode',action='login'):
    s=socket.create_connection(('127.0.0.1',port)); f=s.makefile('rb')
    key=base64.b64encode(os.urandom(16)).decode()
    req=f'GET /api/harnesses/setup?harness={harness}&action={action} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'
    s.sendall(req.encode()); status=f.readline(); assert b' 101 ' in status,status
    while f.readline().strip(): pass
    if win:
        # The real xterm client answers ConPTY's device-status query. Without that
        # handshake ConPTY holds the child's startup/output until the socket closes.
        def terminal_client():
            while True:
                try:
                    hdr=f.read(2)
                    if len(hdr)<2:return
                    kind=hdr[0]&15; n=hdr[1]&127
                    if n==126:n=int.from_bytes(f.read(2),'big')
                    elif n==127:n=int.from_bytes(f.read(8),'big')
                    if n>1048576:raise AssertionError('unexpected frame size')
                    data=f.read(n)
                    if kind==8:return
                    if kind==1:
                        frame=json.loads(data)
                        if frame.get('type')=='queue' and frame.get('waiting') is True:queue_reported.set()
                    reply=(bytes([27])+b'[1;1R') if kind==2 and bytes([27])+b'[6n' in data else (data if kind==9 else None)
                    if reply is not None:
                        mask=os.urandom(4);op=10 if kind==9 else 2
                        s.sendall(bytes([128|op,128|len(reply)])+mask+bytes(b^mask[i%4] for i,b in enumerate(reply)))
                except (OSError, ValueError):return
        threading.Thread(target=terminal_client,daemon=True).start()
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
    fixture=Path(__file__).with_name('opencode-fixture.py').resolve()
    if win:
        # The Windows missing-terminal evidence includes Codex. Run a native fixture inside the real ConPTY.
        marker=root/'login-running'
        source='fn main(){let a=std::env::args().collect::<Vec<_>>();if a.get(1).map(String::as_str)==Some("--version"){println!("codex-cli 0.145.0");return}if a.get(1).map(String::as_str)==Some("login") && a.len()==2{std::fs::write(r#"'+str(marker)+'"#,b"running").unwrap();loop{std::thread::park()}}}'
        shim=root/'shim.rs';shim.write_text(source)
        subprocess.run(['rustc',str(shim),'-o',str(root/'bin/codex.exe')],check=True)
    else:
        exe=root/'bin/opencode'; shutil.copyfile(fixture,exe);exe.chmod(0o755)
    env={'HOME':d,'PATH':str(root/'bin')+':/usr/bin:/bin:/usr/sbin:/sbin','XDG_CONFIG_HOME':d+'/config','XDG_CACHE_HOME':d+'/cache','XDG_DATA_HOME':d+'/data','TMPDIR':d,
        'OPENRESEARCH_API_URL':origin,'ORX_TELEMETRY_HOST':origin,'HTTPS_PROXY':origin,'HTTP_PROXY':origin,'ALL_PROXY':origin,'NO_PROXY':'127.0.0.1,localhost','CI':'true','LANG':'C.UTF-8'}
    if win:
        env.update({k:v for k,v in os.environ.items() if k.upper() in ['SYSTEMROOT','WINDIR','COMSPEC','PATHEXT','SYSTEMDRIVE','NUMBER_OF_PROCESSORS','PROCESSOR_ARCHITECTURE']})
        env.update(USERPROFILE=d, APPDATA=d+'/roaming', LOCALAPPDATA=d+'/local', TEMP=d, TMP=d, FIXTURE_PY=sys.executable,FIXTURE_SCRIPT=str(fixture))
        env['PATH']=str(root/'bin')+os.pathsep+os.path.join(os.environ['SYSTEMROOT'],'System32')
    assert subprocess.check_output([binary,'version','--build-channel'],env=env,text=True).strip()=='production'
    show(revision=mode,platform=platform.system()+' '+platform.machine(),app='unmodified orx',transport='setup WebSocket and localhost telemetry HTTP',external_fixture=('native Codex; managed install already queued' if win else 'OpenCode 2.3; free model, no authenticated providers'))
    servers=[]; sockets=[]; lock_file=None
    def start():
        port=unused_port(); log=open(root/f'{port}.log','w')
        app=([sys.executable,str(Path(__file__).resolve()),'--console',binary,str(port),d] if win else [binary,'up','--no-browser','--port',str(port)])
        p=subprocess.Popen(app,env=env,cwd=d,stdin=subprocess.DEVNULL,stdout=log,stderr=log,creationflags=(subprocess.CREATE_NEW_CONSOLE if win else 0))
        p.proof_port=port
        servers.append(p);log.close();await_until(lambda: alive(port),'app startup');return p,port
    try:
        p,port=start()
        first_id=None
        if not win:
            sockets.extend(ws(port))
            failure=await_until(lambda:next((e for e in events if terminal(e)),None),'login failure')
            show(scenario='free model, login returned without credentials',outcome=failure['outcome'],stage=failure['stage'],reason=failure['reason'],diagnostic=failure['errorExcerpt'])
            assert (failure['outcome'],failure['stage'],failure['reason'])==('failed','verify','not_ready')
            assert failure['errorExcerpt']==('agent reported no usable state' if mode=='before' else 'agent is not signed in')
            first_id=failure['attemptId']
        if win:
            # Hold the same per-installation lock that another managed setup owns. The
            # app must report its actual queue state before the test interrupts anything.
            import msvcrt
            lock=root/'config'/'openresearch'/'managed-agents.lock';lock.parent.mkdir(parents=True,exist_ok=True)
            lock_file=open(lock,'a+b');lock_file.seek(0);msvcrt.locking(lock_file.fileno(),msvcrt.LK_NBLCK,1)
            sockets.extend(ws(port,'codex','install'))
            await_until(queue_reported.is_set,'Windows managed install visibly queued')
        else:
            (root/'hold-login').touch()
            sockets.extend(ws(port))
            await_until(lambda:(root/'login-running').exists(),'native PTY login started',100)
        command=await_until(lambda:next((e for e in events if e['outcome']=='command_started' and e['attemptId']!=first_id),None),'second command_started')
        aid=command['attemptId']
        if win:
            (root/f'{port}.interrupt').touch()
        else: p.send_signal(signal.SIGTERM)
        p.wait(30)
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
            await_until(lambda:any(e['attemptId']==aid and terminal(e) for e in events),'interruption acknowledged at the loopback receiver')
        rows=traces(); ends=[e for e in rows if terminal(e)]
        show(scenario=('Ctrl+C while managed install visibly queued; app restarted' if win else 'SIGTERM while PTY login still in flight; app restarted'),last_stage='command',terminal_events=[(e['outcome'],e['errorExcerpt']) for e in ends])
        if mode=='after':assert [(e['outcome'],e['errorExcerpt']) for e in ends]==[('interrupted','setup command task ended before a result')],ends
        else: assert not ends,ends
        show(result='PASS',production_traffic=False)
    except BaseException:
        show(debug='synthetic fixture only',event_names=sorted(set(received_names)),setup_rows=[(e['outcome'],e['stage'],e['reason'],e['errorExcerpt']) for e in events],marker=(root/'login-running').exists(),queued=queue_reported.is_set(),outbox_files=sum(1 for path in root.rglob('*.json') if 'telemetry' in str(path).lower()),ctrl_sent=any(root.glob('*.sent')),shutdown_seen=any('orx up: shutting down' in f.read_text(errors='replace') for f in root.glob('*.log')),app_exit=[p.poll() for p in servers])
        raise
    finally:
        for obj in sockets:
            if isinstance(obj,socket.socket):
                try:obj.shutdown(socket.SHUT_RDWR)
                except OSError:pass
        for p in servers:
            if win and p.poll() is None:
                (root/f'{p.proof_port}.force').touch()
                try:p.wait(8)
                except subprocess.TimeoutExpired:pass
            if p.poll() is None:
                p.terminate()
                try:p.wait(15)
                except subprocess.TimeoutExpired:p.kill();p.wait()
        for obj in sockets:
            try:obj.close()
            except OSError:pass
        if lock_file is not None:lock_file.close()
        collector.shutdown()
,'rb') as con, open(root/f'{port}.log','w') as log:
        child=subprocess.Popen([binary,'up','--no-browser','--port',port],stdin=con,stdout=log,stderr=log)
        raised=False
        while child.poll() is None:
            if not raised and (root/f'{port}.interrupt').exists():
                assert k.GenerateConsoleCtrlEvent(0,0),ctypes.get_last_error()
                (root/f'{port}.sent').touch(); raised=True
            if (root/f'{port}.force').exists():child.terminate()
            time.sleep(.025)
        sys.exit(child.returncode)
binary,mode=sys.argv[1:3]
binary=str(Path(binary).resolve())
assert mode in ('before','after')
win=os.name=='nt'
events=[]
received_names=[]
queue_reported=threading.Event()
class Receiver(BaseHTTPRequestHandler):
    def do_POST(self):
        raw=self.rfile.read(int(self.headers.get('Content-Length','0')))
        if self.path=='/analytics/v1/cli-events':
            for e in json.loads(raw)['events']:
                received_names.append(e['name'])
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
def ws(port,harness='opencode',action='login'):
    s=socket.create_connection(('127.0.0.1',port)); f=s.makefile('rb')
    key=base64.b64encode(os.urandom(16)).decode()
    req=f'GET /api/harnesses/setup?harness={harness}&action={action} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'
    s.sendall(req.encode()); status=f.readline(); assert b' 101 ' in status,status
    while f.readline().strip(): pass
    if win:
        # The real xterm client answers ConPTY's device-status query. Without that
        # handshake ConPTY holds the child's startup/output until the socket closes.
        def terminal_client():
            while True:
                try:
                    hdr=f.read(2)
                    if len(hdr)<2:return
                    kind=hdr[0]&15; n=hdr[1]&127
                    if n==126:n=int.from_bytes(f.read(2),'big')
                    elif n==127:n=int.from_bytes(f.read(8),'big')
                    if n>1048576:raise AssertionError('unexpected frame size')
                    data=f.read(n)
                    if kind==8:return
                    if kind==1:
                        frame=json.loads(data)
                        if frame.get('type')=='queue' and frame.get('waiting') is True:queue_reported.set()
                    reply=(bytes([27])+b'[1;1R') if kind==2 and bytes([27])+b'[6n' in data else (data if kind==9 else None)
                    if reply is not None:
                        mask=os.urandom(4);op=10 if kind==9 else 2
                        s.sendall(bytes([128|op,128|len(reply)])+mask+bytes(b^mask[i%4] for i,b in enumerate(reply)))
                except (OSError, ValueError):return
        threading.Thread(target=terminal_client,daemon=True).start()
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
    fixture=Path(__file__).with_name('opencode-fixture.py').resolve()
    if win:
        # The Windows missing-terminal evidence includes Codex. Run a native fixture inside the real ConPTY.
        marker=root/'login-running'
        source='fn main(){let a=std::env::args().collect::<Vec<_>>();if a.get(1).map(String::as_str)==Some("--version"){println!("codex-cli 0.145.0");return}if a.get(1).map(String::as_str)==Some("login") && a.len()==2{std::fs::write(r#"'+str(marker)+'"#,b"running").unwrap();loop{std::thread::park()}}}'
        shim=root/'shim.rs';shim.write_text(source)
        subprocess.run(['rustc',str(shim),'-o',str(root/'bin/codex.exe')],check=True)
    else:
        exe=root/'bin/opencode'; shutil.copyfile(fixture,exe);exe.chmod(0o755)
    env={'HOME':d,'PATH':str(root/'bin')+':/usr/bin:/bin:/usr/sbin:/sbin','XDG_CONFIG_HOME':d+'/config','XDG_CACHE_HOME':d+'/cache','XDG_DATA_HOME':d+'/data','TMPDIR':d,
        'OPENRESEARCH_API_URL':origin,'ORX_TELEMETRY_HOST':origin,'HTTPS_PROXY':origin,'HTTP_PROXY':origin,'ALL_PROXY':origin,'NO_PROXY':'127.0.0.1,localhost','CI':'true','LANG':'C.UTF-8'}
    if win:
        env.update({k:v for k,v in os.environ.items() if k.upper() in ['SYSTEMROOT','WINDIR','COMSPEC','PATHEXT','SYSTEMDRIVE','NUMBER_OF_PROCESSORS','PROCESSOR_ARCHITECTURE']})
        env.update(USERPROFILE=d, APPDATA=d+'/roaming', LOCALAPPDATA=d+'/local', TEMP=d, TMP=d, FIXTURE_PY=sys.executable,FIXTURE_SCRIPT=str(fixture))
        env['PATH']=str(root/'bin')+os.pathsep+os.path.join(os.environ['SYSTEMROOT'],'System32')
    assert subprocess.check_output([binary,'version','--build-channel'],env=env,text=True).strip()=='production'
    show(revision=mode,platform=platform.system()+' '+platform.machine(),app='unmodified orx',transport='setup WebSocket and localhost telemetry HTTP',external_fixture=('native Codex; managed install already queued' if win else 'OpenCode 2.3; free model, no authenticated providers'))
    servers=[]; sockets=[]; lock_file=None
    def start():
        port=unused_port(); log=open(root/f'{port}.log','w')
        p=subprocess.Popen([binary,'up','--no-browser','--port',str(port)],env=env,cwd=d,stdin=subprocess.DEVNULL,stdout=log,stderr=log,creationflags=(subprocess.CREATE_NEW_CONSOLE if win else 0))
        servers.append(p);log.close();await_until(lambda: alive(port),'app startup');return p,port
    try:
        p,port=start()
        first_id=None
        if not win:
            sockets.extend(ws(port))
            failure=await_until(lambda:next((e for e in events if terminal(e)),None),'login failure')
            show(scenario='free model, login returned without credentials',outcome=failure['outcome'],stage=failure['stage'],reason=failure['reason'],diagnostic=failure['errorExcerpt'])
            assert (failure['outcome'],failure['stage'],failure['reason'])==('failed','verify','not_ready')
            assert failure['errorExcerpt']==('agent reported no usable state' if mode=='before' else 'agent is not signed in')
            first_id=failure['attemptId']
        if win:
            # Hold the same per-installation lock that another managed setup owns. The
            # app must report its actual queue state before the test interrupts anything.
            import msvcrt
            lock=root/'config'/'openresearch'/'managed-agents.lock';lock.parent.mkdir(parents=True,exist_ok=True)
            lock_file=open(lock,'a+b');lock_file.seek(0);msvcrt.locking(lock_file.fileno(),msvcrt.LK_NBLCK,1)
            sockets.extend(ws(port,'codex','install'))
            await_until(queue_reported.is_set,'Windows managed install visibly queued')
        else:
            (root/'hold-login').touch()
            sockets.extend(ws(port))
            await_until(lambda:(root/'login-running').exists(),'native PTY login started',100)
        command=await_until(lambda:next((e for e in events if e['outcome']=='command_started' and e['attemptId']!=first_id),None),'second command_started')
        aid=command['attemptId']
        if win:
            # Attach a helper to this application's private console; do not send Ctrl+C to the CI runner.
            code='import ctypes,sys,time; k=ctypes.windll.kernel32; k.FreeConsole(); assert k.AttachConsole(int(sys.argv[1])); assert k.SetConsoleCtrlHandler(None,1); assert k.GenerateConsoleCtrlEvent(0,0); time.sleep(.25); k.FreeConsole()'
            subprocess.run([sys.executable,'-c',code,str(p.pid)],check=True)
        else: p.send_signal(signal.SIGTERM)
        p.wait(30)
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
            await_until(lambda:any(e['attemptId']==aid and terminal(e) for e in events),'interruption acknowledged at the loopback receiver')
        rows=traces(); ends=[e for e in rows if terminal(e)]
        show(scenario=('Ctrl+C while managed install visibly queued; app restarted' if win else 'SIGTERM while PTY login still in flight; app restarted'),last_stage='command',terminal_events=[(e['outcome'],e['errorExcerpt']) for e in ends])
        if mode=='after':assert [(e['outcome'],e['errorExcerpt']) for e in ends]==[('interrupted','setup command task ended before a result')],ends
        else: assert not ends,ends
        show(result='PASS',production_traffic=False)
    except BaseException:
        show(debug='synthetic fixture only',event_names=sorted(set(received_names)),setup_rows=[(e['outcome'],e['stage'],e['reason'],e['errorExcerpt']) for e in events],marker=(root/'login-running').exists(),queued=queue_reported.is_set(),outbox_files=sum(1 for path in root.rglob('*.json') if 'telemetry' in str(path).lower()),app_exit=[p.poll() for p in servers])
        raise
    finally:
        for obj in sockets:
            if isinstance(obj,socket.socket):
                try:obj.shutdown(socket.SHUT_RDWR)
                except OSError:pass
        for p in servers:
            if p.poll() is None:
                p.terminate()
                try:p.wait(15)
                except subprocess.TimeoutExpired:p.kill();p.wait()
        for obj in sockets:
            try:obj.close()
            except OSError:pass
        if lock_file is not None:lock_file.close()
        collector.shutdown()
