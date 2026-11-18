import fs from 'node:fs'; import path from 'node:path'; import os from 'node:os';
import http from 'node:http'; import net from 'node:net'; import {spawn,spawnSync} from 'node:child_process';
import readline from 'node:readline'; import {once} from 'node:events'; import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';

const [variant,sha,binary]=process.argv.slice(2), dest=path.resolve('evidence-opencode');
fs.mkdirSync(dest,{recursive:true});
const win=process.platform==='win32', exe=win?'.exe':'', all=[];
const listen=async server=>{server.listen(0,'127.0.0.1');await once(server,'listening');return 'http://127.0.0.1:'+server.address().port};
async function limited(p,ms,label){let t;try{return await Promise.race([p,new Promise((_,rej)=>t=setTimeout(()=>rej(Error('Timeout: '+label)),ms))])}finally{clearTimeout(t)}}
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function exercise(mode) {
  const native=mode==='native', transient=mode.startsWith('timeout'), asset=mode.endsWith('asset'), recover=variant==='fixed' && transient;
  const home=fs.mkdtempSync(path.join(os.tmpdir(),'orx-pr-proof-')), bin=path.join(home,'bin');fs.mkdirSync(bin);
  if(native){
    const script=path.join(home,'fixture.rs');
    fs.writeFileSync(script,'fn main(){ let a=std::env::args().nth(1); if a.as_deref()==Some("--version"){println!("2.0.0");}else{std::process::exit(1);} }');
    const r=spawnSync('rustc',[script,'-o',path.join(bin,'opencode'+exe)],{encoding:'utf8'});assert.equal(r.status,0,r.stderr);
  }
  let tunnels=0, injected=0;const sockets=new Set(),events=[];
  const server=http.createServer(async(req,res)=>{
    if(req.method==='POST' && req.url==='/analytics/v1/cli-events'){
      const chunks=[];for await(const c of req)chunks.push(c);
      for(const e of JSON.parse(Buffer.concat(chunks)).events??[]){const p=e.properties;
        if(e.name==='cli_harness_setup' && p.harness==='opencode' && p.trigger==='manual' && p.action===(native?'update':'install'))
          events.push({outcome:p.outcome,stage:p.stage,reason:p.reason??null,errorExcerpt:p.errorExcerpt??null});
      }res.writeHead(200).end('{}');return;
    }
    res.writeHead(404).end();
  });
  server.on('connect',(req,sock,head)=>{
    sockets.add(sock);sock.on('error',()=>{});sock.on('close',()=>sockets.delete(sock));
    // A fixed upstream only. TLS runs end-to-end between orx and the real npm registry.
    if(req.url!=='registry.npmjs.org:443'){sock.end('HTTP/1.1 403 Forbidden\r\n\r\n');return;}
    const n=++tunnels, target=asset?2:1;
    if(n===target){injected++;if(transient){sock.write('HTTP/1.1 200 Connection Established\r\n\r\n');return;}sock.end('HTTP/1.1 403 Forbidden\r\n\r\n');return;}
    const upstream=net.connect(443,'registry.npmjs.org',()=>{sock.write('HTTP/1.1 200 Connection Established\r\n\r\n');if(head.length)upstream.write(head);sock.pipe(upstream).pipe(sock);});
    upstream.on('error',()=>sock.destroy());sock.once('close',()=>upstream.destroy());
  });
  const origin=await listen(server),env={};
  for(const k of ['SystemRoot','ComSpec','WINDIR','PATHEXT','PROCESSOR_ARCHITECTURE','NUMBER_OF_PROCESSORS','ProgramFiles','ProgramFiles(x86)'])if(process.env[k])env[k]=process.env[k];
  const system=win?[path.join(env.SystemRoot,'System32'),env.SystemRoot,'C:\\Program Files\\Git\\cmd']:['/usr/bin','/bin','/usr/sbin','/sbin'];
  Object.assign(env,{PATH:[bin,...system].join(path.delimiter),HOME:home,USERPROFILE:home,APPDATA:path.join(home,'AppData/Roaming'),LOCALAPPDATA:path.join(home,'AppData/Local'),TEMP:home,TMP:home,TMPDIR:home,
    XDG_CONFIG_HOME:path.join(home,'config'),XDG_CACHE_HOME:path.join(home,'cache'),XDG_DATA_HOME:path.join(home,'data'),XDG_STATE_HOME:path.join(home,'state'),
    ORX_DATA_DIR:path.join(home,'orx-data'),ORX_CACHE_DIR:path.join(home,'orx-cache'),ORX_NO_MODEL_PRICES:'1',ORX_NO_UPDATE_CHECK:'1',ORX_TELEMETRY_HOST:origin,
    OPENRESEARCH_API_URL:origin,ALPHAXIV_API_URL:origin,HTTP_PROXY:origin,HTTPS_PROXY:origin,ALL_PROXY:origin,NO_PROXY:'127.0.0.1,localhost',SHELL:win?env.ComSpec:'/bin/sh'});
  const app=spawn(path.resolve(binary),['up','--no-browser','--port','0'],{env,stdio:['ignore','pipe','pipe']});app.stdout.resume(); const lines=readline.createInterface({input:app.stderr});
  let ws;
  try{
    const base=await limited(new Promise((ok,no)=>{lines.on('line',line=>{const m=/dashboard on (http:\/\/127\.0\.0\.1:\d+)/.exec(line);if(m)ok(m[1]);});app.once('error',no);app.once('exit',c=>no(Error('orx exited '+c)));}),60000,'startup');
    const state=async()=>{const r=await fetch(base+'/api/harnesses/opencode/snapshot');assert.equal(r.status,200);return r.json();};
    const initial=await state(); assert.equal(initial.installed,native,'unexpected preexisting OpenCode');
    const host=base.replace('http:','ws:'); ws=new WebSocket(host+'/api/harnesses/setup?harness=opencode&trigger=manual&action='+(native?'update':'install'),{headers:{Origin:base}});
    // The app terminal answers ConPTY's cursor-position query, even when folded.
    ws.binaryType='arraybuffer';let tail='';
    ws.addEventListener('message',evt=>{if(typeof evt.data!=='string'){const chars=tail+Buffer.from(evt.data).toString('ascii');if(chars.includes('\x1b[6n'))ws.send(Buffer.from('\x1b[1;1R'));tail=chars.slice(-3);}});
    const frame=await limited(new Promise((ok,no)=>{ws.addEventListener('message',evt=>{if(typeof evt.data==='string'){const m=JSON.parse(evt.data);if(['complete','error'].includes(m.type))ok(m);}});ws.addEventListener('error',()=>no(Error('setup websocket failed')));ws.addEventListener('close',()=>no(Error('no terminal frame')));}),180000,'setup socket');
    assert.equal(injected,1);console.log(mode+': socket='+frame.type+' / npm tunnels='+tunnels);
    assert.equal(frame.type,recover?'complete':'error',JSON.stringify(frame).slice(0,450));
    await limited((async()=>{while(!events.some(p=>['succeeded','failed'].includes(p.outcome)))await sleep(100)})(),20000,'local telemetry');
    const finals=events.filter(p=>['succeeded','failed'].includes(p.outcome));assert.equal(finals.length,1);const wire=finals[0];
    assert.equal(wire.outcome,recover?'succeeded':'failed');let version=null;
    if(recover){
      const current=await state();assert.equal(current.installed,true);
      const root=path.join(home,'orx-cache','managed-agents','opencode'),dir=fs.readdirSync(root).find(s=>/^2\./.test(s));assert.ok(dir);
      const command=spawnSync(path.join(root,dir,'opencode'+exe),['--version'],{env,encoding:'utf8',timeout:30000});assert.equal(command.status,0);version=command.stdout.trim();assert.ok(version.includes(dir));
    }else if(transient){
      assert.match(wire.errorExcerpt??'',/timed out/i);assert.match(wire.errorExcerpt,asset?/Could not download/i:/Could not reach/i);
    }else if(variant==='origin'){
      const expected=native?'native agent updater':asset?'managed agent copy':'managed release lookup';assert.ok(wire.errorExcerpt?.startsWith(expected),JSON.stringify(wire));
    }else if(native){assert.equal(wire.errorExcerpt,null);}
    const row={variant,sha,platform:process.platform+' '+os.release()+' '+os.arch(),mode,initiallyInstalled:initial.installed,setupFrame:frame.type,wire,installedVersion:version,npmTunnels:tunnels};
    all.push(row);fs.writeFileSync(path.join(dest,variant+'.json'),JSON.stringify(all,null,2));console.log(JSON.stringify(row));
  }finally{ws?.close();lines.close();app.kill();for(const s of sockets)s.destroy();server.closeAllConnections();server.close();}
}
for(const mode of ['refusal-metadata','refusal-asset','native'])await exercise(mode);
fs.writeFileSync(path.join(dest,variant+'.json'),JSON.stringify(all,null,2));
