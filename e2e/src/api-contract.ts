/**
 * The dashboard API as a protocol-8 dashboard calls it, frozen in `tests/contracts/dashboard-api-8.json`
 * and never regenerated: every later server must still answer it ([remote](../../arc/remote.md#two-versions-at-once)).
 * `ORX_E2E_RECORD_API=<file> node e2e/run.ts` appends each JSON call the pages send, and
 * `node e2e/src/api-contract.ts <file>` keeps one call per route. @module
 */
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import type { Page } from '@playwright/test'
import { FAKE_AGENT } from './orx.ts'

export const CONTRACT = path.join(import.meta.dirname, '..', '..', 'tests', 'contracts', 'dashboard-api-8.json')

export type Call = { method: string; path: string; body?: unknown }

const SANDBOX = /(?:\/private)?\/tmp\/orx-e2e-[^/"?&]+/g
const ENCODED_SANDBOX = /(?:%2Fprivate)?%2Ftmp%2Forx-e2e-[^%&"]+/gi
/** Paths that differ by machine, which a recording names by placeholder. */
const MACHINE = { '{node}': process.execPath, '{agent}': FAKE_AGENT }

/** As a path appears inside a JSON string, where Windows' backslashes are escaped. */
const inJson = (value: string) => JSON.stringify(value).slice(1, -1)

/** A recorded call's JSON or query text with this machine's paths, `root` standing for its sandbox. */
export function fill(text: string, root: string) {
  for (const [placeholder, value] of Object.entries({ ...MACHINE, '{root}': root })) {
    text = text.replaceAll(placeholder, inJson(value)).replaceAll(encodeURIComponent(placeholder), encodeURIComponent(value))
  }
  return text
}

export function recordApi(page: Page, file: string) {
  page.on('request', (request) => {
    const url = new URL(request.url())
    if (url.hostname !== '127.0.0.1' || !url.pathname.startsWith('/api/')) return
    const raw = request.postData()
    let body: unknown
    if (raw !== null) {
      // An upload's bytes are no request shape to keep.
      try {
        body = JSON.parse(raw)
      } catch {
        return
      }
    }
    const call: Call = { method: request.method(), path: url.pathname + url.search, ...(raw === null ? {} : { body }) }
    let text = JSON.stringify(call).replace(SANDBOX, '{root}').replace(ENCODED_SANDBOX, encodeURIComponent('{root}'))
    for (const [placeholder, value] of Object.entries(MACHINE)) text = text.replaceAll(inJson(value), placeholder)
    appendFileSync(file, text + '\n')
  })
}

/** A call's route, with ids and numbers in its path standing for any. */
const route = ({ method, path }: Call) =>
  `${method} ${path.split('?')[0].replace(/\/(?:[a-z]+_)?(?:[0-9a-f]{8}-[0-9a-f-]+|[0-9a-f]{16,}|\d+)(?=\/|$)/gi, '/:id')}`

if (import.meta.main) {
  const calls = readFileSync(process.argv[2], 'utf8').trim().split('\n').map((line) => JSON.parse(line) as Call)
  const byRoute = new Map<string, Call>()
  for (const call of calls) if (!byRoute.has(route(call))) byRoute.set(route(call), call)
  const kept = [...byRoute].sort(([a], [b]) => a.localeCompare(b)).map(([, call]) => call)
  writeFileSync(CONTRACT, JSON.stringify(kept, null, 2) + '\n')
  console.log(`${kept.length} routes from ${calls.length} calls`)
}
