/**
 * Runs scenarios from `scenarios.ts` against the real `orx up` in a sandbox (as the e2e suite does)
 * and prints what orx showed at fixed seconds after each broken run started: the line, the strip, the
 * page, `orx exp status` and what the monitor says of flags and the watcher.
 *
 *   node e2e/faults/run.ts lr-x100 nan-400   # these scenarios; none runs them all, one after another
 *   node e2e/faults/run.ts --list
 *   node e2e/faults/run.ts hang --keep       # leave the server up on a port printed at the end
 *
 * Needs a built `orx` (cargo build) and dashboard (pnpm exec vite build in ui/); ORX_E2E_BINARY names
 * another binary. The timeline goes to stdout and to `.shots/faults/<id>.txt` in the checkout. @module
 */
import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { chromium } from '@playwright/test'
import { startAgentScripts } from '../src/agent.ts'
import { startOutside } from '../src/outside.ts'
import { scenarios, type Scenario } from './scenarios.ts'

const ROOT = path.resolve(import.meta.dirname, '..', '..')
const BINARY = process.env.ORX_E2E_BINARY ?? path.join(ROOT, 'target', 'debug', 'orx')
process.env.ORX_E2E_BINARY = BINARY
// Reads the binary's path when it loads.
const { startOrx } = await import('../src/orx.ts')
const PAYLOAD = path.join(import.meta.dirname, 'payload.mjs')
const VIEW = path.join(import.meta.dirname, 'view.xml')
const OUT = path.join(ROOT, '.shots', 'faults')

const args = process.argv.slice(2)
if (args.includes('--list')) {
  for (const { id, fault } of scenarios) console.log(`${id.padEnd(14)} ${fault}`)
  process.exit(0)
}
const keep = args.includes('--keep')
const wanted = args.filter((arg) => !arg.startsWith('--'))
const unknown = wanted.filter((id) => !scenarios.some((scenario) => scenario.id === id))
if (unknown.length) throw new Error(`No scenario ${unknown.join(', ')}; node e2e/faults/run.ts --list names them`)
if (!existsSync(BINARY)) throw new Error(`No orx at ${BINARY}; run cargo build first, or set ORX_E2E_BINARY`)
mkdirSync(OUT, { recursive: true })

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, Math.max(0, ms)))
/** Direction marks the page puts around numbers and names; text compares without them. */
const plain = (text: string) => text.replace(/[⁦-⁩]/g, '').replace(/\s*\n\s*/g, ' | ').replace(/\s+/g, ' ').trim()

/** The newest message the agent was sent that says a run finished: what wakes a chat when its run ends. */
function wakeText(agentLog: string): string | null {
  const wakes = readFileSync(agentLog, 'utf8').split('\n').filter((line) => line.includes('finished with status'))
  const line = wakes.at(-1)
  if (!line) return null
  const message = JSON.parse(line.slice(line.indexOf('{')))
  return message.params.prompt.map((part: { text?: string }) => part.text ?? '').join('')
}

type Shown = { run: { id: string; experimentId: string; projectId: string; status: string; startedAt: number | null; monitor?: { tone: string } } }

async function runScenario(scenario: Scenario) {
  const lines: string[] = []
  const say = (line = '') => {
    lines.push(line)
    console.log(line)
  }

  const agent = await startAgentScripts()
  const outside = await startOutside()
  const orx = await startOrx({ agent, outside })
  const browser = await chromium.launch()
  try {
    await orx.onboard()
    const project = await orx.createProject(`faults-${scenario.id}`)
    const page = await browser.newPage()
    const banner = page.getByRole('region', { name: 'Runs in this chat' })
    const home = path.join(orx.root, 'home')
    const env = {
      HOME: home,
      XDG_CONFIG_HOME: path.join(home, '.config'),
      XDG_DATA_HOME: path.join(home, '.local', 'share'),
      XDG_CACHE_HOME: path.join(home, '.cache'),
      TMPDIR: path.join(orx.root, 'tmp'),
      PATH: `${path.dirname(BINARY)}:/usr/bin:/bin:/usr/sbin:/sbin`,
    }
    const cli = (...cliArgs: string[]) => spawnSync(BINARY, cliArgs, { env, encoding: 'utf8' })
    const get = async <T>(route: string) => (await (await fetch(`${orx.url}${route}`)).json()) as T
    const shown = async () => (await get<Shown[]>('/api/chat/shown-runs')).find((item) => item.run.projectId === project.id)

    const launch = async (title: string, variant: string, parent?: string) => {
      const command = `${process.execPath} ${PAYLOAD} ${variant}`
      agent.script(`Launch ${title}.`, [
        { run: [
          'set -e',
          `id=$(orx create-experiment ${project.id} --title '${title}' ${parent ? `--parent ${parent} ` : ''}--run-command '${command}' | sed -n 's/^ *id: *//p')`,
          ...(parent ? [] : [`orx exp view set "$id" --file ${VIEW}`]),
          'orx exp run "$id" --backend local',
          'orx exp wake "$id"',
        ].join('\n') },
        { say: `Launched ${title}.` },
      ])
      const hold = `result-${title}`
      agent.script('finished with status', [{ hold }, { say: `${title} is over.` }])
      await page.getByRole('textbox', { name: /^Message/ }).fill(`Launch ${title}.`)
      await page.keyboard.press('Enter')
      await banner.getByRole('button', { name: 'Stop the run', exact: true }).waitFor({ timeout: 60_000 })
      return hold
    }

    await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
    await project.open(page)

    say(`## ${scenario.id}: ${scenario.title}`)
    say(scenario.fault)
    say()

    const parentHold = await launch('Parent', 'healthy')
    const parent = (await shown())!
    await agent.reached(parentHold)
    const parentMonitor = await get<{ latest: Record<string, unknown>; statusText: string }>(`/api/runs/${parent.run.id}/monitor`)
    say(`parent (healthy): ${plain(await banner.innerText())}`)
    say(`parent latest: ${JSON.stringify(parentMonitor.latest)}`)
    agent.release(parentHold)
    await banner.waitFor({ state: 'detached', timeout: 30_000 })
    say()

    const launched = Date.now()
    const hold = await launch(scenario.title, scenario.id, parent.run.experimentId)
    let child = (await shown())!
    // A busy machine can take a while to submit the run; the points count from when it started.
    while (!child.run.startedAt) {
      await wait(250)
      child = (await shown())!
    }
    const origin = child.run.startedAt
    say(`child (${scenario.id}) started ${((origin - launched) / 1000).toFixed(1)} s after its chat message`)

    /** The banner at the level it is at now, then at the other one, ending where it began. */
    const levels = async () => {
      const strip = plain(await banner.innerText())
      const less = banner.getByRole('button', { name: 'Show less', exact: true })
      await less.click()
      await banner.getByRole('button', { name: 'Show more', exact: true }).waitFor()
      const line = plain(await banner.innerText())
      await banner.getByRole('button', { name: 'Show more', exact: true }).click()
      await less.waitFor()
      return { strip, line }
    }

    const sample = async (label: string) => {
      const [run, monitor] = await Promise.all([
        shown(),
        get<Record<string, any>>(`/api/runs/${child.run.id}/monitor`),
      ])
      const charts = await banner.getByRole('img', { name: /^[\w.]+ \S/ }).evaluateAll((nodes) => nodes.map((node) => node.getAttribute('aria-label') ?? ''))
      const { strip, line } = await levels()
      say(`── ${label} ${'─'.repeat(Math.max(0, 70 - label.length))}`)
      say(`line   ${line}`)
      say(`strip  ${strip}`)
      if (charts.length) say(`charts ${charts.map(plain).join(' · ')}`)
      say(`status ${plain(String(monitor.statusText).replace(/^signals +(Last collected|Data as of).*$/gm, ''))}`)
      say(`state  run=${run?.run.status ?? 'gone'} tone=${run?.run.monitor?.tone ?? '-'} flag=${JSON.stringify(monitor.flag)} sentence=${JSON.stringify(monitor.sentence)} watch=${monitor.watch?.state}/${monitor.watch?.looks?.length ?? 0} looks rules=${monitor.rules?.length ?? 0} setup=${JSON.stringify(monitor.setup)} parent=${monitor.parent ? 'yes' : 'no'} versus=${JSON.stringify(monitor.versus)}`)
    }

    const ended = async () => !(await banner.getByRole('button', { name: 'Stop the run', exact: true }).count())

    for (const point of scenario.points) {
      await wait(origin + point * 1000 - Date.now())
      await sample(`+${point}s`)
      if (await ended()) break
    }

    while (!(await ended()) && Date.now() < origin + scenario.stopAt * 1000) await wait(1000)
    if (!(await ended())) {
      say()
      say(`still running at +${scenario.stopAt}s: stopped as its agent would (orx exp cancel)`)
      cli('exp', 'cancel', child.run.experimentId)
    }
    await agent.reached(hold)
    await sample('ended')

    // The experiment page, which its name opens, shows the whole run, a moment after the run ends.
    await wait(3000)
    const slug = scenario.title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/-$/, '')
    await banner.getByRole('button', { name: new RegExp(`^\\W?${slug}\\W?$`) }).click()
    const section = page.locator('section', { has: page.getByRole('heading', { name: 'Latest run', exact: true }) })
    await section.waitFor()
    say(`page   ${plain(await section.innerText())}`)
    say(`log    ${plain(cli('logs', child.run.id).stdout)}`)
    const wake = wakeText(orx.agentLog)
    if (wake) {
      const rows = wake.split('\n')
      say(`wake   ${rows.length} lines to the agent: ${rows.slice(0, 5).join(' | ')} ... ${rows.slice(-7).map((row) => row.slice(0, 160)).join(' | ')}`)
    }
    const status = cli('exp', 'status', child.run.experimentId)
    say(`orx exp status:`)
    for (const line of (status.stdout + status.stderr).trimEnd().split('\n')) say(`  ${line}`)
    agent.release(hold)

    if (keep) {
      say()
      say(`server kept: ${orx.url}/projects/${project.id} (Ctrl-C ends it; its files stay in ${orx.root})`)
      writeFileSync(path.join(OUT, `${scenario.id}.txt`), `${lines.join('\n')}\n`)
      await new Promise(() => undefined)
    }
  } finally {
    writeFileSync(path.join(OUT, `${scenario.id}.txt`), `${lines.join('\n')}\n`)
    await browser.close()
    await orx.stop()
    await agent.close()
    await outside.close()
    if (!keep) rmSync(orx.root, { recursive: true, force: true })
  }
}

for (const scenario of wanted.length ? scenarios.filter((s) => wanted.includes(s.id)) : scenarios) {
  await runScenario(scenario)
  console.log()
}
