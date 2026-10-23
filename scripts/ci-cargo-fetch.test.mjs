import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, writeFileSync, readFileSync, rmSync, existsSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const script = fileURLToPath(new URL('./ci-cargo-fetch.sh', import.meta.url))

for (const scenario of ['warm', 'missing', 'launcher-missing', 'invalid-lock']) {
  test(`Cargo preparation handles ${scenario} dependencies before enabling offline builds`, () => {
    const root = mkdtempSync(join(tmpdir(), 'orx-cargo-fetch-'))
    try {
      writeFileSync(join(root, 'rustc'), '#!/bin/sh\necho "host: x86_64-pc-windows-msvc"\n', { mode: 0o755 })
      writeFileSync(join(root, 'cargo'), `#!/bin/sh
printf '%s\\n' "$*" >> "$CALLS"
case "$SCENARIO:$*" in
  invalid-lock:*) exit 1;;
  missing:*--offline*) exit 1;;
  launcher-missing:*windows/launcher/Cargo.toml*--offline*) exit 1;;
esac
`, { mode: 0o755 })
      const output = join(root, 'env')
      const manifests = scenario === 'warm' ? [] : ['Cargo.toml', 'windows/launcher/Cargo.toml']
      const result = spawnSync('bash', [script, ...manifests], {
        encoding: 'utf8',
        env: { ...process.env, PATH: `${root}:${process.env.PATH}`, GITHUB_ENV: output, CALLS: join(root, 'calls'), SCENARIO: scenario },
      })
      const calls = readFileSync(join(root, 'calls'), 'utf8').trim().split('\n')
      const offline = manifest => `fetch --manifest-path ${manifest} --locked --offline --target x86_64-pc-windows-msvc`
      const online = manifest => `fetch --manifest-path ${manifest} --locked --target x86_64-pc-windows-msvc`
      const expected = [offline('Cargo.toml')]
      if (['missing', 'invalid-lock'].includes(scenario)) expected.push(online('Cargo.toml'))
      if (scenario !== 'warm' && scenario !== 'invalid-lock') {
        expected.push(offline('windows/launcher/Cargo.toml'))
        expected.push(online('windows/launcher/Cargo.toml'))
      }
      assert.deepEqual(calls, expected)
      assert.equal(result.status, scenario === 'invalid-lock' ? 1 : 0, result.stderr)
      if (scenario === 'invalid-lock') assert.equal(existsSync(output), false)
      else assert.equal(readFileSync(output, 'utf8'), 'CARGO_NET_OFFLINE=true\n')
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })
}
