import { mkdir, readFile, rename, rm, utimes, writeFile } from 'node:fs/promises'
import path from 'node:path'
import type { Locator, Page } from '@playwright/test'
import { expect, send, test } from '../src/test.ts'

async function capture(page: Page, gallery: Locator, surface: string) {
  const shots = process.env.ORX_E2E_MEDIA_SHOTS
  if (!shots) return
  await mkdir(shots, { recursive: true })
  for (const width of [1440, 420]) {
    await page.setViewportSize({ width, height: 1000 })
    await gallery.scrollIntoViewIfNeeded()
    for (const colorScheme of ['light', 'dark'] as const) {
      await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
      await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
      await page.screenshot({ animations: 'disabled', path: path.join(shots, `${surface}-${colorScheme}-${width}.png`) })
    }
  }
  await page.setViewportSize({ width: 1440, height: 1000 })
}

test('an authored media gallery follows workspace images on the run page and in its citation', async ({ page, project, agent, orx }) => {
  test.slow()
  await page.clock.install()
  const view = path.join(orx.root, 'samples.xml')
  await writeFile(view, '<view><media title="Simulation samples" glob="samples/*.png" last="3" /></view>')
  agent.script('Run the simulation.', [
    { run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title 'Simulation' --run-command 'echo Simulation finished' | sed -n 's/^ *id: *//p')`,
      `orx exp view set "$id" --file ${view}`,
      'orx exp run "$id" --backend local',
      'orx exp wake "$id"',
    ].join('\n') },
    { say: 'Simulation launched.' },
  ])
  const hold = `samples-result-${project.id}`
  agent.script('finished with status **done**', [{ hold }, { say: 'Simulation complete.' }])
  await project.open(page)
  await send(page, 'Run the simulation.')
  await agent.reached(hold)
  const shown = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json() as
    { experimentName: string; run: { id: string } }[]
  const run = shown.find((entry) => entry.experimentName === 'Simulation')!.run.id
  agent.release(hold)
  await expect(page.getByText('Simulation complete.', { exact: true })).toBeVisible()
  agent.script('Cite the samples.', [{ say: `Simulation samples follow. <run id="${run}" label="Simulation result" figure/>` }])
  await send(page, 'Cite the samples.')

  const citation = page.locator('.md').filter({ hasText: 'Simulation samples follow.' })
  const gallery = citation.getByRole('region', { name: 'Simulation samples' })
  await expect(gallery).toContainText('No files matched')
  const monitorRequests: string[] = []
  const recordMonitor = (request: { url(): string }) => {
    if (new URL(request.url()).pathname === `/api/runs/${run}/monitor`) monitorRequests.push(request.url())
  }
  page.on('request', recordMonitor)
  await page.clock.runFor(5_000)
  page.off('request', recordMonitor)
  expect(monitorRequests).toEqual([])
  const { projects } = await (await page.request.get(`${orx.url}/api/projects`)).json() as
    { projects: { id: string; artifactsDir: string }[] }
  const root = projects.find(({ id }) => id === project.id)!.artifactsDir
  const samples = path.join(root, 'runs', run, 'samples')
  await mkdir(samples, { recursive: true })
  const imgs = await page.evaluate(() => [0.6, 1.2, 1.8, 2.4].map((phase) => {
    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 168
    const context = canvas.getContext('2d')!
    for (let y = 0; y < 168; y += 4) {
      for (let x = 0; x < 168; x += 4) {
        const radius = Math.hypot(x - 84, y - 84)
        const wave = Math.sin(radius / 8 - phase) * Math.cos(y / 15 + x / 22)
        context.fillStyle = `hsl(${188 + wave * 50}, 74%, ${46 + wave * 35}%)`
        context.fillRect(x, y, 4, 4)
      }
    }
    return canvas.toDataURL().split(',')[1]
  }))
  for (const [index, data] of imgs.entries()) {
    const draft = path.join(samples, 'draft.tmp')
    await writeFile(draft, Buffer.from(data, 'base64'))
    await utimes(draft, 1_784_000_000 + index, 1_784_000_000 + index)
    await rename(draft, path.join(samples, `Frame ${index + 1}.png`))
  }
  const thumbnail = gallery.getByRole('img', { name: 'Frame 4.png' })
  await thumbnail.scrollIntoViewIfNeeded()
  await expect(gallery.getByRole('img')).toHaveCount(3)
  await expect(thumbnail).toHaveJSProperty('naturalWidth', 168)
  const old = (await thumbnail.getAttribute('src'))!
  const image = await page.request.get(old)
  expect(image.headers()['content-type']).toBe('image/png')
  expect(image.headers()['x-content-type-options']).toBe('nosniff')
  const outside = new URL(old, orx.url)
  outside.searchParams.set('path', '../samples/Frame 4.png')
  expect((await page.request.get(outside.href)).status()).toBe(404)

  const trigger = gallery.getByRole('button', { name: /Open .*Frame 4\.png/ })
  await trigger.focus()
  await trigger.press('Enter')
  const viewer = page.getByRole('dialog', { name: 'Frame 4.png' })
  await expect(viewer.getByRole('img')).toHaveJSProperty('naturalWidth', 168)
  await expect(viewer.getByRole('button', { name: 'Zoom in' })).toBeVisible()
  await expect(viewer).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog')).toHaveCount(0)
  await expect(trigger).toBeFocused()
  await capture(page, gallery, 'citation')

  const checks = path.join(orx.root, 'samples-check.txt')
  const description = path.join(orx.root, 'samples-show.txt')
  agent.script('Inspect the sample gallery.', [
    { run: `orx exp view check ${run} > ${checks} && orx exp view show ${run} > ${description}` },
    { say: 'Gallery checked.' },
  ])
  await send(page, 'Inspect the sample gallery.')
  await expect(page.getByText('Gallery checked.', { exact: true })).toBeVisible()
  for (const output of [checks, description]) {
    expect(await readFile(output, 'utf8')).toContain('samples/Frame 4.png, samples/Frame 3.png, samples/Frame 2.png')
  }

  await page.getByRole('button', { name: 'Experiments', exact: true }).click()
  const experiments = page.getByRole('list', { name: 'Experiments' })
  await experiments.getByRole('button', { name: 'Simulation', exact: true }).click()
  const latest = page.locator('section', { has: page.getByRole('heading', { name: 'Latest run', exact: true }) })
  const runGallery = latest.getByRole('region', { name: 'Simulation samples' })
  const frame = runGallery.getByRole('img', { name: 'Frame 4.png' })
  await expect(frame).toBeVisible()
  await frame.scrollIntoViewIfNeeded()
  await expect(frame).toHaveJSProperty('naturalWidth', 168)
  await capture(page, runGallery, 'run')

  const rewritten = path.join(samples, 'Frame 4.png')
  await writeFile(rewritten, Buffer.from(imgs[0], 'base64'))
  await utimes(rewritten, 1_784_000_005, 1_784_000_005)
  expect((await page.request.get(old)).status()).toBe(404)
  await expect(frame).not.toHaveAttribute('src', old)
  await expect(frame).toHaveJSProperty('naturalWidth', 168)
  await rm(rewritten)
  await expect(frame).toHaveCount(0)
  await expect(runGallery.getByRole('img', { name: 'Frame 1.png' })).toBeVisible()
})
