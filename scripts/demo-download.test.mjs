import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

test('demo downloads report fresh, cached and failed shards', () => {
  const result = spawnSync(process.platform === 'win32' ? 'python' : 'python3', [
    fileURLToPath(new URL('./test_demo_download.py', import.meta.url)),
  ], { encoding: 'utf8', timeout: 60_000 })
  assert.equal(result.status, 0, result.error?.message ?? result.stdout + result.stderr)
})
