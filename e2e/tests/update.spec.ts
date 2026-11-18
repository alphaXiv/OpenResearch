import { readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { openTerminal } from '../src/terminal.ts'
import { expect, send, test } from '../src/test.ts'

// SIGUSR2 is how `node dev.ts restart` asks for a hand-off; Windows has no such signal, and dev.ts ends the
// process outright there, which the crash tests in restart.spec.ts cover.
test.skip(process.platform === 'win32', 'the hand-off signal is Unix-only')

const allow = (page: import('@playwright/test').Page) => page.getByRole('button', { name: 'Allow this action', exact: true })
const numbers = (text: string) => Array.from(text.matchAll(/\bN:(\d+)\r?$/gm), (match) => Number(match[1]))

test('an approval and a terminal in flight carry over to another executable with the first one deleted', async ({ page, agent, launchOrx }) => {
  const orx = await launchOrx({ ownBuild: true })
  await orx.onboard()
  const project = await orx.createProject('update')
  agent.script('Inspect the data.', [{ permission: 'Read the training data' }, { say: 'Data inspection finished.' }])
  await project.open(page)
  await send(page, 'Inspect the data.')
  await expect(allow(page)).toBeVisible()

  // The loop runs in the shell's background so the shell still takes input after the restart.
  const before = await openTerminal(orx, project.id, 'one')
  before.write('echo SHELL=$$; (i=0; while :; do i=$((i+1)); echo "N:$i"; sleep 0.05; done) &\n')
  await expect.poll(() => numbers(before.output()).length, { message: 'the loop is printing' }).toBeGreaterThan(5)
  const shell = Number(/SHELL=(\d+)\r?$/m.exec(before.output())![1])
  const printed = numbers(before.output()).at(-1)!

  const binary = orx.replaceBuild()
  await orx.restart('SIGUSR2', { binary })
  before.close()

  const after = await openTerminal(orx, project.id, 'one')
  await expect.poll(() => numbers(after.output()).at(-1) ?? 0, { message: 'the loop went on past the restart' }).toBeGreaterThan(printed + 20)
  const counted = numbers(after.output())
  expect(counted, 'every number once, in order, from the first').toEqual(Array.from({ length: counted.length }, (_, index) => index + 1))
  after.write('echo AGAIN=$$\n')
  await expect.poll(() => /AGAIN=(\d+)\r?$/m.exec(after.output())?.[1], { message: 'the shell answers' }).toBe(String(shell))
  expect(() => process.kill(shell, 0), 'the shell never exited').not.toThrow()

  // A terminal the new server starts needs its own keeper, which the replaced files cannot provide.
  const fresh = await openTerminal(orx, project.id, 'two')
  fresh.write('echo FRESH=$((6*7))\n')
  await expect.poll(() => /FRESH=42\r?$/m.test(fresh.output())).toBe(true)
  after.close()
  fresh.close()

  await page.reload()
  await expect(allow(page)).toHaveCount(1)
  await allow(page).click()
  await expect(page.getByText('Data inspection finished.', { exact: true })).toBeVisible()
  await expect(page.getByText('This turn did not finish.')).toBeHidden()
  await page.reload()
  await expect(page.getByText('Data inspection finished.', { exact: true })).toHaveCount(1)
  await expect(allow(page)).toBeHidden()

  const log = readFileSync(orx.agentLog, 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const [pid, arrow, ...json] = line.split(' ')
      return { pid, arrow, message: JSON.parse(json.join(' ')) }
    })
  const asked = log.filter(({ arrow, message }) => arrow === '→' && message.method === 'session/request_permission')
  expect(asked, 'the agent asked once').toHaveLength(1)
  const answered = log.filter(({ arrow, message }) => arrow === '←' && message.method === undefined && message.id === asked[0]!.message.id)
  expect(answered, 'and was answered once, by the process that asked').toMatchObject([{ pid: asked[0]!.pid }])
})

test('a page restarting into an update keeps its overlay through the reload', async ({ page, launchOrx }) => {
  const orx = await launchOrx({ appWindow: true })
  await orx.onboard()
  const project = await orx.createProject('overlay')
  // An update on disk, as the updater records one; this build stands in for it, so the restart comes back
  // unchanged. Written before the page loads, since the server samples it only every ten seconds after.
  writeFileSync(path.join(orx.config, 'update-check-unknown.json'), JSON.stringify({ checked_at: 0, latest: '999.0.0', installed_version: '999.0.0', installed_tag: 'v999.0.0' }))
  await project.open(page)
  const restart = page.getByRole('button', { name: 'Restart now' })
  await expect(restart).toBeVisible()
  const server = orx.pid()

  // The next page's app never loads, so what shows after the reload is index.html's alone.
  await page.route('**/assets/*.js', (route) => route.abort())
  const reloaded = page.waitForEvent('framenavigated', (frame) => frame === page.mainFrame())
  await restart.click()
  const updating = page.getByRole('alertdialog', { name: 'Updating OpenResearch' })
  await expect(updating).toBeVisible()
  await reloaded
  await expect(updating).toBeVisible()
  await expect(updating).toContainText(/Restarting into \W*v999\.0\.0/)
  expect(orx.pid(), 'the window keeps the same server process, restarted in place').toBe(server)

  await page.unroute('**/assets/*.js')
  await page.reload()
  const unchanged = page.getByRole('alertdialog', { name: 'Update not installed' })
  await expect(unchanged).toContainText(/OpenResearch is still on \W*v/)
  await page.getByRole('button', { name: 'Close' }).click()
  await expect(unchanged).toBeHidden()
  await expect(restart).toBeVisible()
})
