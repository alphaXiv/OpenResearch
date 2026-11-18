import assert from 'node:assert/strict'; import http from 'node:http'; import fs from 'node:fs'; import os from 'node:os'; import path from 'node:path'; import {spawn,execFileSync} from 'node:child_process'; import {once} from 'node:events'; import {setTimeout as delay} from 'node:timers/promises'; import {DatabaseSync} from 'node:sqlite'; import readline from 'node:readline';
const win=process.platform==='win32',ext=win?'.exe':'',exe=n=>n+ext;
const sha='34e1eb2985a59869ef731ac2dc2310eae4e61f56';
const out=path.resolve('receipts');fs.mkdirSync(out,{recursive:true}); const verified=[];
function findFile(dir,name){for(const e of fs.readdirSync(dir,{withFileTypes:true})){const p=path.join(dir,e.name);if(e.isFile()&&e.name===name)return p;if(e.isDirectory()){const f=findFile(p,name);if(f)return f;}}}
async function until(fn,label,ms=90000){const end=Date.now()+ms;while(Date.now()<end){const v=await fn();if(v)return v;await delay(120);}throw Error('Timed out: '+label);}
for(const [phase,bin] of [['released',findFile(process.env.PROOF_BASELINE,exe('orx'))],['candidate',path.resolve('target/debug',exe('orx'))]]) {
 assert.ok(bin&&fs.existsSync(bin));const home=fs.mkdtempSync(path.join(os.tmpdir(),'orxrt-')),bind=path.join(home,'bin');
 for(const d of ['bin','tmp','projects','state','xdg-config','xdg-cache','xdg-data','roaming','local'])fs.mkdirSync(path.join(home,d));
 for(const name of ['claude','codex','opencode','gh'])fs.copyFileSync(path.join(process.env.RUNNER_TEMP,exe('proof-agent')),path.join(bind,exe(name)));
 fs.writeFileSync(path.join(home,'.gitconfig'),'[user]\n name = Fixture\n email = fixture@example.test\n');
 const all=new Map(); const sink=http.createServer(async(req,res)=>{let s='';for await(const x of req)s+=x;if(req.url==='/analytics/v1/cli-events'){for(const e of JSON.parse(s).events){if(['cli_chat_retry_exhausted','cli_chat_retry_started'].includes(e.name)) all.set(e.eventId,{event:e.name,...e.properties});}res.writeHead(200,{'content-type':'application/json'}).end('{"accepted":true}');}else res.writeHead(404).end();});
 sink.on('connect',(_,s)=>s.destroy());sink.listen(0,'127.0.0.1');await once(sink,'listening');const outside='http://127.0.0.1:'+sink.address().port;
 const env={};for(const k of ['SystemRoot','COMSPEC','PATHEXT','ProgramFiles','ProgramFiles(x86)','WINDIR','LANG'])if(process.env[k])env[k]=process.env[k];
 Object.assign(env,{HOME:home,USERPROFILE:home,APPDATA:path.join(home,'roaming'),LOCALAPPDATA:path.join(home,'local'),XDG_CONFIG_HOME:path.join(home,'xdg-config'),XDG_DATA_HOME:path.join(home,'xdg-data'),XDG_CACHE_HOME:path.join(home,'xdg-cache'),CODEX_HOME:path.join(home,'.codex'),ORX_DATA_DIR:path.join(home,'state'),TMPDIR:path.join(home,'tmp'),TMP:path.join(home,'tmp'),TEMP:path.join(home,'tmp'),PATH:bind+path.delimiter+(process.env.PATH??process.env.Path),LANG:'en_US.UTF-8',ORX_NO_MODEL_PRICES:'1',ORX_NO_UPDATE_CHECK:'1',ORX_TELEMETRY_HOST:outside,OPENRESEARCH_API_URL:outside,ALPHAXIV_API_URL:outside,HTTPS_PROXY:outside,HTTP_PROXY:outside,ALL_PROXY:outside,NO_PROXY:'127.0.0.1,localhost',no_proxy:'127.0.0.1,localhost'});
 const app=spawn(bin,['up','--no-browser','--port','0'],{env,cwd:home,stdio:['ignore','ignore','pipe']});let url;
 const rl=readline.createInterface({input:app.stderr});rl.on('line',l=>{const m=l.match(/dashboard on (http:\/\/127\.0\.0\.1:\d+)/);if(m)url=m[1]});
 try{
  await until(()=>{assert.equal(app.exitCode,null,'app exited '+app.exitCode);return url;},'orx up');
  const api=async(route,body)=>{const r=await fetch(url+'/api/'+route,{method:body?'POST':'GET',headers:{'Content-Type':'application/json',Origin:url},body:body&&JSON.stringify(body)});if(!r.ok)throw Error(route+' '+r.status+' '+await r.text());return r.json();};
  const {project}=await api('projects',{name:'Runtime fixture',path:path.join(home,'projects','runtime'),createFolder:true,requireNewFolder:true,initializeGit:true});
  const rows=[];
  for(const harness of ['claude-code','codex']){
   const id=crypto.randomUUID();const db=new DatabaseSync(findFile(home,'orx.db'));db.exec('pragma busy_timeout=10000');
   db.prepare('insert into chat_sessions(id,project_id,harness,title,model,created_at,updated_at) values(?,?,?,?,?,1,1)').run(id,project.id,harness,'Existing fixture',harness==='codex'?'gpt-5.4':'sonnet');db.close();
   await api('chat/sessions/'+id+'/message',{text:'Synthetic turn',clientTurnId:crypto.randomUUID()});
   const local=await until(async()=>{for(const m of (await api('chat/sessions/'+id+'/messages')).messages)for(const p of m.parts??[]) if(p.id==='turn-recovery')return p.state;},'recorded terminal recovery');
   await until(()=>[...all.values()].some(r=>r.harness===harness&&r.event==='cli_chat_retry_exhausted'),'terminal telemetry over HTTP');
   const events=[...all.values()].filter(e=>e.harness===harness);const kind=harness==='codex'?'codex_setup':'claude_setup';
   assert.equal(local.input.errorKind,kind);assert.equal(local.input.recoveryAction,'retry');
   assert.deepEqual(events.find(e=>e.event==='cli_chat_retry_exhausted'),{event:'cli_chat_retry_exhausted',harness,owner:'orx',reason:phase==='released'?kind:kind+(harness==='codex'?'_initialize':'_sign_in'),attempt:4});
   assert.deepEqual(events.find(e=>e.event==='cli_chat_retry_started'),{event:'cli_chat_retry_started',harness,owner:'orx',reason:'local_control_plane',attempt:2});
   assert.equal(events.length,2);const receipt={phase,platform:os.platform(),osRelease:os.release(),arch:os.arch(),kind,recovery:local.input.recoveryAction,events}; rows.push(receipt);console.log(JSON.stringify(receipt));
  }
  verified.push(...rows);
 }finally{app.kill();rl.close();sink.closeAllConnections();sink.close();}
}
assert.equal(verified.length,4);
fs.writeFileSync(path.join(out,'native-traces.json'),JSON.stringify({candidate:sha,baseline:'v0.2.27',fixtures:'isolated existing chats; signed-out Claude and retryable Codex initialize rejection',collector:'real release-capable local apps to loopback HTTP',verified},null,2));
