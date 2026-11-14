import { spawnSync } from 'node:child_process'
import { writeFile } from 'node:fs/promises'
import path from 'node:path'
import { expect, send, test } from '../src/test.ts'

for (const dismiss of [false, true]) {
  test(`an ended run stays until ${dismiss ? 'dismissed in both windows' : 'its result reply completes'}`, async ({ page, browser, project, agent, orx }) => {
    const badLevel = await page.request.post(`${orx.url}/api/settings/ui-state`, {
      headers: { origin: orx.url }, data: { runBanner: 'whole' },
    })
    expect(badLevel.status()).toBe(400)
    const missing = await page.request.post(`${orx.url}/api/chat/sessions/missing/shown-runs/missing/dismiss`, { headers: { origin: orx.url } })
    expect(missing.status()).toBe(404)
    const initial = await page.request.post(`${orx.url}/api/settings/ui-state`, {
      headers: { origin: orx.url }, data: { runBanner: 'strip' },
    })
    expect(initial.status()).toBe(200)
    const gate = path.join(orx.root, `finish-${project.id}`)
    const fifo = spawnSync('mkfifo', [gate])
    expect(fifo.status).toBe(0)
    agent.script('Launch the monitored run.', [
      { run: [
        'set -e',
        `id=$(orx create-experiment ${project.id} --title Monitored --run-command 'cat ${gate}; echo loss=0.125' | sed -n 's/^ *id: *//p')`,
        'orx exp run "$id" --backend local',
        'orx exp wake "$id"',
      ].join('\n') },
      { say: 'Launched the monitored run.' },
    ])
    const hold = `result-${project.id}`
    agent.script('finished with status **done**', [{ hold }, { say: 'The monitored result is ready.' }])
    await project.open(page)
    await send(page, 'Launch the monitored run.')
    const banner = page.getByRole('region', { name: 'Runs in this chat' })
    await expect(banner.getByRole('button', { name: 'Stop the run', exact: true })).toBeVisible()
    await expect(page.getByText('Launched the monitored run.', { exact: true })).toBeVisible()
    await expect(banner.getByRole('img', { name: 'Running', exact: true })).toBeVisible()
    const rows = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
    const row = rows.find((item: { run: { projectId: string } }) => item.run.projectId === project.id)
    const refusal = await page.request.post(`${orx.url}/api/chat/sessions/${row.sessionId}/shown-runs/${row.run.id}/dismiss`, { headers: { origin: orx.url } })
    expect(refusal.status()).toBe(409)
    const context = await browser.newContext()
    const other = await context.newPage()
    try {
      await other.goto(page.url())
      const otherBanner = other.getByRole('region', { name: 'Runs in this chat' })
      await expect(otherBanner).toBeVisible()
      await writeFile(gate, 'finish\n')
      await agent.reached(hold)
      await expect(banner.getByRole('button', { name: 'Dismiss', exact: true })).toBeVisible()
      await expect(banner).toContainText('Done')
      await expect(otherBanner.getByRole('button', { name: 'Dismiss', exact: true })).toBeVisible()
      const active = await (await page.request.get(`${orx.url}/api/chat/active-runs`)).json()
      expect(active.some((item: { run: { id: string } }) => item.run.id === row.run.id)).toBe(false)
      const saved = page.waitForResponse((response) => response.request().method() === 'POST' && response.url().endsWith('/api/settings/ui-state'))
      await banner.getByRole('button', { name: 'Show less', exact: true }).click()
      expect((await saved).status()).toBe(200)
      await expect(banner).toContainText('Done')
      await expect(banner.getByRole('button', { name: /monitored/i, exact: true })).toBeVisible()
      const preferences = await (await page.request.get(`${orx.url}/api/settings/ui-state`)).json()
      expect(preferences.runBanner).toBe('line')
      await other.reload()
      await expect(otherBanner.getByRole('button', { name: 'Show more', exact: true })).toBeVisible()
      if (dismiss) {
        await banner.getByRole('button', { name: 'Dismiss', exact: true }).click()
        await expect(banner).toHaveCount(0)
        await expect(otherBanner).toHaveCount(0)
      }
      agent.release(hold)
      await expect(page.getByText('The monitored result is ready.', { exact: true })).toBeVisible()
      await expect(banner).toHaveCount(0)
      await expect(otherBanner).toHaveCount(0)
    } finally {
      await context.close()
      await page.request.post(`${orx.url}/api/settings/ui-state`, { headers: { origin: orx.url }, data: { runBanner: 'strip' } })
    }
  })
}

test('a server restart keeps an ended run whose result reply was interrupted', async ({ page, agent, launchOrx }) => {
  const server = await launchOrx()
  await server.onboard()
  const project = await server.createProject('shown-restart')
  agent.script('Launch before restart.', [{ run: [
    'set -e',
    `id=$(orx create-experiment ${project.id} --title Restarted --run-command 'echo finished' | sed -n 's/^ *id: *//p')`,
    'orx exp run "$id" --backend local',
    'orx exp wake "$id"',
  ].join('\n') }, { say: 'Launched before restart.' }])
  agent.script('finished with status **done**', [{ hold: 'restart-result' }])
  await project.open(page)
  await send(page, 'Launch before restart.')
  await agent.reached('restart-result')
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  await expect(banner.getByRole('button', { name: 'Dismiss', exact: true })).toBeVisible()
  await server.restart('SIGKILL')
  agent.release('restart-result')
  await page.reload()
  await expect(banner.getByRole('button', { name: 'Dismiss', exact: true })).toBeVisible()
  await banner.getByRole('button', { name: 'Dismiss', exact: true }).click()
  await expect(banner).toHaveCount(0)
})

test('a failed result reply keeps the line until Continue gets one', async ({ page, project, agent }) => {
  agent.script('Launch then fail.', [{ run: [
    'set -e',
    `id=$(orx create-experiment ${project.id} --title Continued --run-command 'echo finished' | sed -n 's/^ *id: *//p')`,
    'orx exp run "$id" --backend local',
    'orx exp wake "$id"',
  ].join('\n') }, { say: 'Launched, then fail.' }])
  agent.script('finished with status **done**', [{ exit: 1 }])
  await project.open(page)
  await send(page, 'Launch then fail.')
  const banner = page.getByRole('region', { name: 'Runs in this chat' })
  await expect(page.getByText('This turn did not finish.')).toBeVisible()
  await expect(banner.getByRole('button', { name: 'Dismiss', exact: true })).toBeVisible()
  agent.script('finished with status **done**', [{ say: 'The recovered result is ready.' }])
  await page.getByText('This turn did not finish.').click()
  await page.getByRole('button', { name: 'Continue', exact: true }).click()
  await expect(page.getByText('The recovered result is ready.', { exact: true })).toBeVisible()
  await expect(banner).toHaveCount(0)
})
