"""Linux package check: real app, real HTML preview, no second webview program."""
import json
import os
import re
from pathlib import Path
import signal
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request

image = str(Path(sys.argv[1]).resolve())
mode = sys.argv[2]
assert mode in tuple(runtime + suffix for runtime in ('system', 'bundled') for suffix in ('', '-disabled', '-abort', '-hang'))
fault = mode.split('-')[1] if mode.endswith(('-abort', '-hang')) else None
fault_dir = str(Path(sys.argv[3]).resolve()) if fault else None
disabled = mode.endswith('-disabled') or fault is not None
runtime = mode.split('-')[0]
root = Path(__file__).resolve().parent.parent
state = Path(tempfile.mkdtemp(prefix='orx-appimage-media-'))
env = os.environ.copy()
for key in ('WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'GDK_BACKEND', 'LD_LIBRARY_PATH', 'DBUS_SESSION_BUS_ADDRESS'):
    env.pop(key, None)
for key, directory in (('HOME', 'home'), ('ORX_DATA_DIR', 'data'), ('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'share'), ('XDG_CACHE_HOME', 'cache'), ('XDG_RUNTIME_DIR', 'runtime')):
    env[key] = str(state / directory)
    (state / directory).mkdir(mode=0o700)
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
origin = f'http://127.0.0.1:{port}'
env.update(SHELL='/bin/sh', ORX_APP_RELAUNCH_PORT=str(port), ORX_NO_UPDATE_CHECK='1', ORX_TELEMETRY_ENV='off', APPIMAGE_EXTRACT_AND_RUN='1', TMPDIR=str(state), GTK_THEME='Adwaita:dark', APPIMAGE_GTK_THEME='Adwaita:light', WEBKIT_SHOW_CONSOLE_MESSAGES='1', XDG_DATA_DIRS='/usr/local/share:/usr/share')
process = None
log = None

def api(path, body=None):
    request = urllib.request.Request(origin + path, data=None if body is None else json.dumps(body).encode(), headers={'Content-Type':'application/json', 'Origin':origin})
    return json.load(urllib.request.urlopen(request, timeout=10))

def stop():
    global process, log
    if process:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
        except ProcessLookupError:
            pass
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        # A child can outlive xvfb-run after TERM.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process = None
    if log:
        log.close()
        log = None

def start():
    global process, log
    log = (state / 'app.log').open('w')
    process = subprocess.Popen(['xvfb-run', '-a', '-s', '-screen 0 1440x1000x24', image], env=env, stdout=log, stderr=log, start_new_session=True)
    for _ in range(100):
        try:
            if api('/api/health')['ok']:
                return
        except (OSError, ValueError):
            pass
        assert process.poll() is None, 'App exited before health check'
        time.sleep(0.2)
    raise AssertionError('Dashboard never answered')

def app_process():
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            raw = (entry/'environ').read_bytes().split(b'\0')
            if ('XDG_RUNTIME_DIR='+env['XDG_RUNTIME_DIR']).encode() not in raw:
                continue
            args = (entry/'cmdline').read_bytes().split(b'\0')
            if len(args)>1 and args[1] == b'app' and args[0].endswith(b'/usr/bin/orx'):
                return entry, dict(item.decode().split('=',1) for item in raw if b'=' in item)
        except (OSError, UnicodeError):
            pass
    raise AssertionError('No owned native app process')

try:
    start()
    proc, actual_env = app_process()
    exe = str((proc/'exe').resolve())
    supported = subprocess.run([exe, 'check-media', '--help'], env=actual_env, cwd=proc/'cwd', stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
    if not supported:
        assert actual_env['ORX_APPIMAGE_RUNTIME'] == runtime
        for _ in range(100):
            visible = subprocess.run(['xdotool','search','--onlyvisible','--class','^OpenResearch$'],env=actual_env,capture_output=True)
            if visible.returncode == 0:
                break
            time.sleep(0.2)
        assert visible.returncode == 0, 'Older release has no visible dashboard'
        print('::notice::Older release dashboard checked; own-window media and degradation checks require check-media support.')
        sys.exit(0)
    project_dir = state/'home'/'media-project'
    project = api('/api/projects', {'name':'Media runtime check','path':str(project_dir),'createFolder':True,'requireNewFolder':True,'initializeGit':True})['project']
    fixture = root/'scripts/fixtures/appimage-media.html'
    shutil.copy2(fixture, project_dir/'appimage-media.html')
    pane = {'kind':'file','path':'appimage-media.html','source':'repo'}
    location = f'/projects/{project["id"]}/tasks/new?' + urllib.parse.urlencode({'pane':json.dumps(pane)})
    workspace = {'lastLocation':location,'railOpen':True,'panelWidth':850,'experimentsView':'table'}
    api('/api/settings/ui-state', {'tourCompleted':True,'workspace':workspace})
    stop()
    with sqlite3.connect(state/'data'/'orx.db') as db:
        db.execute('update ui_state set onboarding_completed=1, workspace_state_json=? where id=1', (json.dumps(workspace),))
    if fault:
        assert (Path(fault_dir)/'libgstorxprobe.so').is_file(), 'Missing media fault fixture'
        # Run the external plugin's initializer inside the real media-check child.
        env.update(ORX_MEDIA_FAULT=fault, GST_PLUGIN_PATH_1_0=fault_dir, GST_PLUGIN_SYSTEM_PATH_1_0='', GST_REGISTRY_FORK='yes', GST_REGISTRY=str(state/'fault-registry.bin'))
        scanner = Path('/usr/lib', os.uname().machine+'-linux-gnu', 'gstreamer1.0/gstreamer-1.0/gst-plugin-scanner')
        if scanner.is_file():
            env['GST_PLUGIN_SCANNER_1_0'] = str(scanner)
    start()
    proc, actual_env = app_process()
    assert actual_env['ORX_APPIMAGE_RUNTIME'] == runtime, actual_env.get('ORX_APPIMAGE_RUNTIME')
    expected_theme = 'Adwaita:dark' if runtime == 'system' else 'Adwaita:light'
    assert actual_env['GTK_THEME'] == expected_theme
    assert actual_env['ORX_HOST_LD_LIBRARY_PATH'] == '-'
    assert actual_env['ORX_HOST_GDK_BACKEND'] == '-'
    assert 'GDK_BACKEND' not in actual_env
    windows = []
    for _ in range(100):
        found = subprocess.run(['xdotool','search','--onlyvisible','--class','^OpenResearch$'], env=actual_env, text=True, capture_output=True)
        windows = found.stdout.splitlines()
        if windows:
            break
        assert process.poll() is None, 'App exited while opening HTML'
        time.sleep(0.2)
    assert windows, 'Native window not visible'
    if fault == 'hang':
        hung = re.search(r'ORX fault fixture: pid=(\d+) mode=hang', (state/'app.log').read_text(errors='replace'))
        assert hung and Path('/proc', hung[1]).exists(), 'Timed-out media check was not given time to finish scanning'
    screenshot = state/'native.png'
    green_pixels = 0
    for _ in range(100):
        subprocess.run(['import','-window', windows[-1], str(screenshot)], env=actual_env, check=True)
        # Both the video and the canvas drawn after Web Audio initialization must be green.
        histogram = subprocess.check_output(['convert', str(screenshot), '-depth','8','-format','%c','histogram:info:-'],text=True)
        colors = [tuple(map(int, match)) for match in re.findall(r"^\s*(\d+):\s*\(\s*(\d+),\s*(\d+),\s*(\d+)", histogram, re.MULTILINE)]
        green_pixels = sum(count for count,r,g,b in colors if g>90 and r<20 and b<20)
        blue_pixels = sum(count for count,r,g,b in colors if b>230 and r<20 and g<20)
        if blue_pixels > 100 and (disabled or green_pixels > 20000):
            break
        assert process.poll() is None, 'App exited while decoding the video'
        time.sleep(0.2)
    text = (state/'app.log').read_text(errors='replace')
    assert ('audio/video disabled:' in text) == disabled, text[-4000:]
    if fault:
        initialized = re.search(r'ORX fault fixture: pid=(\d+) mode='+fault, text)
        assert initialized, 'Fault plugin was not initialized'
        if fault == 'hang':
            # The app opens first; the check keeps scanning for SLOW_SCAN_GRACE (media.rs).
            for _ in range(200):
                if not Path('/proc', initialized[1]).exists():
                    break
                time.sleep(0.2)
        assert not Path('/proc', initialized[1]).exists(), 'Media-check child was not reaped'
        assert ('signal: 6' if fault == 'abort' else 'Media check timed out') in text, text[-4000:]
    assert blue_pixels > 100, 'HTML report did not render'
    if not disabled:
        assert green_pixels > 20000, f'No decoded video frame: {green_pixels} green pixels'
    else:
        assert green_pixels == 0, 'Disabled media unexpectedly decoded a frame'
    result = {'enabled':not disabled,'decodedGreenPixels':green_pixels,'htmlMarkerPixels':blue_pixels}
    assert api('/api/health')['ok']
    evidence = os.environ.get('ORX_SMOKE_EVIDENCE_DIR')
    if evidence:
        out = Path(evidence)
        out.mkdir(parents=True, exist_ok=True)
        shutil.copy2(screenshot, out/f'{mode}.png')
        shutil.copy2(state/'app.log', out/f'{mode}.log')
    helpers = 0
    for helper in Path('/proc').iterdir():
        try:
            if ('XDG_RUNTIME_DIR='+env['XDG_RUNTIME_DIR']).encode() not in (helper/'environ').read_bytes().split(b'\0'):
                continue
            if 'WebKitWebProcess' not in str((helper/'exe').resolve()):
                continue
            helpers += 1
            maps=(helper/'maps').read_text()
            bundled=any('appimage_extracted_' in line and 'libwebkit2gtk' in line for line in maps.splitlines())
            assert bundled == (runtime=='bundled'), 'Wrong WebKit loaded'
        except (OSError, ValueError):
            continue
    assert helpers > 0, 'No owned WebKit web process'
    print(json.dumps({'mode':mode,'dashboard':True,'nativeWindow':True,'media':result}))
except BaseException:
    if (state/'app.log').exists():
        print((state/'app.log').read_text(errors='replace')[-10000:], file=sys.stderr)
    raise
finally:
    stop()
    shutil.rmtree(state)
