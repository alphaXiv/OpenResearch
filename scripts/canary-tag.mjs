import { readFileSync } from 'node:fs'

const date = process.argv[2]
if (!/^\d{8}$/.test(date ?? '')) throw new Error('Expected a UTC date in YYYYMMDD form')
const releases = JSON.parse(readFileSync(0, 'utf8'))
if (!Array.isArray(releases)) throw new Error('Expected a list of release tags')
const prefix = `canary-${date}.`
const sequences = releases.flatMap((tag) => {
  if (typeof tag !== 'string' || !tag.startsWith(prefix)) return []
  const sequence = tag.slice(prefix.length).split('--', 1)[0]
  return /^[1-9]\d*$/.test(sequence) && Number.isSafeInteger(Number(sequence)) ? [Number(sequence)] : []
})
console.log(`${prefix}${Math.max(0, ...sequences) + 1}`)
