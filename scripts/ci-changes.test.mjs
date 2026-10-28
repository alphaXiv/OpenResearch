import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { changedChecks, classifyChanges } from './ci-changes.mjs'
import { checkResults } from './ci-check-result.mjs'

const enabled = paths => Object.entries(classifyChanges(paths)).filter(([, value]) => value).map(([name]) => name)
test('frontend changes do not select Rust', () => {
  assert.deepEqual(enabled(['ui/src/App.tsx', 'ui/pnpm-lock.yaml']), ['frontend'])
})
test('Rust and embedded resources select all existing Rust platforms', () => {
  for (const path of ['src/main.rs', 'tests/compute_cli.rs', 'Cargo.lock', 'build.rs', 'agent-skills/orx-compute/SKILL.md', 'demo/nanochat/base/file.txt', 'SYSTEM_PROMPT.md', 'SKILL.md']) {
    assert.deepEqual(enabled([path]), ['rust', 'windows', 'linux', 'macos'], path)
  }
})
test('platform files select corresponding checks', () => {
  assert.deepEqual(enabled(['windows/launcher/src/main.rs']), ['windows'])
  assert.deepEqual(enabled(['linux/OpenResearch.png']), ['linux'])
  assert.deepEqual(enabled(['macos/Info.plist']), ['rust', 'macos'])
})
test('helper tests are independent and combine with other changes', () => {
  assert.deepEqual(enabled(['scripts/desktop.js']), ['dev_slot'])
  assert.deepEqual(enabled(['scripts/dev-slot.mjs', 'scripts/test_orx_chart.py', 'scripts/release_notes.py', 'ui/package.json']), ['frontend', 'dev_slot', 'chart', 'release_notes'])
})
test('docs skip builds, workflow changes and unknown files select all', () => {
  assert.deepEqual(enabled(['README.md', 'docs/linux.md', 'LICENSE', '.github/readme-assets/banner.png', 'macos/DISTRIBUTION.md', 'windows/README.md', 'ui/README.md']), [])
  for (const path of ['.github/workflows/ci.yml', 'scripts/ci-changes.mjs', '.cargo/config.toml', 'new-build-input']) assert.equal(enabled([path]).length, 9)
})
test('embedded chart changes also run chart tests', () => {
  assert.equal(classifyChanges(['agent-skills/orx-figures/assets/orx_chart.py']).chart, true)
})
test('diff reports both deleted and added paths for renames', () => {
  const calls = []
  const checks = changedChecks({ base: 'base', head: 'merge' }, args => {
    calls.push(args)
    return args[0] === 'diff' ? 'ui/old.ts\0src/new.rs\0' : 'sha\n'
  })
  assert.equal(checks.frontend, true)
  assert.equal(checks.rust, true)
  assert.deepEqual(calls[2], ['diff', '--name-only', '--no-renames', '-z', 'base', 'merge', '--'])
})
test('unavailable comparisons and forced runs select everything', () => {
  for (const options of [{}, { base: '000000' }, { base: 'missing' }, { forceAll: true }]) {
    const checks = changedChecks(options, () => { throw new Error('missing commit') })
    assert.equal(Object.values(checks).every(Boolean), true)
  }
})
const flags = Object.fromEntries(Object.entries(classifyChanges(['ui/src/App.tsx'])).map(([key, value]) => [key, String(value)]))
const needs = { changes: { result: 'success' }, frontend: { result: 'success' }, rust: { result: 'skipped' }, windows: { result: 'skipped' }, 'linux-app': { result: 'skipped' }, helpers: { result: 'skipped' } }
test('aggregate accepts intentional skips and successful selected jobs', () => checkResults(needs, flags))
test('aggregate rejects failures, cancellations, missing and unexpectedly skipped jobs', () => {
  for (const result of ['failure', 'cancelled', 'skipped', undefined]) assert.throws(() => checkResults({ ...needs, frontend: { result } }, flags))
  assert.throws(() => checkResults({ ...needs, rust: { result: 'failure' } }, flags))
  assert.throws(() => checkResults({ ...needs, changes: { result: 'failure' } }, flags))
  assert.throws(() => checkResults(needs, { ...flags, rust: undefined }))
})

test('backend and embedded JavaScript inputs run their suites without a frontend build', () => {
  for (const path of ['src/local/acp/zcode-runtime.cjs', 'src/commands/app.rs', 'agent-skills/orx-figures/assets/orx-chart.html']) {
    const checks = classifyChanges([path])
    assert.equal(checks.javascript, true, path)
    assert.equal(checks.frontend, false, path)
  }
  const selected = { ...flags, frontend: 'false', javascript: 'true' }
  assert.throws(() => checkResults({ ...needs, frontend: { result: 'skipped' } }, selected))
  checkResults({ ...needs, frontend: { result: 'skipped' }, helpers: { result: 'success' } }, selected)
})

test('shallow checkouts fetch only the missing comparison commit', () => {
  const root = mkdtempSync(join(tmpdir(), 'orx-ci-shallow-'))
  const source = join(root, 'source')
  const clone = join(root, 'clone')
  mkdirSync(source)
  const gitAt = cwd => args => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()
  const git = gitAt(source)
  try {
    git(['init', '-b', 'main'])
    git(['config', 'user.name', 'CI test'])
    git(['config', 'user.email', 'ci-test@example.invalid'])
    writeFileSync(join(source, 'README.md'), 'base')
    git(['add', '.']); git(['commit', '-m', 'base'])
    const base = git(['rev-parse', 'HEAD'])
    mkdirSync(join(source, 'ui'))
    writeFileSync(join(source, 'ui', 'app.ts'), 'changed')
    git(['add', '.']); git(['commit', '-m', 'frontend'])
    git(['clone', '--depth=1', pathToFileURL(source).href, clone])
    const shallow = gitAt(clone)
    assert.throws(() => shallow(['cat-file', '-e', base]))
    const calls = []
    const checks = changedChecks({ base }, args => { calls.push(args); return shallow(args) })
    assert.equal(checks.frontend, true)
    assert.equal(checks.rust, false)
    assert.deepEqual(calls.find(args => args[0] === 'fetch'), ['fetch', '--no-tags', '--depth=1', 'origin', base])
    assert.equal(shallow(['rev-parse', '--is-shallow-repository']), 'true')
    assert.equal(shallow(['rev-list', '--count', 'HEAD']), '1')
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
