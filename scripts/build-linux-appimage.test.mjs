import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const hasCompiler = spawnSync('cc', ['--version']).status === 0
if (process.platform === 'linux' && process.env.CI && !hasCompiler) throw new Error('AppImage launcher tests need cc')
const skip = process.platform !== 'linux' || !hasCompiler

function fixture({ hostWorks = false, bundleWorks = true } = {}) {
  const sandbox = mkdtempSync(join(tmpdir(), 'orx-apprun '))
  const root = join(sandbox, 'AppDir')
  const host = join(sandbox, 'host')
  const bundle = join(root, 'usr/lib')
  for (const dir of [host, bundle, join(root, 'usr/bin'), join(root, 'apprun-hooks')]) mkdirSync(dir, { recursive: true })
  copyFileSync(new URL('../linux/AppRun', import.meta.url), join(root, 'AppRun'))
  execFileSync('cc', [fileURLToPath(new URL('../linux/appimage-runtime.c', import.meta.url)), '-ldl', '-o', join(root, 'usr/bin/orx-runtime')])
  // Stand-ins for the external WebKit/GStreamer ABI, not the runtime checker.
  for (const [dir, works] of [[host, hostWorks], [bundle, bundleWorks]]) {
    const source = join(dir, 'sdk.c')
    writeFileSync(source, `
#include <stdlib.h>
int gst_init_check(void *a, void *b, void *c) { (void)a; (void)b; (void)c; return 1; }
void *gst_element_factory_make(const char *name, const char *instance) {
  (void)name; (void)instance; return ${works ? 'malloc(1)' : 'NULL'};
}
void g_object_unref(void *object) { free(object); }
`)
    execFileSync('cc', ['-shared', '-fPIC', source, '-o', join(dir, 'libgstreamer-1.0.so.0')])
    execFileSync('cc', ['-shared', '-fPIC', source, '-o', join(dir, 'libwebkit2gtk-4.1.so.0')])
    writeFileSync(join(dir, 'libgtk-3.so.0'), '')
  }
  writeFileSync(join(root, 'apprun-hooks/gtk.sh'), `
printf hook >> "$HOOK_CALLS_FILE"
export GDK_BACKEND=x11 GTK_THEME=bundled
export XDG_DATA_DIRS="$APPDIR/usr/share:$XDG_DATA_DIRS"
export GIO_EXTRA_MODULES="$APPDIR/usr/lib/gio/modules"
`)
  // Observe the launcher's CLI/process boundary without starting a server.
  writeFileSync(join(root, 'usr/bin/orx'), `#!${process.execPath}
if (process.argv[2] === 'version') process.exit(process.env.REJECT_NATIVE_VERSION === '1' && process.env.ORX_APPIMAGE_RUNTIME !== 'bundled' ? 1 : 0)
console.log(JSON.stringify({ args: process.argv.slice(2), cwd: process.cwd(), env: process.env }))
`, { mode: 0o755 })
  const env = {
    ...process.env, APPDIR: root, HOOK_CALLS_FILE: join(root, 'hooks-called'),
    LD_LIBRARY_PATH: host, GTK_THEME: 'host theme:dark',
    XDG_DATA_DIRS: '/host/share:/usr/share', GIO_EXTRA_MODULES: '/host/gio',
  }
  delete env.ORX_APPIMAGE_RUNTIME
  delete env.GDK_BACKEND
  delete env.LD_PRELOAD
  delete env.DISPLAY
  delete env.WAYLAND_DISPLAY
  return { sandbox, root, host, env, run: () => JSON.parse(execFileSync('bash', [join(root, 'AppRun'), 'app', 'orx', 'openresearch:project/test'], { env, encoding: 'utf8' })) }
}

for (const backend of [undefined, '', 'x11', 'wayland', 'wayland,x11']) {
  test(`AppRun preserves the session backend (${JSON.stringify(backend)})`, { skip }, () => {
    const app = fixture()
    try {
      if (backend !== undefined) app.env.GDK_BACKEND = backend
      const result = app.run()
      assert.equal(readFileSync(app.env.HOOK_CALLS_FILE, 'utf8'), 'hook')
      assert.equal(result.env.GDK_BACKEND, backend)
      assert.equal(result.env.ORX_HOST_GDK_BACKEND, backend === undefined ? '-' : `=${backend}`)
      assert.equal(result.env.ORX_HOST_LD_LIBRARY_PATH, `=${app.host}`)
      assert.equal(result.env.ORX_HOST_GTK_THEME, '=host theme:dark')
      assert.equal(result.env.ORX_HOST_XDG_DATA_DIRS, '=/host/share:/usr/share')
      assert.equal(result.env.ORX_HOST_GIO_EXTRA_MODULES, '=/host/gio')
      assert.equal(result.env.ORX_APPIMAGE_RUNTIME, 'bundled')
      assert.equal(result.env.LD_LIBRARY_PATH, app.host)
      assert.equal(result.env.GTK_THEME, 'bundled')
      assert.equal(result.env.GIO_EXTRA_MODULES, undefined)
      assert.equal(result.env.GIO_MODULE_DIR, join(app.root, 'usr/lib/gio/modules'))
      assert.equal(result.cwd, join(app.root, 'usr'))
      assert.deepEqual(result.args, ['app', 'openresearch:project/test'])
    } finally { rmSync(app.sandbox, { recursive: true, force: true }) }
  })
}

test('AppRun uses the compatible host runtime without applying the bundled GTK hook', { skip }, () => {
  const app = fixture({ hostWorks: true })
  try {
    app.env.GDK_BACKEND = 'wayland'
    const result = app.run()
    assert.equal(result.env.ORX_APPIMAGE_RUNTIME, 'system')
    assert.equal(result.env.LD_LIBRARY_PATH.split(':')[0], app.host)
    assert.equal(result.env.ORX_HOST_LD_LIBRARY_PATH, `=${app.host}`)
    assert.equal(result.env.GDK_BACKEND, 'wayland')
    assert.equal(result.env.GTK_THEME, 'host theme:dark')
    assert.equal(result.env.GIO_EXTRA_MODULES, '/host/gio')
    assert.equal(existsSync(app.env.HOOK_CALLS_FILE), false)
    assert.deepEqual(result.args, ['app', 'openresearch:project/test'])
  } finally { rmSync(app.sandbox, { recursive: true, force: true }) }
})

test('AppRun rejects a host runtime that cannot load the desktop binary', { skip }, () => {
  const app = fixture({ hostWorks: true })
  try {
    app.env.REJECT_NATIVE_VERSION = '1'
    assert.equal(app.run().env.ORX_APPIMAGE_RUNTIME, 'bundled')
    assert.equal(readFileSync(app.env.HOOK_CALLS_FILE, 'utf8'), 'hook')
  } finally { rmSync(app.sandbox, { recursive: true, force: true }) }
})

test('AppRun checks media elements again after choosing the system library directory', { skip }, () => {
  const app = fixture()
  try {
    const alternate = join(app.sandbox, 'alternate')
    mkdirSync(alternate)
    copyFileSync(join(app.root, 'usr/lib/libgstreamer-1.0.so.0'), join(alternate, 'libgstreamer-1.0.so.0'))
    app.env.LD_LIBRARY_PATH = `${alternate}:${app.host}`
    assert.equal(app.run().env.ORX_APPIMAGE_RUNTIME, 'bundled')
  } finally { rmSync(app.sandbox, { recursive: true, force: true }) }
})

test('AppRun reports missing media dependencies instead of starting a broken desktop', { skip }, () => {
  const app = fixture({ bundleWorks: false })
  try {
    const result = spawnSync('bash', [join(app.root, 'AppRun')], { env: app.env, encoding: 'utf8' })
    assert.notEqual(result.status, 0)
    assert.equal(result.stdout, '')
    assert.match(result.stderr, /Missing or incompatible GStreamer element: appsrc/)
    assert.match(result.stderr, /sudo apt install libwebkit2gtk-4\.1-0/)
    assert.match(result.stderr, /sudo pacman -S --needed webkit2gtk-4\.1/)
  } finally { rmSync(app.sandbox, { recursive: true, force: true }) }
})
