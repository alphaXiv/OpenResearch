import assert from 'node:assert/strict';
import {spawn, spawnSync} from 'node:child_process';
import http from 'node:http';
import {once} from 'node:events';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import readline from 'node:readline';

const candidateSha='92352e3685382a7a61ae37520010b4e852aecc39';
const proof=path.resolve('receipts'); fs.mkdirSync(proof,{recursive:true});
const harmless='This chat\'s OpenAI Codex account was removed. Choose another agent.';
const fixture='Codex sign-in for a removed account';
const results=[];
const pending=[];
let failedEvent;
const collect = http.createServer(async(req,res)=>{
  if (req.method==='POST' && req.url==='/analytics/v1/cli-events') {
    const chunks=[]; for await(const chunk of req) chunks.push(chunk);
    for(const e of JSON.parse(Buffer.concat(chunks)).events ?? []) {
      const p=e.properties;
      if(e.name==='cli_harness_setup' && p.harness==='codex' && p.action==='login') {
        pending.push({stage:p.stage,reason:p.reason??null,errorExcerpt:p.errorExcerpt??null,outcome:p.outcome});
        if(p.outcome==='failed') failedEvent?.(pending.at(-1));
      }
    }
    res.writeHead(200,{'content-type':'application/json'}).end('{"accepted":true}'); return;
  }
  res.writeHead(404).end();
});
collect.listen(0,'127.0.0.1'); await once(collect,'listening');
const collector='http://127.0.0.1:'+collect.address().port;
async function limited(p,ms,label) {
  let t;try{return await Promise.race([p,new Promise((_,reject)=>t=setTimeout(()=>reject(new Error('Timed out: '+label)),ms))]);}finally{clearTimeout(t);}
}
async function exercise(binary,label,build,expected) {
  pending.length=0;
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'orx-evidence-'));
  const home=path.join(root,'home'); fs.mkdirSync(home);
  const env={};for(const k of ['SystemRoot','ComSpec','WINDIR','PATHEXT','PROCESSOR_ARCHITECTURE','NUMBER_OF_PROCESSORS','ProgramFiles','ProgramFiles(x86)']) if(process.env[k])env[k]=process.env[k];
  Object.assign(env,{
    PATH:[path.dirname(process.env.PROOF_CODEX),path.join(env.SystemRoot,'System32'),env.SystemRoot].join(';'), HOME:home,USERPROFILE:home,
    APPDATA:path.join(home,'AppData','Roaming'),LOCALAPPDATA:path.join(home,'AppData','Local'),TEMP:root,TMP:root,
    XDG_CONFIG_HOME:path.join(home,'config'),XDG_DATA_HOME:path.join(home,'share'),XDG_CACHE_HOME:path.join(home,'cache'),XDG_STATE_HOME:path.join(home,'state'),
    CODEX_HOME:path.join(home,'.codex'), ORX_DATA_DIR:path.join(home,'data'),ORX_CACHE_DIR:path.join(root,'cache'),
    ORX_NO_MODEL_PRICES:'1',ORX_NO_UPDATE_CHECK:'1', ORX_TELEMETRY_HOST:collector,
    OPENRESEARCH_API_URL:collector, ALPHAXIV_API_URL:collector, HTTPS_PROXY:collector, HTTP_PROXY:collector,ALL_PROXY:collector,NO_PROXY:'127.0.0.1,localhost'
  });
  const app=spawn(binary,['up','--no-browser','--port','0'],{env,stdio:['ignore','pipe','pipe']});
  app.stdout.resume(); const lines=readline.createInterface({input:app.stderr});
  try{
    const base=await limited(new Promise((resolve,reject)=>{
      lines.on('line',l=>{const m=/dashboard on (http:\/\/127\.0\.0\.1:\d+)/.exec(l);if(m)resolve(m[1]);});
      app.once('error',reject);app.once('exit',c=>reject(new Error('CLI exited '+c)));
    }),60000,'dashboard startup');
    const h=await fetch(base+'/api/harnesses/codex/snapshot');assert.equal(h.status,200);assert.equal((await h.json()).installed,true,'real Windows Codex detected');
    const post=await fetch(base+'/api/agent-profiles',{method:'POST',headers:{'Content-Type':'application/json',Origin:base},body:JSON.stringify({harness:'codex',name:'Disposable proof fixture'})});
    assert.equal(post.status,200,'create synthetic account: '+post.status);const {id}=await post.json();assert.match(id,/^codex-[a-z0-9]{10}$/);
    const deleted=await fetch(base+'/api/agent-profiles/'+id,{method:'DELETE',headers:{Origin:base}});assert.equal(deleted.status,200,'delete disposable profile');
    const event=new Promise(resolve=>failedEvent=resolve);
    const ws=new WebSocket(base.replace('http:','ws:')+'/api/harnesses/setup?'+new URLSearchParams({harness:'codex',action:'login',profile:id}), {headers:{Origin:base}});
    const message=await limited(new Promise((resolve,reject)=>{
      ws.addEventListener('message',ev=>{if(typeof ev.data==='string'){const m=JSON.parse(ev.data);if(m.type==='error')resolve(m.error);}});
      ws.addEventListener('error',()=>reject(new Error('setup socket failed')));
      ws.addEventListener('close',()=>reject(new Error('setup socket closed before error')));
    }),90000,'setup error');
    ws.close(); assert.equal(message,harmless);
    const wire=await limited(event,25000,'loopback telemetry failure');assert.deepEqual(wire,{stage:'detect',reason:expected[0],errorExcerpt:expected[1],outcome:'failed'});
    assert.equal(pending.filter(e=>e.outcome==='failed').length,1);
    const row={label,build,fixture,platform:os.type()+' '+os.release()+' '+os.arch(),codex:'0.169.1',appMessage:message,wire};
    results.push(row);console.log(JSON.stringify(row));
  }finally{failedEvent=null;app.kill();lines.close();}
}
try{
  await exercise(process.env.ORX_BEFORE,'Before','v0.2.27',['not_installed',null]);
  await exercise(path.resolve('candidate/target/debug/orx.exe'),'After',candidateSha,['not_ready','agent account could not be prepared']);
  const data=JSON.stringify(results,null,2);fs.writeFileSync(path.join(proof,'receipts.json'),data);
  const box=r=>'<section><h2>'+r.label+' <small>'+r.build.slice(0,12)+'</small></h2><p>Setup dialog: '+r.appMessage+'</p><pre>'+JSON.stringify(r.wire,null,2)+'</pre></section>';
  const html='<!doctype html><meta charset=utf-8><title>Actual Windows setup receipts</title><style>body{font:21px Segoe UI,sans-serif;margin:40px;color:#162532;background:#f4f7f8}h1{font-size:40px;margin:0}h2{margin:0;color:#244065}small{font:18px Consolas} main{display:flex;gap:20px}section{flex:1;background:white;border:1px solid #c4ccd0;border-radius:8px;padding:22px}pre{background:#edf2f5;font:20px Consolas;padding:12px;white-space:pre-wrap}</style><h1>Codex sign-in: the account was removed, not the CLI</h1><p>Native '+results[0].platform+' · vendor Codex 0.169.1 · actual setup socket and local telemetry collector</p><main>'+results.map(box).join('')+'</main><p>Same case in both runs: create and remove a disposable account, then request sign-in for it. No real account or credential used.</p>';
  const htmlPath=path.join(proof,'receipts.html');fs.writeFileSync(htmlPath,html);
  const edge=path.join(process.env['ProgramFiles(x86)'],'Microsoft','Edge','Application','msedge.exe');
  const screenshot=path.join(proof,'receipts.png');
  const screen=spawnSync(edge,['--headless','--disable-gpu','--hide-scrollbars','--no-first-run','--user-data-dir='+fs.mkdtempSync(path.join(os.tmpdir(),'orx-edge-')),'--window-size=1600,760','--screenshot='+screenshot,new URL('file:///'+htmlPath.replaceAll('\\','/')).href],{timeout:60000});
  assert.ok(fs.existsSync(screenshot),'Edge screenshot failed: '+screen.status);
}finally{collect.closeAllConnections();collect.close();}
