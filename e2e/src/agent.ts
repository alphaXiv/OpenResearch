/** The test's side of the fake agent: scripts its turns and holds them at named points. @module */
import { createServer, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'

export type Step = { say: string } | { think: string } | { hold: string } | { run: string } | { exit: number }

type Hold = { reached: PromiseWithResolvers<void>; response?: ServerResponse }

export type Agent = Awaited<ReturnType<typeof startAgentScripts>>

export async function startAgentScripts() {
  const scripts: { match: string; steps: Step[] }[] = []
  const holds = new Map<string, Hold>()
  const unclaimed: string[] = []
  const hold = (name: string) => {
    let entry = holds.get(name)
    if (!entry) holds.set(name, (entry = { reached: Promise.withResolvers() }))
    return entry
  }

  const server = createServer(async (request, response) => {
    let body = ''
    for await (const chunk of request) body += chunk
    if (request.url === '/turn') {
      const { text } = JSON.parse(body) as { text: string }
      const index = scripts.findIndex(({ match }) => text.includes(match))
      if (index < 0) unclaimed.push(text)
      const steps = index < 0 ? [{ say: 'No e2e script matched this prompt.' }] : scripts.splice(index, 1)[0]!.steps
      response.setHeader('content-type', 'application/json').end(JSON.stringify(steps))
      return
    }
    const name = decodeURIComponent(request.url!.slice('/hold/'.length))
    const entry = hold(name)
    entry.response = response
    entry.reached.resolve()
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))

  return {
    url: `http://127.0.0.1:${(server.address() as AddressInfo).port}`,
    /** The next prompt containing `match` plays `steps`, once. */
    script(match: string, steps: Step[]) {
      scripts.push({ match, steps })
    },
    /** Resolves once a turn stops at `{ hold: name }`. */
    reached(name: string) {
      return hold(name).reached.promise
    },
    release(name: string) {
      const entry = holds.get(name)
      if (!entry?.response) throw new Error(`No turn is held at ${name}`)
      entry.response.end()
      holds.delete(name)
    },
    /** What the test left behind: scripts no prompt used, prompts no script matched. */
    finish() {
      const problems = [
        ...scripts.map(({ match }) => `No prompt used the script for ${JSON.stringify(match)}`),
        ...unclaimed.map((text) => `No script matched the prompt ${JSON.stringify(text.slice(-200))}`),
      ]
      scripts.length = 0
      unclaimed.length = 0
      for (const entry of holds.values()) entry.response?.end()
      holds.clear()
      return problems
    },
    close() {
      server.closeAllConnections()
      return new Promise((resolve) => server.close(resolve))
    },
  }
}
