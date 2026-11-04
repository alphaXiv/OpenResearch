import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { changesAt, classify } from './change-counts.mjs'

test('commit titles bucket by conventional prefix', () => {
  assert.equal(classify('feat: add a thing'), 'features')
  assert.equal(classify('feat(ui)!: break a thing'), 'features')
  assert.equal(classify('fix(updates): mend a thing'), 'fixes')
  assert.equal(classify('chore: tidy'), 'other')
  assert.equal(classify('Revert "feat: add a thing"'), 'other')
  assert.equal(classify('fixture: not a fix'), 'other')
})

test('totals count non-merge commits reachable from a commit', () => {
  const dir = mkdtempSync(join(tmpdir(), 'change-counts-'))
  const git = (...args) => execFileSync('git', ['-C', dir, '-c', 'user.name=t', '-c', 'user.email=t@t', ...args], { encoding: 'utf8' })
  try {
    git('init', '-q', '-b', 'main')
    for (const title of ['feat: a', 'fix: b', 'feat(ui): c', 'chore: d']) git('commit', '-q', '--allow-empty', '-m', title)
    const head = git('rev-parse', 'HEAD').trim()
    git('commit', '-q', '--allow-empty', '-m', 'fix: later')
    const previous = process.cwd()
    process.chdir(dir)
    try {
      assert.deepEqual(changesAt(head), { features: 2, fixes: 1, other: 1 })
    } finally {
      process.chdir(previous)
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
