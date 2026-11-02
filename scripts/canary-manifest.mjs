import { createHash } from 'node:crypto'
import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

const [directory, version, tag, commit, ancestryPath, shared] = process.argv.slice(2)
if (!directory || !version || !tag || !commit || !ancestryPath) throw new Error('Expected artifact directory, version, tag, commit, and ancestry')
const ancestors = readFileSync(ancestryPath, 'utf8').trim().split('\n')
if (ancestors[0] !== commit || ancestors.some((sha) => !/^[0-9a-f]{40}$/.test(sha))) throw new Error('Invalid canary ancestry')
const assets = {}
for (const [key, asset] of [
  ['macos', 'OpenResearch.dmg'],
  ['windows', 'OpenResearch-Canary-orx.exe'],
  ['linux-x86_64', 'OpenResearch-x86_64.AppImage'],
  ['linux-aarch64', 'OpenResearch-aarch64.AppImage'],
]) {
  const name = shared === 'shared' ? `${tag}--${asset}` : asset
  const path = join(directory, name)
  if (existsSync(path)) assets[key] = { asset: name, sha256: createHash('sha256').update(readFileSync(path)).digest('hex') }
}
if (!Object.keys(assets).length) throw new Error('No desktop artifacts were built')
console.log(JSON.stringify({ version, tag, commit, ancestors, assets }))
