import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

function tag(date, releases) {
  return execFileSync(process.execPath, ['scripts/canary-tag.mjs', date], {
    input: JSON.stringify(releases), encoding: 'utf8',
  }).trim()
}

test('numbers published canaries independently each UTC day', () => {
  assert.equal(tag('20261026', []), 'canary-20261026.1')
  assert.equal(tag('20261026', ['canary-20261026.2', 'canary-20261026.10', 'canary-20261025.99', 'v0.2.21']), 'canary-20261026.11')
  assert.equal(tag('20261027', ['canary-20261026.10']), 'canary-20261027.1')
  assert.equal(tag('20261027', ['canary-20261027.3--OpenResearch.dmg', 'canary-20261027.2']), 'canary-20261027.4')
})

test('the published manifest pins each desktop asset to its bytes and commit', () => {
  const dir = mkdtempSync(join(tmpdir(), 'orx-canary-manifest-'))
  try {
    mkdirSync(join(dir, 'artifacts'))
    const ancestry = join(dir, 'ancestry')
    writeFileSync(ancestry, `${'a'.repeat(40)}\n${'b'.repeat(40)}\n`)
    const files = ['OpenResearch.dmg', 'OpenResearch-Canary-orx.exe', 'OpenResearch-x86_64.AppImage', 'OpenResearch-aarch64.AppImage']
    for (const file of files) writeFileSync(join(dir, 'artifacts', file), file)
    const output = execFileSync(process.execPath, ['scripts/canary-manifest.mjs', join(dir, 'artifacts'), '0.2.21', 'canary-20261027.2', 'a'.repeat(40), ancestry], { encoding: 'utf8' })
    const manifest = JSON.parse(output)
    assert.equal(manifest.tag, 'canary-20261027.2')
    assert.equal(manifest.commit, 'a'.repeat(40))
    assert.deepEqual(manifest.ancestors, ['a'.repeat(40), 'b'.repeat(40)])
    assert.deepEqual(Object.keys(manifest.assets).sort(), ['linux-aarch64', 'linux-x86_64', 'macos', 'windows'])
    for (const asset of Object.values(manifest.assets)) {
      assert.equal(asset.sha256, createHash('sha256').update(asset.asset).digest('hex'))
    }
    mkdirSync(join(dir, 'partial'))
    writeFileSync(join(dir, 'partial', 'OpenResearch.dmg'), 'mac')
    const partial = execFileSync(process.execPath, ['scripts/canary-manifest.mjs', join(dir, 'partial'), '0.2.21', 'canary-20261027.2', 'a'.repeat(40), ancestry], { encoding: 'utf8' })
    assert.deepEqual(Object.keys(JSON.parse(partial).assets), ['macos'])
    const sharedName = 'canary-20261027.2--OpenResearch.dmg'
    writeFileSync(join(dir, 'partial', sharedName), 'shared')
    const shared = execFileSync(process.execPath, ['scripts/canary-manifest.mjs', join(dir, 'partial'), '0.2.21', 'canary-20261027.2', 'a'.repeat(40), ancestry, 'shared'], { encoding: 'utf8' })
    assert.deepEqual(JSON.parse(shared).assets.macos, { asset: sharedName, sha256: createHash('sha256').update('shared').digest('hex') })
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
