import { readFileSync } from 'node:fs'
import { CONTRACT, fill, type Call } from '../src/api-contract.ts'
import { expect, test } from '../src/test.ts'

// What only the router and axum's extractors answer: a route the server lacks gets its fallback's
// 404 or a bare 405, and a request it can't read gets one of these, never a handler's own message.
const REJECTION = /^(Failed to (deserialize|parse|buffer)|Expected request with|Invalid URL|Wrong number of path arguments|Cannot parse)/
const MISSING_ROUTE = /^(|\{"error":"route not found"\})$/
const refusal = (status: number, text: string) =>
  ((status === 404 || status === 405) && MISSING_ROUTE.test(text)) || REJECTION.test(text)

test('this server answers every call a protocol-8 dashboard makes', async ({ launchOrx }) => {
  const orx = await launchOrx()
  await orx.onboard()
  const calls = JSON.parse(readFileSync(CONTRACT, 'utf8')) as Call[]

  const refused: string[] = []
  // The last call proves a missing route is still told apart from a handler's own 404.
  for (const call of [...calls, { method: 'GET', path: '/api/no-such-route' }]) {
    // A HEAD answer has no body to tell the router's 404 from a handler's.
    const method = call.method === 'HEAD' ? 'GET' : call.method
    const response = await fetch(`${orx.url}${fill(call.path, orx.root)}`, {
      method,
      headers: { origin: orx.url, ...(call.body === undefined ? {} : { 'content-type': 'application/json' }) },
      body: call.body === undefined ? undefined : fill(JSON.stringify(call.body), orx.root),
    })
    if (response.status < 400 || response.status >= 500) {
      await response.body?.cancel()
      continue
    }
    const text = await response.text()
    if (refusal(response.status, text)) refused.push(`${call.method} ${call.path}: ${response.status} ${text || '(no body)'}`)
  }
  expect(refused, 'routes a protocol-8 dashboard calls are gone, or refuse what it sends').toEqual([
    expect.stringMatching(/^GET \/api\/no-such-route: 40[45] /),
  ])
})
