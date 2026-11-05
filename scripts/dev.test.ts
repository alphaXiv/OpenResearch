import assert from 'node:assert/strict'
import { existsSync, linkSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import test from 'node:test'

import { copyState, parseArgs } from '../dev.ts'

test('snapshots a running database and keeps hard links linked', async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'orx-dev-state-'))
  const from = path.join(root, 'live')
  mkdirSync(path.join(from, 'data', 'run-logs'), { recursive: true })
  writeFileSync(path.join(from, 'data', 'run-logs', 'run.log'), 'log')
  mkdirSync(path.join(from, 'config', 'locks'), { recursive: true })
  writeFileSync(path.join(from, 'config', 'orx.lifecycle.lock'), '')
  linkSync(path.join(from, 'config', 'orx.lifecycle.lock'), path.join(from, 'config', 'locks', 'orx.lifecycle.lock'))
  const live = new DatabaseSync(path.join(from, 'data', "o'rx.db"))
  try {
    live.exec("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE projects (name TEXT); INSERT INTO projects VALUES ('kept')")
    assert.equal(existsSync(path.join(from, 'data', "o'rx.db-wal")), true)

    const to = path.join(root, 'states', 'saved')
    await copyState(from, to)
    assert.equal(existsSync(path.join(to, 'data', "o'rx.db-wal")), false)
    assert.equal(existsSync(path.join(to, 'data', 'run-logs', 'run.log')), true)
    const lock = (dir: string) => statSync(path.join(to, 'config', dir, 'orx.lifecycle.lock')).ino
    assert.equal(lock(''), lock('locks'))
    assert.notEqual(lock(''), statSync(path.join(from, 'config', 'orx.lifecycle.lock')).ino)
    const copy = new DatabaseSync(path.join(to, 'data', "o'rx.db"), { readOnly: true })
    assert.deepEqual({ ...copy.prepare('SELECT name FROM projects').get() }, { name: 'kept' })
    copy.close()
  } finally {
    live.close()
    rmSync(root, { recursive: true, force: true })
  }
})

test('accepts at most one state flag and only plain state names', () => {
  assert.deepEqual(parseArgs(['--state', 'demo']), { command: 'run', desktop: false, seed: { kind: 'state', name: 'demo' } })
  assert.deepEqual(parseArgs(['save', 'demo']), { command: 'save', name: 'demo' })
  assert.throws(() => parseArgs(['--copy', '--onboarding']), /at most one/)
  for (const name of ['..', '../escape', '.hidden']) assert.throws(() => parseArgs(['save', name]), /Invalid state name/)
})

test('parses the commands agents use', () => {
  assert.deepEqual(parseArgs(['logs', 'api', '-n', '20']), { command: 'logs', source: 'api', lines: 20 })
  assert.deepEqual(parseArgs(['logs']), { command: 'logs', source: null, lines: 200 })
  assert.deepEqual(parseArgs(['status', '--json']), { command: 'status', json: true })
  assert.throws(() => parseArgs(['logs', '-n', '0']), /positive/)
  assert.throws(() => parseArgs(['restart', 'now']), /Unknown argument/)
})
