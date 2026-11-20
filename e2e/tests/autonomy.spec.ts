import { expect, send, test } from '../src/test.ts'

test('the autonomy picker saves Full auto-research and applies it to the chat', async ({ page, project, agent }, info) => {
  await project.open(page)
  const trigger = page.getByRole('button', { name: /Autonomy: Default$/ })
  const menu = page.getByRole('menu', { name: 'Permissions' })
  const defaultLevel = menu.getByRole('menuitemradio', { name: /^Default\b/ })
  const fullAutoResearch = menu.getByRole('menuitemradio', { name: /^Full auto-research\b/ })
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
    await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
    await trigger.press('Enter')
    await expect(defaultLevel).toHaveAttribute('aria-checked', 'true')
    await expect(fullAutoResearch).toHaveAttribute('aria-checked', 'false')
    await menu.screenshot({ path: info.outputPath(`autonomy-${colorScheme}.png`) })
    await page.keyboard.press('Escape')
    await expect(trigger).toBeFocused()
  }

  await trigger.press('ArrowDown')
  await expect(menu.getByRole('menuitemradio').first()).toBeFocused()
  await page.keyboard.press('End')
  await expect(fullAutoResearch).toBeFocused()
  await page.keyboard.press('Enter')
  const fullAutoTrigger = page.getByRole('button', { name: /Autonomy: Full auto-research$/ })
  await expect(fullAutoTrigger).toBeVisible()
  agent.script('<orx-autonomy level="autoresearch">', [{ say: 'Ready to follow the approved research direction.' }])
  agent.script('[orx] If an approved next step remains', [{ say: 'The approved direction has no step left.' }])
  await send(page, 'Investigate the experiment results.')
  await expect(page.getByText('Ready to follow the approved research direction.', { exact: true })).toBeVisible()
  const sessionId = new URL(page.url()).pathname.split('/').at(-1)!
  const playbook = await page.request.get(`/api/projects/${project.id}/file`, {
    params: { sessionId, path: '.openresearch/agent/autoresearch-local.md' },
  })
  expect(playbook.ok()).toBe(true)
  const { content } = await playbook.json()
  expect(content).toMatch(/Leave the closing line to the chat status beside the\s+reply box\./)
  // A Full auto-research turn that ends with nothing running and no yield gets exactly one stop check.
  await expect(page.getByText('The approved direction has no step left.', { exact: true })).toHaveCount(1)

  await page.reload()
  await page.setViewportSize({ width: 720, height: 480 })
  await fullAutoTrigger.click()
  await expect(fullAutoResearch).toHaveAttribute('aria-checked', 'true')
  await fullAutoResearch.scrollIntoViewIfNeeded()
  await expect(fullAutoResearch).toBeInViewport({ ratio: 1 })
})
