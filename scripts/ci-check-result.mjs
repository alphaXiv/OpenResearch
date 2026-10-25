import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

export function checkResults(needs, flags) {
  if (needs.changes?.result !== 'success') throw new Error('Change detection did not succeed')
  const selected = name => {
    if (!['true', 'false'].includes(flags[name])) throw new Error(`Missing or invalid change flag: ${name}`)
    return flags[name] === 'true'
  }
  const expected = {
    frontend: selected('frontend'),
    rust: selected('rust'),
    windows: selected('windows'),
    'linux-app': selected('linux'),
    helpers: ['dev_slot', 'chart', 'release_notes', 'javascript'].map(selected).some(Boolean),
  }
  for (const [job, required] of Object.entries(expected)) {
    const result = needs[job]?.result
    if (result !== 'success' && (required || result !== 'skipped')) throw new Error(`${job}: ${result ?? 'missing'}${required ? ' (required)' : ''}`)
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  checkResults(JSON.parse(process.env.NEEDS_JSON), JSON.parse(process.env.FLAGS_JSON))
}
