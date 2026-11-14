/**
 * Screenshots of what the watcher shows for one scenario from `scenarios.ts`, or for `sweep`, three healthy
 * siblings launched in one message: the runs above the composer while they run, a tick's tooltip, and the
 * experiment page's Watch section once they end, each in the light and the dark theme.
 *
 *   node e2e/faults/shots.ts wrong-depth --out .shots/w9-watch
 *   node e2e/faults/shots.ts sweep --watcher real --out .shots/w9-watch
 *
 * Needs what `run.ts` needs. @module
 */
import { spawnSync } from 'node:child_process'
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { chromium, type Locator, type Page } from '@playwright/test'
import { startAgentScripts } from '../src/agent.ts'
import { startOutside } from '../src/outside.ts'
import { scenarios } from './scenarios.ts'

const ROOT = path.resolve(import.meta.dirname, '..', '..')
const BINARY = process.env.ORX_E2E_BINARY ?? path.join(ROOT, 'target', 'debug', 'orx')
process.env.ORX_E2E_BINARY = BINARY
const { startOrx } = await import('../src/orx.ts')
const PAYLOAD = path.join(import.meta.dirname, 'payload.mjs')
const VIEW = path.join(import.meta.dirname, 'view.xml')
const RULES = path.join(import.meta.dirname, 'rules.json')

const args = process.argv.slice(2)
const option = (name: string, fallback: string) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback)
const watcher = option('--watcher', 'fake')
const out = path.resolve(option('--out', path.join(ROOT, '.shots', 'w9-watch')))
const id = args.find((arg, index) => !arg.startsWith('--') && !args[index - 1]?.startsWith('--'))
const scenario = id === 'sweep' ? null : scenarios.find((s) => s.id === id)
if (!id || (id !== 'sweep' && !scenario)) throw new Error('Name a scenario from scenarios.ts, or sweep')
mkdirSync(out, { recursive: true })

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, Math.max(0, ms)))
const realClaude = spawnSync('/bin/sh', ['-c', 'command -v claude'], { encoding: 'utf8' }).stdout.trim()
const claude = watcher === 'real'
  ? `#!/bin/sh\n# The sandbox sends every request off the machine to its stand-in; a real look goes to Anthropic.\nunset HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy\nexec ${realClaude} "$@"\n`
  : `#!/bin/sh\nexec ${process.execPath} ${path.join(import.meta.dirname, 'watcher.mjs')} "$@"\n`

const agent = await startAgentScripts()
const outside = await startOutside()
/** A real look signs in as the shell that started the shots does: with its API key, if it has one. */
const signIn = Object.fromEntries(['ANTHROPIC_API_KEY', 'ANTHROPIC_BASE_URL'].flatMap((name) => watcher === 'real' && process.env[name] ? [[name, process.env[name]!]] : []))
const orx = await startOrx({ agent, outside, claude, env: { ORX_WATCH_WITH: 'claude-code', ...signIn } })
const browser = await chromium.launch()

/** `target` in both themes, as `<name>-light.png` and `<name>-dark.png`. */
async function shoot(page: Page, target: Locator, name: string) {
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme })
    await wait(300)
    await target.screenshot({ path: path.join(out, `${name}-${scheme}.png`), animations: 'disabled' })
  }
}

const holds: string[] = []
const release = () => {
  for (const hold of holds.splice(0)) {
    try {
      agent.release(hold)
    } catch {
      // A result the agent was never asked about holds nothing.
    }
  }
}

try {
  await orx.onboard()
  const project = await orx.createProject(`shots-${id}`)
  const page = await browser.newPage({ viewport: { width: 1100, height: 900 } })
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  const shown = async () => ((await (await fetch(`${orx.url}/api/chat/shown-runs`)).json()) as { run: { id: string; experimentId: string; projectId: string; status: string } }[])
    .filter((item) => item.run.projectId === project.id)

  /** One chat message that launches each of `runs` and waits for the last to start. */
  const launch = async (message: string, runs: { title: string; variant: string; parent?: string; brief?: Record<string, string> }[]) => {
    const lines = ['set -e']
    for (const [index, run] of runs.entries()) {
      const config = path.join(orx.root, 'tmp', `config-${index}.json`)
      if (run.brief) writeFileSync(config, JSON.stringify({ brief: run.brief }))
      lines.push(
        `id=$(orx create-experiment ${project.id} --title '${run.title}' ${run.parent ? `--parent ${run.parent} ` : ''}--run-command '${process.execPath} ${PAYLOAD} ${run.variant}' | sed -n 's/^ *id: *//p')`,
        ...(run.parent ? [] : [`orx exp view set "$id" --file ${VIEW} --config ${RULES}`]),
        ...(run.brief ? [`orx exp view set "$id" --config ${config}`] : []),
        'orx exp run "$id" --backend local',
        'orx exp wake "$id"',
      )
    }
    agent.script(message, [{ run: lines.join('\n') }, { say: 'Launched.' }])
    // The agent answers each result only once the shots are taken, so ended runs stay above the composer.
    for (const run of runs) {
      holds.push(`result-${run.title}`)
      agent.script('finished with status', [{ hold: `result-${run.title}` }, { say: `${run.title} is over.` }])
    }
    await page.getByRole('textbox', { name: /^Message/ }).fill(message)
    await page.keyboard.press('Enter')
    await banner.getByRole('button', { name: /Stop the run|Show each run|Show less/ }).first().waitFor({ timeout: 60_000 })
  }

  await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
  await project.open(page)
  await launch('Launch the parent.', [{ title: 'Parent', variant: 'healthy' }])
  const parent = (await shown())[0]
  while ((await shown()).some((item) => !['done', 'failed', 'cancelled'].includes(item.run.status))) await wait(1000)
  await agent.reached(holds[0])
  release()
  await banner.waitFor({ state: 'detached', timeout: 60_000 })

  const children = scenario
    ? [{ title: scenario.title, variant: scenario.id, parent: parent.run.experimentId, brief: scenario.brief }]
    : ['Depth 4 at 2e15 FLOPs', 'Depth 6 at 2e15 FLOPs', 'Depth 8 at 2e15 FLOPs'].map((title) => ({ title, variant: 'healthy', parent: parent.run.experimentId }))
  await launch(`Launch ${id}.`, children)
  const started = Date.now()
  for (const [at, label] of [[14, 'early'], [36, 'middle']] as const) {
    await wait(started + at * 1000 - Date.now())
    await shoot(page, banner, `${id}-banner-${label}`)
  }
  // A tick's tooltip: what that look said, and what else it covered.
  const tick = banner.locator('[aria-hidden="true"] > span:visible').last()
  if (await tick.count()) {
    await tick.hover({ timeout: 5000 })
    await wait(600)
    await page.screenshot({ path: path.join(out, `${id}-tick.png`), clip: await banner.boundingBox().then((box) => box && { x: box.x, y: Math.max(0, box.y - 80), width: box.width, height: box.height + 80 }) ?? undefined })
  }
  while ((await shown()).some((item) => !['done', 'failed', 'cancelled'].includes(item.run.status))) await wait(1000)
  // Each run's last look waits for its log, then for siblings ending with it.
  await wait(8000)

  const first = (await shown()).find((item) => item.run.experimentId !== parent.run.experimentId) ?? (await shown())[0]
  const slug = children[0].title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/-$/, '')
  // A lone run's name is its experiment's slug; one of several says its whole title.
  const label = children[0].title
  await banner.getByRole('button', { name: new RegExp(`^\\W*(${slug}|${label})\\W*$`) }).first().click()
  const watch = page.locator('section', { has: page.getByRole('heading', { name: 'Watch', exact: true }) })
  await watch.waitFor({ timeout: 30_000 })
  await watch.scrollIntoViewIfNeeded()
  await shoot(page, watch, `${id}-journal`)
  const latest = page.locator('section', { has: page.getByRole('heading', { name: 'Latest run', exact: true }) })
  await shoot(page, latest, `${id}-page`)
  console.log(`${id}: shots of run ${first?.run.id} in ${out}`)
  release()
} finally {
  await browser.close()
  await orx.stop()
  await agent.close()
  await outside.close()
  rmSync(orx.root, { recursive: true, force: true })
}
