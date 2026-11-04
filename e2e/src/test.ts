/**
 * Fixtures every spec imports instead of `@playwright/test`'s. A worker owns one onboarded
 * `orx up` that its tests share, each in a project of its own; `launchOrx` gives a test a server
 * of its own when it needs onboarding or a restart. @module
 */
import { readFileSync, rmSync, statSync } from 'node:fs'
import { test as base, expect, type Page } from '@playwright/test'
import { startAgentScripts, type Agent } from './agent.ts'
import { startOrx, type Orx, type Project } from './orx.ts'
import { listedEgress, startOutside, type Outside } from './outside.ts'

export { expect }

export const composer = (page: Page) => page.getByRole('textbox', { name: /^Message/ })

/** Types a chat message and sends it with Enter, as a user does. */
export async function send(page: Page, text: string) {
  await composer(page).fill(text)
  await composer(page).press('Enter')
}

const size = (file: string) => statSync(file, { throwIfNoEntry: false })?.size ?? 0
const since = (file: string, offset: number) => (size(file) ? readFileSync(file).subarray(offset).toString() : '')

export const test = base.extend<{ launchOrx: () => Promise<Orx>; project: Project }, { agent: Agent; outside: Outside; orx: Orx }>({
  agent: [
    async ({}, use) => {
      const agent = await startAgentScripts()
      await use(agent)
      await agent.close()
    },
    { scope: 'worker' },
  ],
  outside: [
    async ({}, use) => {
      const outside = await startOutside()
      await use(outside)
      await outside.close()
    },
    { scope: 'worker' },
  ],
  orx: [
    async ({ agent, outside }, use) => {
      const orx = await startOrx({ agent, outside })
      await orx.onboard()
      await use(orx)
      await orx.stop()
      if (!process.env.ORX_E2E_KEEP) rmSync(orx.root, { recursive: true, force: true })
    },
    { scope: 'worker', timeout: 60_000 },
  ],
  baseURL: async ({ orx }, use) => use(orx.url),

  launchOrx: async ({ agent, outside }, use, info) => {
    const launched: Orx[] = []
    await use(async () => {
      const orx = await startOrx({ agent, outside })
      launched.push(orx)
      return orx
    })
    for (const orx of launched) {
      await orx.stop()
      await info.attach('launched orx.log', { path: orx.log })
      if (size(orx.agentLog)) await info.attach('launched agent.log', { path: orx.agentLog })
      if (!process.env.ORX_E2E_KEEP && info.status === info.expectedStatus) rmSync(orx.root, { recursive: true, force: true })
    }
  },

  project: async ({ orx }, use, info) => use(await orx.createProject(`e2e-${info.testId.slice(-8)}-${info.repeatEachIndex}`)),

  /**
   * Attaches what the browser, the worker's server, the agent and the outside saw during the test.
   * Fails a passing test that left a script unplayed, sent the agent an unscripted prompt, or sent
   * anything off the machine that `listedEgress` does not name.
   */
  page: async ({ page, agent, outside, orx }, use, info) => {
    const offsets = { log: size(orx.log), agent: size(orx.agentLog), outside: outside.requests.length }
    const browser: string[] = []
    page.on('console', (message) => browser.push(`[${message.type()}] ${message.text()}`))
    page.on('pageerror', (error) => browser.push(`[pageerror] ${error.stack ?? error.message}`))

    await use(page)

    const attach = (name: string, body: string) => info.attach(name, { body, contentType: 'text/plain' })
    const requests = outside.requests.slice(offsets.outside)
    await attach('browser.log', browser.join('\n'))
    await attach('orx.log', since(orx.log, offsets.log))
    await attach('agent.log', since(orx.agentLog, offsets.agent))
    await attach('outside.log', requests.map(({ service, method, target }) => `${service} ${method} ${target}`).join('\n'))
    const problems = [
      ...agent.finish(),
      ...requests.filter((request) => !listedEgress(request)).map(({ method, target }) => `Unlisted request off the machine: ${method} ${target}`),
    ]
    if (!problems.length) return
    if (info.status !== info.expectedStatus) await attach('problems', problems.join('\n'))
    else throw new Error(problems.join('\n'))
  },
})
