import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const skip = process.platform !== 'linux'
function fixture({ hostWorks = false } = {}) {
  const sandbox = mkdtempSync(join(tmpdir(), 'orx-apprun '))
  const root = join(sandbox, 'AppDir'), host = join(sandbox, 'host')
  for (const dir of [host, join(root, 'usr/bin'), join(root, 'apprun-hooks'), join(sandbox, 'bin')]) mkdirSync(dir, { recursive: true })
  copyFileSync(new URL('../linux/AppRun', import.meta.url), join(root, 'AppRun'))
  if (hostWorks) writeFileSync(join(host, 'libwebkit2gtk-4.1.so.0'), '')
  writeFileSync(join(sandbox, 'bin/ldconfig'), '#!/bin/sh\nprintf "libwebkit2gtk-4.1.so.0 (libc6) => %s/libwebkit2gtk-4.1.so.0\\n" "$HOST_LIB"\n', { mode: 0o755 })
  writeFileSync(join(root, 'apprun-hooks/gtk.sh'), `
printf hook >> "$HOOK_CALLS_FILE"
export GDK_BACKEND=x11 GTK_THEME=bundled
export XDG_DATA_DIRS="$APPDIR/usr/share:$XDG_DATA_DIRS"
export GIO_EXTRA_MODULES="$APPDIR/usr/lib/gio/modules"
`)
  writeFileSync(join(root, 'usr/bin/orx'), `#!${process.execPath}
if (process.argv[2] === 'version') {
 if (process.env.LD_BIND_NOW !== '1') process.exit(2)
 process.exit(process.env.REJECT_NATIVE_VERSION === '1' ? 1 : 0)
}
console.log(JSON.stringify({ args: process.argv.slice(2), cwd: process.cwd(), env: process.env }))
`, { mode: 0o755 })
  const env = {...process.env, PATH: `${join(sandbox,'bin')}:${process.env.PATH}`, APPDIR: root, HOST_LIB: host,
    HOOK_CALLS_FILE: join(root,'hooks-called'), LD_LIBRARY_PATH: '/session/lib', GTK_THEME: 'host theme:dark',
    XDG_DATA_DIRS: '/host/share:/usr/share', GIO_EXTRA_MODULES: '/host/gio'}
  for (const key of ['ORX_APPIMAGE_RUNTIME','GDK_BACKEND','LD_PRELOAD','DISPLAY','WAYLAND_DISPLAY']) delete env[key]
  return {sandbox,root,host,env,run:(args=['app','openresearch:project/test']) => JSON.parse(execFileSync('bash',[join(root,'AppRun'),...args],{env,encoding:'utf8'}))}
}
for (const backend of [undefined, '', 'x11', 'wayland', 'wayland,x11']) {
 test(`bundled runtime preserves session backend ${JSON.stringify(backend)}`,{skip},()=>{
  const app=fixture()
  try {
   if(backend!==undefined) app.env.GDK_BACKEND=backend
   const r=app.run()
   assert.equal(readFileSync(app.env.HOOK_CALLS_FILE,'utf8'),'hook')
   assert.equal(r.env.GDK_BACKEND,backend)
   assert.equal(r.env.ORX_HOST_GDK_BACKEND,backend===undefined?'-':`=${backend}`)
   assert.equal(r.env.ORX_HOST_LD_LIBRARY_PATH,'=/session/lib')
   assert.equal(r.env.LD_LIBRARY_PATH,'/session/lib')
   assert.equal(r.env.GTK_THEME,'bundled')
   assert.equal(r.env.GIO_EXTRA_MODULES,undefined)
   assert.equal(r.env.GIO_MODULE_DIR,join(app.root,'usr/lib/gio/modules'))
   assert.equal(r.env.ORX_APPIMAGE_RUNTIME,'bundled')
   assert.deepEqual(r.args,['app','openresearch:project/test'])
  }finally{rmSync(app.sandbox,{recursive:true,force:true})}
 })
}
test('loadable system WebKit retains session settings and precedes bundled libraries',{skip},()=>{
 const app=fixture({hostWorks:true})
 try{
  app.env.GDK_BACKEND='wayland'
  const r=app.run()
  assert.equal(r.env.ORX_APPIMAGE_RUNTIME,'system')
  assert.equal(r.env.LD_LIBRARY_PATH,`${app.host}:/session/lib`)
  assert.equal(r.env.GDK_BACKEND,'wayland')
  assert.equal(r.env.GTK_THEME,'host theme:dark')
  assert.equal(r.env.GIO_EXTRA_MODULES,'/host/gio')
  assert.equal(existsSync(app.env.HOOK_CALLS_FILE),false)
 }finally{rmSync(app.sandbox,{recursive:true,force:true})}
})
test('unloadable system WebKit falls back and still starts the app',{skip},()=>{
 const app=fixture({hostWorks:true})
 try{app.env.REJECT_NATIVE_VERSION='1'; assert.equal(app.run().env.ORX_APPIMAGE_RUNTIME,'bundled')}
 finally{rmSync(app.sandbox,{recursive:true,force:true})}
})
test('file links containing spaces reach the desktop app',{skip},()=>{
 const app=fixture()
 try{assert.deepEqual(app.run(['some ignored argument','/tmp/a report.orx']).args,['app','/tmp/a report.orx'])}
 finally{rmSync(app.sandbox,{recursive:true,force:true})}
})
