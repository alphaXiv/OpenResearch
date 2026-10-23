import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const workflow = name => readFileSync(new URL(`../.github/workflows/${name}.yml`, import.meta.url), 'utf8')
const release = workflow('release')
const ci = workflow('ci')

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
