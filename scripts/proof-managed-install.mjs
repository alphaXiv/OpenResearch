import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {createServer} from 'node:http';
import {mkdirSync,mkdtempSync,writeFileSync} from 'node:fs';
import {tmpdir,release} from 'node:os';
import path from 'node:path';
import {createInterface} from 'node:readline';
import {setTimeout as pause} from 'node:timers/promises';

const [base,fixed,outdir,sha] = process.argv.slice(2);
assert.equal(process.platform,'win32');
const root=mkdtempSync(path.join(tmpdir(),'orx-proof-'));
const powershell=path.join(process.env.SystemRoot,'System32','WindowsPowerShell','v1.0','powershell.exe');
const socketScript=path.join(root,'connect.ps1');
writeFileSync(socketScript,`
param([int]$Port)
$ErrorActionPreference='Stop'
$socket = [Net.WebSockets.ClientWebSocket]::new()
$socket.Options.SetRequestHeader('Origin', "http://127.0.0.1:$Port")
$cts=[Threading.CancellationTokenSource]::new(90000)
try {
  $socket.ConnectAsync([Uri]"ws://127.0.0.1:$Port/api/harnesses/setup?harness=claude-code&action=install", $cts.Token).GetAwaiter().GetResult()
  [byte[]]$buf=New-Object byte[] 262144
  while ($socket.State -eq 'Open') {
    $msg=[IO.MemoryStream]::new()
    do {
      $res=$socket.ReceiveAsync([ArraySegment[byte]]::new($buf),$cts.Token).GetAwaiter().GetResult()
      $msg.Write($buf,0,$res.Count)
    } until ($res.EndOfMessage)
    if ($res.MessageType -eq 'Text') {
      $v=[Text.Encoding]::UTF8.GetString($msg.ToArray()) | ConvertFrom-Json
      if ($v.type -eq 'error') {
        if ($v.error -notlike '*Could not look up the latest Claude Code release*') {throw 'unexpected failure'}
        Write-Output 'product error: Could not look up the latest Claude Code release'
        break
      }
    }
  }
} finally { $socket.Abort();$socket.Dispose();$cts.Dispose() }
`);
const evidence=[];
for (const [label,exe] of [['Released v0.2.27',base],['Candidate '+sha.slice(0,8),fixed]]) {
  const home=mkdtempSync(path.join(root,'home-'));
  const events=[]; let connects=0;
  const srv=createServer(async(req,res)=>{
    if(req.method==='POST' && req.url==='/analytics/v1/cli-events') {
      let body=''; for await(const chunk of req) body+=chunk;
      const data=JSON.parse(body);
      for(const ev of data.events??[]) if(ev.name==='cli_harness_setup' && ev.properties?.harness==='claude-code' && ev.properties.action==='install') events.push(ev.properties);
      res.writeHead(204); res.end();
    } else { res.writeHead(404); res.end() }
  });
  srv.on('connect',(req,socket)=>{
    if(req.url==='downloads.claude.ai:443') connects++;
    socket.end('HTTP/1.1 418 Rejected by test fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n');
  });
  await new Promise(resolve=>srv.listen(0,'127.0.0.1',resolve));
  const sink='http://127.0.0.1:'+srv.address().port;
  const appdata=path.join(home,'AppData','Roaming'),local=path.join(home,'AppData','Local');
  for(const d of [appdata,local,path.join(home,'tmp')]) mkdirSync(d,{recursive:true});
  const env={
    SYSTEMROOT:process.env.SystemRoot,WINDIR:process.env.SystemRoot,
    COMSPEC:process.env.ComSpec,PATHEXT:process.env.PATHEXT,PATH:process.env.Path,
    TEMP:path.join(home,'tmp'),TMP:path.join(home,'tmp'),HOME:home,USERPROFILE:home,
    APPDATA:appdata,LOCALAPPDATA:local,XDG_CONFIG_HOME:path.join(home,'config'),XDG_DATA_HOME:path.join(home,'data'),XDG_CACHE_HOME:path.join(home,'cache'),
    ORX_DATA_DIR:path.join(home,'orx-data'),ORX_NO_UPDATE_CHECK:'1',ORX_TELEMETRY_HOST:sink,OPENRESEARCH_API_URL:sink,
    ORX_TELEMETRY_ENV:'production',HTTP_PROXY:sink,HTTPS_PROXY:sink,ALL_PROXY:sink,NO_PROXY:'127.0.0.1,localhost',CI:'true',
  };
  const app=spawn(exe,['up','--no-browser','--port','0'],{env,stdio:['ignore','pipe','pipe']});
  try {
    let matched=false;
    const port=await new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>reject(Error('dashboard startup timed out')),65000);
      app.once('error',reject); app.once('exit',code=>{clearTimeout(timer);reject(Error('dashboard exited '+code))});
      for(const stream of [app.stdout,app.stderr]) createInterface({input:stream}).on('line',l=>{
        const m=l.match(/^orx up: dashboard on http:\/\/127\.0\.0\.1:(\d+)\s*$/); if(m) {clearTimeout(timer);resolve(m[1])}
      });
    });
    await new Promise((resolve,reject)=>{
      const p=spawn(powershell,['-NoProfile','-NonInteractive','-File',socketScript,port],{stdio:['ignore','pipe','pipe']});
      p.stdout.on('data',b=>{if(b.toString().includes('product error:')) matched=true});
      p.stderr.resume(); p.on('error',reject);p.on('close',code=>code===0?resolve():reject(Error('setup socket failed: '+code)));
    });
    assert(matched, 'setup socket did not reach the expected user-visible error');
    for(let i=0;i<200&&!events.some(e=>e.outcome==='failed');i++) await pause(100);
    const fails=events.filter(e=>e.outcome==='failed'); assert.equal(fails.length,1);
    const e=fails[0];
    assert(connects>0); assert.equal(e.stage,'command');assert.equal(e.reason,'exit_nonzero');assert.equal(e.exitCode,1);
    if(exe===base) assert.equal(e.errorExcerpt,null);
    else assert.equal(e.errorExcerpt,'managed release lookup; Managed release lookup failed; Could not look up the latest Claude Code release');
    const receipt={build:label,os:'Windows '+release()+' '+process.arch,entry:'Dashboard setup WebSocket / Claude Code / Install',vendorRequest:'downloads.claude.ai:443 rejected by local fixture',localError:'Could not look up the latest Claude Code release',telemetry:{outcome:e.outcome,stage:e.stage,reason:e.reason,exitCode:e.exitCode,errorExcerpt:e.errorExcerpt}};
    evidence.push(receipt); console.log(JSON.stringify(receipt,null,2));
  } finally {
    app.kill();srv.closeAllConnections();srv.close();
  }
}
mkdirSync(outdir,{recursive:true});
writeFileSync(path.join(outdir,'receipt.json'),JSON.stringify(evidence,null,2));
const esc=s=>String(s).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;');
const html='<!doctype html><meta charset=utf-8><style>body{margin:40px;font:19px Segoe UI, sans-serif;color:#132232;background:#f5f8fa}h1{font-size:32px}h2{font-size:22px}article{background:white;border-left:5px solid #357298;padding:10px 20px;margin:14px 0}pre{font-size:16px;white-space:pre-wrap;margin:0 0 10px}</style><h1>Claude Code install · Windows telemetry capture</h1><p>Same setup request, same loopback proxy rejection. Captured at a local telemetry receiver, not production.</p>'+evidence.map(e=>'<article><h2>'+esc(e.build)+' · '+esc(e.os)+'</h2><pre>'+esc(e.entry+'\n'+e.vendorRequest+'\nApp: '+e.localError+'\n'+JSON.stringify(e.telemetry,null,2))+'</pre></article>').join('');
writeFileSync(path.join(outdir,'capture.html'),html);
