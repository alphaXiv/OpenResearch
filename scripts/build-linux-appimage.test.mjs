import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

for (const backend of [undefined, '', 'x11', 'wayland', 'wayland,x11']) {
  test(`AppRun preserves the session backend (${JSON.stringify(backend)})`, { skip: process.platform !== 'linux' }, () => {
    const root = mkdtempSync(join(tmpdir(), 'orx-apprun '))
    try {
      mkdirSync(join(root, 'usr/bin'), { recursive: true })
      mkdirSync(join(root, 'apprun-hooks'))
      copyFileSync(new URL('../linux/AppRun', import.meta.url), join(root, 'AppRun'))
      // The external GTK plugin changes these values even on a Wayland session.
      writeFileSync(join(root, 'apprun-hooks/gtk.sh'), `
printf hook >> "$HOOK_CALLS_FILE"
export GDK_BACKEND=x11 GTK_THEME=bundled
export XDG_DATA_DIRS="$APPDIR/usr/share:$XDG_DATA_DIRS"
export GIO_EXTRA_MODULES="$APPDIR/usr/lib/gio/modules"
`)
      // Record the launcher's process boundary without starting the desktop app.
      writeFileSync(join(root, 'usr/bin/orx'), `#!${process.execPath}
console.log(JSON.stringify({ args: process.argv.slice(2), cwd: process.cwd(), env: process.env }))
`, { mode: 0o755 })
      const env = {
        ...process.env,
        APPDIR: root,
        HOOK_CALLS_FILE: join(root, 'hooks-called'),
        GTK_THEME: 'host theme:dark',
        XDG_DATA_DIRS: '/host/share:/usr/share',
        GIO_EXTRA_MODULES: '/host/gio',
      }
      delete env.GDK_BACKEND
      if (backend !== undefined) env.GDK_BACKEND = backend
      const result = JSON.parse(execFileSync('bash', [join(root, 'AppRun'), 'app', 'orx', 'openresearch:project/test'], { env, encoding: 'utf8' }))
      assert.equal(readFileSync(env.HOOK_CALLS_FILE, 'utf8'), 'hook')
      assert.equal(result.env.GDK_BACKEND, backend)
      assert.equal(result.env.ORX_HOST_GDK_BACKEND, backend === undefined ? '-' : `=${backend}`)
      assert.equal(result.env.ORX_HOST_GTK_THEME, '=host theme:dark')
      assert.equal(result.env.ORX_HOST_XDG_DATA_DIRS, '=/host/share:/usr/share')
      assert.equal(result.env.ORX_HOST_GIO_EXTRA_MODULES, '=/host/gio')
      assert.equal(result.env.GTK_THEME, 'bundled')
      assert.equal(result.env.GIO_EXTRA_MODULES, undefined)
      assert.equal(result.env.GIO_MODULE_DIR, join(root, 'usr/lib/gio/modules'))
      assert.equal(result.cwd, join(root, 'usr'))
      assert.deepEqual(result.args, ['app', 'openresearch:project/test'])
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })
}
