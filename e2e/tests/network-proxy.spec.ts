import { expect, test } from '../src/test.ts'

test('a saved network proxy survives a restart and can return to the environment default', async ({ page, launchOrx, outside }) => {
  const orx = await launchOrx({ appWindow: true })
  await orx.onboard()
  const project = await orx.createProject('proxy-settings')
  await project.open(page)
  await page.getByRole('button', { name: 'Settings', exact: true }).click()

  const section = page.getByRole('heading', { name: 'Network proxy', exact: true }).locator('..')
  const address = section.getByLabel('Proxy address')
  const restart = section.getByRole('status')
  const proxyUrl = new URL(outside.url).href
  await address.fill(proxyUrl)
  await section.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(restart).toContainText('Restart OpenResearch')

  await orx.restart()
  await page.reload()
  await expect(address).toHaveValue(proxyUrl)
  await expect(restart).not.toBeVisible()

  await address.fill('')
  await section.getByRole('button', { name: 'Save', exact: true }).click()
  await expect(restart).toContainText('Restart OpenResearch')
  await orx.restart()
  await page.reload()
  await expect(address).toHaveValue('')
  await expect(restart).not.toBeVisible()
})
