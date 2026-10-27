import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { runInNewContext } from 'node:vm'

const workflow = name => readFileSync(new URL(`../.github/workflows/${name}.yml`, import.meta.url), 'utf8')
const release = workflow('release')
const ci = workflow('ci')
const canary = workflow('release-canary')

test('canary identity handles paginated releases and keeps a running build', () => {
  assert.match(canary, /group: canary-desktop\n\s+cancel-in-progress: false\n\s+queue: max/)
  assert.match(canary, /if: github.ref == 'refs\/heads\/main'/)
  for (const job of ['macos', 'windows']) {
    const section = canary.split(`\n  ${job}:`)[1].split(/\n  [a-z]/)[0]
    assert.match(section, /environment: release-signing/)
  }
  const section = canary.split('      - id: identity')[1].split('\n  ci:')[0]
  const shell = section.split('        run: |\n')[1].split('\n').map(line => line.replace(/^          /, '')).join('\n')
  const root = mkdtempSync(join(tmpdir(), 'orx-canary-identity-'))
  try {
    const bin = join(root, 'bin')
    const scripts = join(root, 'scripts')
    mkdirSync(bin)
    mkdirSync(scripts)
    writeFileSync(join(bin, 'gh'), '#!/bin/sh\ncase "$*" in *--slurp*--jq*) exit 2;; esac\nprintf \'[[{"tag_name":"canary-20261027.2"}],[{"tag_name":"canary-20261027.4"}]]\'\n', { mode: 0o755 })
    writeFileSync(join(bin, 'date'), '#!/bin/sh\nprintf 20261027\n', { mode: 0o755 })
    writeFileSync(join(scripts, 'canary-tag.mjs'), readFileSync(new URL('../scripts/canary-tag.mjs', import.meta.url)))
    writeFileSync(join(root, 'Cargo.toml'), '[package]\nversion = "0.2.21"\n')
    const output = join(root, 'output')
    execFileSync('bash', ['-c', shell], { cwd: root, env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, GITHUB_REPOSITORY: 'test/repo', GITHUB_OUTPUT: output } })
    assert.match(readFileSync(output, 'utf8'), /^tag=canary-20261027\.5\nversion=0\.2\.21\n$/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('canary recovery preserves published bytes and cannot move the pointer backward', () => {
  const section = canary.split('      - name: Publish canary')[1].split('      - name: Retain seven canary builds')[0]
  const shell = section.split('        run: |\n')[1].split('\n').map(line => line.replace(/^          /, '')).join('\n')
  const root = mkdtempSync(join(tmpdir(), 'orx-canary-publish-'))
  try {
    const bin = join(root, 'bin')
    for (const name of ['bin', 'scripts', 'artifacts', 'remote', 'runner']) mkdirSync(join(root, name))
    writeFileSync(join(root, 'scripts/canary-manifest.mjs'), readFileSync(new URL('../scripts/canary-manifest.mjs', import.meta.url)))
    writeFileSync(join(root, 'artifacts/OpenResearch.dmg'), 'rebuilt bytes')
    writeFileSync(join(root, 'remote/OpenResearch.dmg'), 'original published bytes')
    writeFileSync(join(bin, 'gh'), `#!/usr/bin/env node
const fs = require('fs'), path = require('path');
const args = process.argv.slice(2), [kind, action, tag] = args;
if (kind === 'api') { console.log(process.env.SOURCE_SHA); process.exit(0); }
if (kind !== 'release') process.exit(2);
if (action === 'view') { if (tag !== 'canary' && process.env.FIXTURE_EXISTING === 'no') process.exit(1); if (args.includes('--json')) console.log(tag === 'canary' ? 'desktop-canary.json' : 'OpenResearch.dmg'); process.exit(0); }
if (action === 'create' && tag !== 'canary') { fs.copyFileSync('artifacts/OpenResearch.dmg', path.join(process.env.FIXTURE_ROOT, 'remote/OpenResearch.dmg')); process.exit(0); }
if (action === 'download') {
  const dir = args[args.indexOf('--dir') + 1];
  if (tag === 'canary') fs.writeFileSync(path.join(dir, 'desktop-canary.json'), JSON.stringify({commit:process.env.PREVIOUS_SHA}));
  else fs.copyFileSync(path.join(process.env.FIXTURE_ROOT, 'remote/OpenResearch.dmg'), path.join(dir, 'OpenResearch.dmg'));
  process.exit(0);
}
if (action === 'upload' && tag === 'canary') { fs.copyFileSync(args[3], path.join(process.env.FIXTURE_ROOT, 'pointer.json')); process.exit(0); }
process.exit(2);
`, { mode: 0o755 })
    writeFileSync(join(bin, 'git'), `#!/usr/bin/env node
if (process.argv[2] === 'merge-base') process.exit(process.env.FIXTURE_ANCESTOR === 'yes' ? 0 : 1);
if (process.argv[2] === 'rev-list') { console.log(process.env.SOURCE_SHA + '\\n' + process.env.PREVIOUS_SHA); process.exit(0); }
process.exit(2);
`, { mode: 0o755 })
    const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, FIXTURE_ROOT: root,
      RUNNER_TEMP: join(root, 'runner'), GITHUB_REPOSITORY: 'test/repo', TAG: 'canary-20261027.4',
      VERSION: '0.2.21', SOURCE_SHA: 'b'.repeat(40), PREVIOUS_SHA: 'a'.repeat(40) }
    execFileSync('bash', ['-c', shell], { cwd: root, env: { ...env, FIXTURE_ANCESTOR: 'no' } })
    assert.throws(() => readFileSync(join(root, 'pointer.json')))
    rmSync(join(root, 'runner'), { recursive: true, force: true })
    mkdirSync(join(root, 'runner'))
    execFileSync('bash', ['-c', shell], { cwd: root, env: { ...env, FIXTURE_ANCESTOR: 'yes' } })
    const manifest = JSON.parse(readFileSync(join(root, 'pointer.json'), 'utf8'))
    assert.equal(manifest.commit, env.SOURCE_SHA)
    assert.equal(manifest.assets.macos.sha256, createHash('sha256').update('original published bytes').digest('hex'))
    rmSync(join(root, 'runner'), { recursive: true, force: true })
    mkdirSync(join(root, 'runner'))
    execFileSync('bash', ['-c', shell], { cwd: root, env: { ...env, FIXTURE_ANCESTOR: 'yes', FIXTURE_EXISTING: 'no' } })
    const fresh = JSON.parse(readFileSync(join(root, 'pointer.json'), 'utf8'))
    assert.equal(fresh.assets.macos.sha256, createHash('sha256').update('rebuilt bytes').digest('hex'))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('canary retention keeps the published pointer and the six newest other builds', () => {
  const section = canary.split('      - name: Retain seven canary builds')[1]
  const shell = section.split('        run: |\n')[1].split('\n').map(line => line.replace(/^          /, '')).join('\n')
  const root = mkdtempSync(join(tmpdir(), 'orx-canary-retention-'))
  try {
    const bin = join(root, 'bin')
    mkdirSync(bin)
    mkdirSync(join(root, 'runner'))
    writeFileSync(join(bin, 'gh'), `#!/usr/bin/env node
const fs = require('fs'), path = require('path');
const args = process.argv.slice(2);
if (args[0] === 'api') {
  const releases = Array.from({length: 10}, (_, i) => ({tag_name: 'canary-20261027.' + (i + 1), draft: false, prerelease: true, published_at: String(i).padStart(2, '0'), id: i + 1}));
  releases.push({tag_name: 'canary', draft: false, prerelease: true, published_at: '99', id: 99}, {tag_name: 'v1.0.0', draft: false, prerelease: false, published_at: '99', id: 100});
  console.log(JSON.stringify([releases.slice(0, 5), releases.slice(5)])); process.exit(0);
}
if (args[0] === 'release' && args[1] === 'download') { fs.writeFileSync(path.join(args[args.indexOf('--dir') + 1], 'desktop-canary.json'), JSON.stringify({tag: process.env.LATEST})); process.exit(0); }
if (args[0] === 'release' && args[1] === 'delete' && args.includes('--cleanup-tag') && args.includes('--yes')) { fs.appendFileSync(process.env.DELETED, args[2] + '\\n'); process.exit(0); }
process.exit(2);
`, { mode: 0o755 })
    const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, RUNNER_TEMP: join(root, 'runner'), GITHUB_REPOSITORY: 'test/repo', DELETED: join(root, 'deleted') }
    execFileSync('bash', ['-c', shell], { cwd: root, env: { ...env, LATEST: 'canary-20261027.1' } })
    assert.equal(readFileSync(env.DELETED, 'utf8'), 'canary-20261027.4\ncanary-20261027.3\ncanary-20261027.2\n')
    rmSync(env.DELETED)
    execFileSync('bash', ['-c', shell], { cwd: root, env: { ...env, LATEST: 'canary-20261027.99' } })
    assert.equal(readFileSync(env.DELETED, 'utf8'), 'canary-20261027.3\ncanary-20261027.2\ncanary-20261027.1\n')
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('releases have no PR trigger or dry-run packaging path', () => {
  const triggers = release.slice(release.indexOf('\non:\n'), release.indexOf('\njobs:\n'))
  assert.doesNotMatch(triggers, /pull_request|dry-run/)
  assert.match(release, /test "\$WORKFLOW_REF" = refs\/heads\/main/)
  assert.match(workflow('release-on-bump'), /-f source-sha="\$GITHUB_SHA"/)
})
test('ordinary CI cannot package, and optimized checks require release context', () => {
  assert.equal([...ci.matchAll(/cargo build --release/g)].length, 1)
  assert.doesNotMatch(ci, /build-linux-appimage|ISCC|upload-artifact|build-macos-app/)
  assert.match(ci, /Optimized source builds disable production telemetry\n\s+if: inputs.release && github.event_name != 'pull_request'/)
  assert.match(release, /source-sha: \$\{\{ needs.plan.outputs.source-sha \}\}\n\s+release: true/)
})
test('publication and source checks use the resolved release SHA', () => {
  assert.match(release, /RELEASE_COMMIT: "\$\{\{ needs.plan.outputs.source-sha \}\}"/)
  assert.match(release, /--target "\$RELEASE_COMMIT"/)
  const host = release.split('\n  host:')[1].split('\n  announce:')[0]
  assert.match(host, /fetch-depth: 0/)
  assert.match(host, /git rev-list "\$RELEASE_COMMIT".*artifacts\/desktop-ancestry\.json/)
  for (const job of ['custom-ci', 'custom-telemetry-contract']) {
    const section = release.split(`\n  ${job}:`)[1].split(/\n  [a-z]/)[0]
    assert.match(section, /source-sha: \$\{\{ needs.plan.outputs.source-sha \}\}/)
    assert.match(release.split('\n  host:')[1], new RegExp(`needs\\.${job}\\.result == 'success'`))
  }
  assert.match(workflow('telemetry-contract'), /ref: \$\{\{ inputs.source-sha \}\}/)
})
test('desktop attachments resolve the published tag instead of mutable main', () => {
  for (const platform of ['linux', 'windows', 'macos']) {
    const source = workflow(`release-${platform}-app`)
    assert.doesNotMatch(source, /workflow_run.head_sha/)
    assert.match(source, /--name release-source/)
    assert.match(source, /git rev-parse "refs\/tags\/\$tag\^\{commit\}"/)
    assert.match(source, /test "\$sha" = "\$expected_sha"/)
    assert.match(source, /ref: \$\{\{ needs.check.outputs.source-sha \}\}/)
  }
})
test('release source guard accepts merged source and rejects wrong tag, branch, or unmerged source', () => {
  const section = release.split('      - name: Validate the release source')[1].split('      - uses:')[0]
  const shell = section.split('        run: |\n')[1].split('\n').map(line => line.replace(/^          /, '')).join('\n')
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
    const run = (tag, ref) => execFileSync('bash', ['-c', shell], { cwd: root, env: { ...process.env, RELEASE_TAG: tag, WORKFLOW_REF: ref, GITHUB_OUTPUT: join(root, 'output') }, stdio: ['ignore', 'pipe', 'pipe'] })
    run('v0.2.20', 'refs/heads/main')
    assert.deepEqual(JSON.parse(readFileSync(join(root, 'release-source.json'))), { tag: 'v0.2.20', sha })
    assert.throws(() => run('v0.2.21', 'refs/heads/main'))
    assert.throws(() => run('dry-run', 'refs/heads/main'))
    assert.throws(() => run('v0.2.20', 'refs/heads/feature'))
    git(['commit', '--allow-empty', '-m', 'unmerged'])
    assert.throws(() => run('v0.2.20', 'refs/heads/main'))
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('PR comparison jobs avoid full history downloads without skipping version validation', () => {
  assert.doesNotMatch(ci, /fetch-depth: 0/)
  const version = ci.split('\n  version-guard:')[1]
  assert.match(version, /git fetch --no-tags --depth=1 origin "\$BASE_SHA"/)
  assert.match(version, /git show "\$\{BASE_SHA\}:Cargo.toml"/)
})

test('Windows PR lanes execute the full test suite alongside lint; full validation keeps one cache writer', () => {
  const windows = ci.split('\n  windows:')[1].split('\n  check:')[0]
  const modesExpression = windows.match(/mode: \$\{\{ (.+) \}\}/)[1]
  const steps = [...windows.matchAll(/- name: (.+)\n(?:        if: (.+)\n)?        run: (.+)/g)]
  const saveExpression = windows.match(/save-if: \$\{\{ (.+) \}\}/)[1]
  assert.match(windows, /fail-fast: false/)
  assert.match(windows, /shared-key: windows/)
  assert.match(ci, /needs: \[changes, frontend, helpers, rust, linux-app, windows\]/)
  for (const [event, release, expectedModes] of [
    ['pull_request', false, ['lint', 'test']],
    ['pull_request', true, ['all']],
    ['push', false, ['all']],
    ['workflow_dispatch', false, ['all']],
    ['workflow_call', true, ['all']],
  ]) {
    const context = { github: { event_name: event }, inputs: { release }, fromJSON: JSON.parse }
    const modes = runInNewContext(modesExpression, context)
    assert.deepEqual(modes, expectedModes)
    const commands = []
    for (const mode of modes) {
      const lane = { ...context, matrix: { mode } }
      assert.equal(runInNewContext(saveExpression, lane), mode === 'all')
      for (const [, , condition, command] of steps) {
        if (!condition || runInNewContext(condition, lane)) commands.push(command)
      }
    }
    assert.equal(commands.filter(command => command === 'cargo clippy --all-targets --locked -- -D warnings').length, 1)
    assert.equal(commands.filter(command => command === 'cargo fmt --manifest-path windows/launcher/Cargo.toml --check').length, 1)
    assert.equal(commands.filter(command => command === 'cargo clippy --manifest-path windows/launcher/Cargo.toml --locked -- -D warnings').length, 1)
    assert.equal(commands.filter(command => command === 'cargo build --manifest-path windows/launcher/Cargo.toml --locked').length, 1)
    assert.equal(commands.includes('cargo build --locked'), false)
    assert.equal(commands.filter(command => command === 'cargo test --locked').length, 1)
  }
})

test('ARM PR lanes preserve desktop lint and linking; other validation remains combined', () => {
  const linux = ci.split('\n  linux-app:')[1].split('\n  windows:')[0]
  const matrixExpression = linux.match(/include: \$\{\{ (.+) \}\}/)[1]
  const steps = [...linux.matchAll(/- name: (.+)\n(?:        if: (.+)\n)?        run: (.+)/g)]
  assert.match(linux, /fail-fast: false/)
  assert.match(linux, /shared-key: linux-app/)
  assert.match(linux, /save-if: \$\{\{ matrix.mode == 'all' \}\}/)
  for (const [event, release] of [['pull_request', false], ['pull_request', true], ['push', false], ['workflow_dispatch', false], ['workflow_call', true]]) {
    const context = { github: { event_name: event }, inputs: { release }, fromJSON: JSON.parse }
    const rows = runInNewContext(matrixExpression, context)
    const pr = event === 'pull_request' && !release
    assert.deepEqual(rows, pr
      ? [{ arch: 'x86_64', mode: 'all' }, { arch: 'aarch64', mode: 'lint' }, { arch: 'aarch64', mode: 'build' }]
      : [{ arch: 'x86_64', mode: 'all' }, { arch: 'aarch64', mode: 'all' }])
    for (const arch of ['x86_64', 'aarch64']) {
      const commands = rows.filter(row => row.arch === arch).flatMap(matrix => steps
        .filter(([, , condition]) => !condition || runInNewContext(condition, { ...context, matrix }))
        .map(([, , , command]) => command))
      assert.equal(commands.filter(command => command === 'cargo clippy --all-targets --locked --features desktop -- -D warnings').length, 1)
      assert.equal(commands.filter(command => command === 'cargo build --locked --features desktop').length, pr ? 1 : 0)
      assert.equal(commands.filter(command => command === 'cargo test --locked --features desktop').length, pr ? 0 : 1)
    }
  }
})
