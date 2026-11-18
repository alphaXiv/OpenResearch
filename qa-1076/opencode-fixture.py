#!/usr/bin/env python3
"""External OpenCode fixture: one anonymous free model and no provider connections."""
import json,os,sqlite3,sys,time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
args=sys.argv[1:]
if args==['--version']: print('2.3.0'); sys.exit()
if args[:2]==['auth','login']:
    root=Path(os.environ['HOME'])
    if (root/'hold-login').exists():
        (root/'login-running').touch()
        while True: time.sleep(1)
    sys.exit(0)
if args[:1]!=['serve']: sys.exit(1)
db=sqlite3.connect(os.environ['OPENCODE_DB'])
db.executescript("""
CREATE TABLE IF NOT EXISTS session_v2 (id TEXT PRIMARY KEY, directory TEXT);
CREATE TABLE IF NOT EXISTS session_message (id TEXT,session_id TEXT,data TEXT);
CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY,value TEXT);
CREATE TABLE IF NOT EXISTS migration (id TEXT PRIMARY KEY,time_completed INTEGER);
INSERT OR IGNORE INTO migration VALUES ('20260910120000_clear_v1_session_permission',1);
""")
db.close()
data={'/api/info':{'version':'2.3.0'},'/api/plugin':{'data':[{'id':'fixture'}]},'/api/experimental/migration/v1':{'status':'completed'},'/api/model':{'data':[{'id':'free-test','providerID':'opencode','enabled':True,'cost':[{'input':0,'output':0}]}]},'/api/integration':{'data':[]}}
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        obj=data.get(self.path.split('?')[0]); self.send_response(200 if obj else 404); self.end_headers(); self.wfile.write(json.dumps(obj or {}).encode())
    def log_message(self,*args):pass
s=ThreadingHTTPServer(('127.0.0.1',0),H)
print(json.dumps({'url':'http://127.0.0.1:'+str(s.server_port)}),flush=True);s.serve_forever()
