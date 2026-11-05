import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const script = name => fileURLToPath(new URL(name, import.meta.url))

test('canary identity numbers past paginated releases and shared assets', () => {
  const root = mkdtempSync(join(tmpdir(), 'orx-canary-identity-'))
  try {
    const bin = join(root, 'bin')
    mkdirSync(bin)
    writeFileSync(join(bin, 'gh'), '#!/bin/sh\ncase "$*" in *--slurp*--jq*) exit 2;; *releases/tags/canary*) printf 1;; *assets?*) printf \'[[{"name":"canary-20261027.6--OpenResearch.dmg"}]]\';; *) printf \'[[{"tag_name":"canary"},{"tag_name":"canary-20261027.2"}],[{"tag_name":"canary-20261027.4"}]]\';; esac\n', { mode: 0o755 })
    writeFileSync(join(bin, 'date'), '#!/bin/sh\nprintf 20261027\n', { mode: 0o755 })
    writeFileSync(join(root, 'Cargo.toml'), '[package]\nversion = "0.2.21"\n')
    const git = (...args) => execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@t', ...args], { cwd: root, encoding: 'utf8' }).trim()
    git('init', '-q', '-b', 'main')
    for (const title of ['feat: a', 'fix: b', 'chore: c']) git('commit', '-q', '--allow-empty', '-m', title)
    const output = join(root, 'output')
    execFileSync('bash', [script('canary-identity.sh')], { cwd: root, env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, GITHUB_REPOSITORY: 'test/repo', GITHUB_OUTPUT: output, GITHUB_SHA: git('rev-parse', 'HEAD') } })
    assert.equal(readFileSync(output, 'utf8'), 'tag=canary-20261027.7\nversion=0.2.21\nchanges={"features":1,"fixes":1,"other":1}\n')
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('canary publication keeps published bytes and rejects older sources', () => {
  const root = mkdtempSync(join(tmpdir(), 'orx-canary-publish-'))
  try {
    for (const name of ['bin', 'artifacts', 'remote', 'runner']) mkdirSync(join(root, name))
    mkdirSync(join(root, 'remote/canary'))
    writeFileSync(join(root, 'artifacts/OpenResearch.dmg'), 'original')
    writeFileSync(join(root, 'remote/canary/desktop-canary-v2.json'), JSON.stringify({ tag: 'canary-20261027.1', commit: 'a'.repeat(40) }))
    writeFileSync(join(root, 'bin/gh'), `#!/usr/bin/env node
const fs = require('fs'), path = require('path');
const a = process.argv.slice(2), [kind, action, tag] = a, remote = path.join(process.env.FIXTURE_ROOT, 'remote'), dir = path.join(remote, tag || '');
if (kind === 'api') {
  if (action === '-X' && tag === 'PATCH' && process.env.FAIL_TAG_UPDATE === 'yes') process.exit(1);
  if (a.some(x => x.includes('/releases/tags/canary'))) console.log('1');
  else if (a.some(x => x.includes('/assets?'))) console.log(JSON.stringify([fs.readdirSync(path.join(remote, 'canary')).map(name => ({name}))]));
  else console.log(process.env.SOURCE_SHA);
  process.exit(0);
}
if (kind !== 'release') process.exit(2);
if (action === 'view') { if (!fs.existsSync(dir)) process.exit(1); if (a.includes('--json')) console.log(fs.readdirSync(dir).join('\\n')); process.exit(0); }
if (action === 'create') { fs.mkdirSync(dir); for (const f of a.slice(3).filter(x => x.startsWith('artifacts/'))) fs.copyFileSync(f, path.join(dir, path.basename(f))); process.exit(0); }
if (action === 'upload') { const f = a[3]; fs.copyFileSync(f, path.join(dir, path.basename(f))); process.exit(0); }
if (action === 'edit') process.exit(0);
if (action === 'download') { const dest = a[a.indexOf('--dir') + 1], pattern = a[a.indexOf('--pattern') + 1], prefix = pattern.split('*')[0]; let n = 0; for (const f of fs.readdirSync(dir)) if (pattern.includes('*') ? f.startsWith(prefix) : f === pattern) { fs.copyFileSync(path.join(dir, f), path.join(dest, f)); n++; } process.exit(n ? 0 : 1); }
process.exit(2);
`, { mode: 0o755 })
    writeFileSync(join(root, 'bin/git'), `#!/usr/bin/env node
if (process.argv[2] === 'merge-base') process.exit(Number(process.env.FIXTURE_ANCESTOR));
if (process.argv[2] === 'log') { console.log(process.env.SOURCE_SHA + '\\tfeat: a thing'); process.exit(0); }
if (process.argv[2] === 'rev-list') { console.log(process.env.SOURCE_SHA + '\\n' + process.env.PREVIOUS_SHA); process.exit(0); }
process.exit(2);
`, { mode: 0o755 })
    const env = { ...process.env, PATH: `${join(root, 'bin')}:${process.env.PATH}`, FIXTURE_ROOT: root, RUNNER_TEMP: join(root, 'runner'), GITHUB_REPOSITORY: 'test/repo', CANARY_CHANGES: '{"features":1,"fixes":2,"other":3}', TAG: 'canary-20261027.4', VERSION: '0.2.21', SOURCE_SHA: 'b'.repeat(40), PREVIOUS_SHA: 'a'.repeat(40) }
    const run = (ancestor, overrides = {}) => execFileSync('bash', [script('canary-publish.sh')], { cwd: root, env: { ...env, ...overrides, FIXTURE_ANCESTOR: String(ancestor) }, stdio: ['ignore', 'pipe', 'pipe'] })
    const manifest = () => JSON.parse(readFileSync(join(root, 'remote/canary/desktop-canary-v2.json')))
    run(1)
    assert.equal(manifest().tag, 'canary-20261027.1')
    rmSync(join(root, 'runner'), { recursive: true }); mkdirSync(join(root, 'runner'))
    assert.throws(() => run(128))
    rmSync(join(root, 'runner'), { recursive: true }); mkdirSync(join(root, 'runner'))
    run(0)
    const latest = manifest()
    assert.equal(latest.tag, env.TAG)
    assert.equal(latest.assets.macos.asset, `${env.TAG}--OpenResearch.dmg`)
    assert.equal(latest.assets.macos.sha256, createHash('sha256').update('original').digest('hex'))
    assert.deepEqual(latest.changes, { features: 1, fixes: 2, other: 3 })
    assert.deepEqual(latest.notes.commits, [{ sha: env.SOURCE_SHA, title: 'feat: a thing' }])
    writeFileSync(join(root, 'artifacts/OpenResearch.dmg'), 'rebuilt')
    rmSync(join(root, 'runner'), { recursive: true }); mkdirSync(join(root, 'runner'))
    run(0)
    const rerun = manifest()
    assert.equal(rerun.assets.macos.sha256, latest.assets.macos.sha256)
    rmSync(join(root, 'runner'), { recursive: true }); mkdirSync(join(root, 'runner'))
    run(0, { TAG: 'canary-20261027.5', SOURCE_SHA: 'c'.repeat(40), PREVIOUS_SHA: env.SOURCE_SHA, FAIL_TAG_UPDATE: 'yes' })
    const next = manifest()
    assert.equal(next.tag, 'canary-20261027.5')
    assert.equal(next.assets.macos.asset, 'canary-20261027.5--OpenResearch.dmg')
  } finally { rmSync(root, { recursive: true, force: true }) }
})

test('canary retention keeps seven shared builds and deletes older canary releases', () => {
  const root = mkdtempSync(join(tmpdir(), 'orx-canary-retention-'))
  try {
    for (const name of ['bin', 'runner']) mkdirSync(join(root, name))
    writeFileSync(join(root, 'bin/gh'), `#!/usr/bin/env node
const fs = require('fs'), path = require('path'), a = process.argv.slice(2);
if (a[0] === 'release' && a[1] === 'download') { const dir = a[a.indexOf('--dir') + 1]; fs.writeFileSync(path.join(dir, 'desktop-canary-v2.json'), JSON.stringify({tag:'canary-20261027.1'})); process.exit(0); }
if (a[0] === 'api' && a[1].endsWith('/releases/tags/canary')) { console.log('1'); process.exit(0); }
if (a[0] === 'api' && a.some(x => x.includes('/assets?'))) { console.log(JSON.stringify([Array.from({length:10}, (_,i) => ({id:i+1,name:'canary-20261027.'+(i+1)+'--OpenResearch.dmg'}))])); process.exit(0); }
if (a[0] === 'api' && a[1] === '-X') { fs.appendFileSync(process.env.DELETED, a[3].split('/').pop()+'\\n'); process.exit(0); }
if (a[0] === 'api') { console.log(JSON.stringify([[{tag_name:'canary-20261027.1',draft:false,prerelease:true},{tag_name:'canary-20261027.2',draft:false,prerelease:true}]])); process.exit(0); }
if (a[0] === 'release' && a[1] === 'delete') { fs.appendFileSync(process.env.RELEASES, a[2]+'\\n'); process.exit(0); }
process.exit(2);
`, { mode: 0o755 })
    const env = { ...process.env, PATH: `${join(root, 'bin')}:${process.env.PATH}`, RUNNER_TEMP: join(root, 'runner'), GITHUB_REPOSITORY: 'test/repo', DELETED: join(root, 'deleted'), RELEASES: join(root, 'releases') }
    execFileSync('bash', [script('canary-retain.sh')], { cwd: root, env })
    assert.equal(readFileSync(env.DELETED, 'utf8'), '4\n3\n2\n')
    assert.equal(readFileSync(env.RELEASES, 'utf8'), 'canary-20261027.2\n')
  } finally { rmSync(root, { recursive: true, force: true }) }
})

test('release source guard accepts merged source and rejects wrong tag, branch, or unmerged source', () => {
  const root = mkdtempSync(join(tmpdir(), 'orx-release-source-'))
  const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()
  try {
    git(['init', '-b', 'main'])
    git(['config', 'user.name', 'CI test'])
    git(['config', 'user.email', 'ci-test@example.invalid'])
    writeFileSync(join(root, 'Cargo.toml'), '[package]\nversion = "0.2.20"\n')
    git(['add', 'Cargo.toml']); git(['commit', '-m', 'version'])
    const sha = git(['rev-parse', 'HEAD'])
    git(['update-ref', 'refs/remotes/origin/main', sha])
    const run = (tag, ref) => execFileSync('bash', [script('release-source.sh')], { cwd: root, env: { ...process.env, RELEASE_TAG: tag, WORKFLOW_REF: ref, GITHUB_OUTPUT: join(root, 'output') }, stdio: ['ignore', 'pipe', 'pipe'] })
    run('v0.2.20', 'refs/heads/main')
    assert.deepEqual(JSON.parse(readFileSync(join(root, 'release-source.json'))), { tag: 'v0.2.20', sha })
    assert.throws(() => run('v0.2.21', 'refs/heads/main'))
    assert.throws(() => run('dry-run', 'refs/heads/main'))
    assert.throws(() => run('v0.2.20', 'refs/heads/feature'))
    assert.throws(() => run('v0.2.20', 'refs/pull/1/merge'))
    git(['commit', '--allow-empty', '-m', 'unmerged'])
    assert.throws(() => run('v0.2.20', 'refs/heads/main'))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
