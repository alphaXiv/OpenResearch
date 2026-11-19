import type { Page } from '@playwright/test'
import { composer, expect, test } from '../src/test.ts'

async function gripBox(page: Page) {
  const box = await page.getByRole('separator', { name: 'Resize message box' }).boundingBox()
  if (!box) throw new Error('The composer resize grip is not on screen')
  return box
}

async function dragGrip(page: Page, by: number) {
  const box = await gripBox(page)
  const x = box.x + box.width / 2
  const y = box.y + box.height / 2
  await page.mouse.move(x, y)
  await page.mouse.down()
  await page.mouse.move(x, y + by, { steps: 5 })
  await page.mouse.up()
}

test('a composer dragged short still grows with the draft', async ({ page, project }) => {
  await project.open(page)
  const firstLine = page.getByText('First line', { exact: true })

  await test.step('dragged to its smallest, it grows as lines are added', async () => {
    await dragGrip(page, 200)
    await composer(page).click()
    await composer(page).pressSequentially('First line')
    for (const line of ['Second line', 'Third line']) {
      await composer(page).press('Shift+Enter')
      await composer(page).pressSequentially(line)
    }
    await expect(firstLine).toBeInViewport({ ratio: 1 })
  })

  await test.step('dragged below a tall draft, it stays short until a new line', async () => {
    const tall = (await gripBox(page)).y
    await dragGrip(page, 200)
    const short = (await gripBox(page)).y
    expect(short).toBeGreaterThan(tall)
    await composer(page).pressSequentially(' and more')
    await expect(composer(page)).toContainText('Third line and more')
    expect((await gripBox(page)).y).toBe(short)
    await composer(page).press('Shift+Enter')
    await expect.poll(async () => (await gripBox(page)).y).toBeLessThanOrEqual(tall)
  })
})
