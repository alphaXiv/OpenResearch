import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import net from 'node:net'
import path from 'node:path'
import test from 'node:test'

import {
  acquireAdvisoryLock,
  initializeDatabase,
  managedStateMatches,
  parseArgs,
  resolveLiveDataDir,
  slotEnvironment,
  stopSshServices,
  sqliteBackupCommand,
  supervisorCommandMatches,
} from './dev-slot.mjs'

test('requires an explicit database mode when starting', () => {
  assert.throws(() => parseArgs(['start']), /requires --db empty or --db copy/)
  assert.equal(parseArgs(['start', '--db', 'empty']).db, 'empty')
  assert.equal(parseArgs(['start', '--db', 'copy', '--open']).open, true)
})

test('desktop launch is optional and exclusive to start', () => {
  assert.equal(parseArgs(['start', '--db', 'empty']).desktop, false)
  assert.throws(() => parseArgs(['status', '--desktop']), /only valid with start/)
  assert.throws(() => parseArgs(['start', '--db', 'empty', '--open', '--desktop']), /cannot be combined/)
  if (process.platform === 'darwin') {
    assert.equal(parseArgs(['start', '--db', 'empty', '--desktop']).desktop, true)
  } else {
    assert.throws(() => parseArgs(['start', '--db', 'empty', '--desktop']), /requires macOS/)
  }
})

test('empty mode creates no database', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'orx-dev-slot-empty-'))
  try {
    const destination = path.join(root, "slot's")
    const result = initializeDatabase('empty', destination)
    assert.equal(existsSync(destination), true)
    assert.equal(existsSync(path.join(destination, 'orx.db')), false)
    assert.deepEqual(result, { sourceDb: null, copiedRunLogs: false })
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('slot environment isolates data, cache, config, and build artifacts', () => {
  assert.deepEqual(slotEnvironment({
    dataDir: '/dev/slot-3',
    cacheDir: '/dev/slot-3/cache',
    configDir: '/dev/slot-3-config',
    cargoTargetDir: '/dev/cargo-target',
  }), {
    ORX_DATA_DIR: '/dev/slot-3',
    ORX_CACHE_DIR: '/dev/slot-3/cache',
    XDG_CONFIG_HOME: '/dev/slot-3-config',
    CARGO_TARGET_DIR: '/dev/cargo-target',
  })
})

test('copy mode takes a SQLite backup and copies run logs', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'orx-dev-slot-copy-'))
  try {
    const source = path.join(root, 'source')
    const destination = path.join(root, "slot's")
    mkdirSync(path.join(source, 'run-logs'), { recursive: true })
    execFileSync('sqlite3', [path.join(source, 'orx.db'), "create table projects (name text); insert into projects values ('copied');"])
    writeFileSync(path.join(source, 'run-logs', 'run.log'), 'test')

    const result = initializeDatabase('copy', destination, source)
    const name = execFileSync('sqlite3', [path.join(destination, 'orx.db'), 'select name from projects;'], { encoding: 'utf8' }).trim()
    assert.equal(name, 'copied')
    assert.equal(readFileSync(path.join(destination, 'run-logs', 'run.log'), 'utf8'), 'test')
    assert.equal(result.copiedRunLogs, true)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('escapes apostrophes in SQLite backup destinations', () => {
  assert.equal(sqliteBackupCommand("/tmp/daniel's.db"), '.backup "/tmp/daniel\'s.db"')
})

test('copy source follows persisted settings before XDG defaults', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'orx-dev-slot-settings-'))
  try {
    const configHome = path.join(root, 'config')
    const configured = path.join(root, 'configured-data')
    mkdirSync(path.join(configHome, 'openresearch'), { recursive: true })
    writeFileSync(path.join(configHome, 'openresearch', 'settings.json'), JSON.stringify({ dataDir: configured }))
    assert.equal(resolveLiveDataDir({ XDG_CONFIG_HOME: configHome, XDG_DATA_HOME: path.join(root, 'xdg-data'), ORX_DATA_DIR: '/another/dev-slot' }, root), configured)
    assert.equal(resolveLiveDataDir({ XDG_CONFIG_HOME: path.join(root, 'missing'), XDG_DATA_HOME: path.join(root, 'xdg-data'), ORX_DATA_DIR: '/another/dev-slot' }, root), path.join(root, 'xdg-data', 'openresearch'))
    writeFileSync(path.join(configHome, 'openresearch', 'settings.json'), '{broken')
    assert.equal(resolveLiveDataDir({ XDG_CONFIG_HOME: configHome, XDG_DATA_HOME: path.join(root, 'xdg-data') }, root), path.join(root, 'xdg-data', 'openresearch'))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('serializes lifecycle transitions for the same worktree', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'orx-dev-slot-lock-'))
  try {
    const lock = path.join(root, 'lifecycle-lock')
    if (process.platform === 'win32') {
      await assert.rejects(acquireAdvisoryLock(lock), /requires macOS or Linux/)
      return
    }
    const releaseFirst = await acquireAdvisoryLock(lock)
    let secondAcquired = false
    const second = acquireAdvisoryLock(lock).then(async (release) => {
      secondAcquired = true
      await release()
    })
    await new Promise((resolve) => setTimeout(resolve, 30))
    assert.equal(secondAcquired, false)
    await releaseFirst()
    await second
    assert.equal(secondAcquired, true)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('requires exact managed-state ownership before cleanup', () => {
  const info = { worktreePath: '/tmp/worktree', manifestPath: '/tmp/worktree/Cargo.toml' }
  const slot = { slotKey: 'slot-4', backendPort: 4904, uiPort: 5204 }
  const state = {
    version: 1,
    slotKey: 'slot-4',
    worktreePath: '/tmp/worktree',
    manifestPath: '/tmp/worktree/Cargo.toml',
    dbMode: 'copy',
    backendPort: 4904,
    uiPort: 5204,
  }
  assert.equal(managedStateMatches(state, info, slot), true)
  assert.equal(managedStateMatches({ ...state, worktreePath: '/tmp/other' }, info, slot), false)
  assert.equal(managedStateMatches({ ...state, slotKey: 'slot-5' }, info, slot), false)
})

test('supervisor identity includes its unguessable launch token', () => {
  const helper = path.resolve('scripts/dev-slot.mjs')
  const command = `${process.execPath} ${helper} _supervise token-a cargo run`
  assert.equal(supervisorCommandMatches(command, 'token-a', helper), true)
  assert.equal(supervisorCommandMatches(command, 'token-b', helper), false)
  assert.equal(supervisorCommandMatches(command, 'token-a', '/tmp/other-helper.mjs'), false)
})

test('stopping a dashboard leaves busy SSH monitoring alive', async () => {
  const dataDir = mkdtempSync(path.join(os.tmpdir(), 'orx-busy-service-'))
  const root = path.join(dataDir, `ssh-service-${'a'.repeat(64)}`)
  mkdirSync(root)
  const server = net.createServer((socket) => {
    let received = Buffer.alloc(0)
    socket.on('data', (chunk) => {
      received = Buffer.concat([received, chunk])
      if (received.length < 4 || received.length < 4 + received.readUInt32BE()) return
      const request = JSON.parse(received.subarray(4).toString())
      assert.ok(['shutdown_if_idle', 'shutdown'].includes(request.operation.op))
      const response = Buffer.from(JSON.stringify(request.operation.op === 'shutdown'
        ? { value: null, error: 'SSH monitoring is busy' }
        : { value: { busy: true }, error: null }))
      const header = Buffer.alloc(4)
      header.writeUInt32BE(response.length)
      socket.end(Buffer.concat([header, response]))
    })
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  try {
    writeFileSync(path.join(root, 'endpoint.json'), JSON.stringify({ port: server.address().port, token: 'fixture', version: 1 }))
    await stopSshServices({ dataDir })
    assert.equal(server.listening, true)
    await assert.rejects(stopSshServices({ dataDir }, true), /SSH monitoring is busy/)
    assert.equal(server.listening, true)
  } finally {
    await new Promise((resolve) => server.close(resolve))
    rmSync(dataDir, { recursive: true, force: true })
  }
})
