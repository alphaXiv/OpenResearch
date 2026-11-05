import { FAKE_AGENT } from '../src/orx.ts'
import { composer, expect, test } from '../src/test.ts'

test('a first run connects an agent, creates a project and streams its first reply', async ({ page, agent, launchOrx }) => {
  const orx = await launchOrx()
  await page.goto(orx.url)

  await test.step('onboarding connects the agent through ACP', async () => {
    await page.getByRole('button', { name: 'Continue' }).click()
    await page.getByRole('button', { name: 'Connect a coding agent' }).click()
    const dialog = page.getByRole('dialog', { name: 'Connect a coding agent' })
    await dialog.getByRole('button', { name: 'Coding agent' }).click()
    await dialog.getByRole('button', { name: 'Other (custom setup)', exact: true }).click()
    await dialog.getByRole('textbox', { name: 'Name' }).fill('E2E Agent')
    await dialog.getByRole('textbox', { name: 'Launch command' }).fill(`${process.execPath} ${FAKE_AGENT}`)
    await dialog.getByRole('button', { name: 'Test connection' }).click()
    await expect(dialog.getByRole('status')).toHaveText('Connected · 1.0.0')
    await dialog.getByRole('button', { name: 'Save' }).click()
    await expect(page.getByRole('button', { name: /^E2E Agent via ACP/ })).toHaveAttribute('aria-pressed', 'true')
    await page.getByRole('button', { name: 'Continue', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Bring your alphaXiv library' })).toBeVisible()
    await page.getByRole('button', { name: 'Skip' }).click()
  })

  await test.step('a new project opens on a new chat with that agent', async () => {
    await page.getByRole('dialog', { name: 'Welcome to OpenResearch' }).getByRole('button', { name: 'Create a new project' }).click()
    await page.getByRole('textbox', { name: 'Project name' }).fill('dropout-sweep')
    await expect(page.getByRole('textbox', { name: 'Project location' })).toHaveValue('~/OpenResearch/dropout-sweep')
    await page.getByRole('button', { name: 'Create project' }).click()
    await expect(page.getByRole('heading', { name: 'What should we research?' })).toBeVisible()
    await expect(composer(page)).toHaveAccessibleName(/^Message ⁦E2E Agent⁩/)
  })

  await test.step('the first message streams in', async () => {
    agent.script('Does dropout help a 10M-parameter model?', [{ say: 'Usually not at this size. ' }, { hold: 'reply' }, { say: 'I can test 0.1 against none.' }])
    await composer(page).fill('Does dropout help a 10M-parameter model?')
    await page.getByRole('button', { name: 'Send', exact: true }).click()
    await agent.reached('reply')
    await expect(page.getByText('Usually not at this size.')).toBeVisible()
    agent.release('reply')
    await expect(page.getByText('Usually not at this size. I can test 0.1 against none.')).toBeVisible()
  })
})
