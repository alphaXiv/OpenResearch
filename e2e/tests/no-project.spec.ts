import { composer, expect, send, test } from '../src/test.ts'
import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'

const commit = 'git -c user.name=E2E -c user.email=e2e@example.com commit -qm'

test('a chat without a project becomes the project its agent proposes', async ({ page, project, agent, orx }, info) => {
  const folder = path.join(orx.root, 'kept', 'tokenizer-probes')
  agent.script('Jot down some ideas.', [
    { run: `printf 'ideas\\n' > notes.md && printf 'draft\\n' > draft.md && git add notes.md && ${commit} 'Add notes'` },
    { say: 'Wrote notes.md.' },
  ])
  agent.script('Keep this as a project.', [
    { run: `orx project create --name 'Tokenizer probes' --path '${folder}'` },
    { say: 'Proposed the project.' },
  ])
  await project.open(page)

  await test.step('the composer starts a chat with no project', async () => {
    await page.getByRole('button', { name: /^e2e-/, expanded: false }).click()
    await page.getByRole('menuitem', { name: 'No project' }).click()
    await expect(page.getByRole('button', { name: 'No project', expanded: false })).toBeVisible()
    // The new draft focuses its composer once it is ready for a message.
    await expect(composer(page)).toBeFocused()
    await send(page, 'Jot down some ideas.')
    await expect(page.getByText('Wrote notes.md.', { exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Create project' })).toBeVisible()
    await expect(page.getByRole('complementary').getByRole('button', { name: 'No project' })).toBeVisible()
  })

  await test.step("the agent's proposal moves the chat's work into the new project", async () => {
    await send(page, 'Keep this as a project.')
    await expect(page.getByText('Proposed the project.', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Review…' }).click()
    const dialog = page.getByRole('dialog')
    await expect(dialog.getByRole('textbox', { name: 'Project name' })).toHaveValue('Tokenizer probes')
    await expect(dialog.getByRole('textbox', { name: 'Project location' })).toHaveValue(folder)
    await page.screenshot({ path: info.outputPath('review.png') })
    await dialog.getByRole('button', { name: 'Create project' }).click()
    await expect(dialog).toBeHidden()
    await expect(page.getByText('Created', { exact: true })).toBeVisible()
    await expect(page.getByRole('complementary').getByRole('button', { name: 'Tokenizer probes', exact: true })).toBeVisible()
    // The folder gets what the chat committed; its unsaved draft stays with the chat.
    expect(readFileSync(path.join(folder, 'notes.md'), 'utf8')).toBe('ideas\n')
    expect(existsSync(path.join(folder, 'draft.md'))).toBe(false)
  })
})

test('a chat in a project moves into a new one once its turn ends', async ({ page, project, agent, orx }) => {
  const folder = path.join(orx.root, 'kept', 'split-off')
  agent.script('Write a probe and keep working.', [
    { run: `printf 'probe\\n' > probe.txt && git add probe.txt && ${commit} 'Add probe'` },
    { hold: 'working' },
    { say: 'Probe written.' },
  ])
  agent.script('made this chat the project', [{ say: 'Carrying on in the new project.' }])
  await project.open(page)
  await send(page, 'Write a probe and keep working.')
  await agent.reached('working')

  await page.getByRole('complementary').getByRole('button', { name: /^Write a probe and keep working\./ }).click({ button: 'right' })
  await page.getByRole('menuitem', { name: 'Move to a new project…' }).click()
  const dialog = page.getByRole('dialog')
  await dialog.getByRole('textbox', { name: 'Project name' }).fill('Split off')
  await dialog.getByRole('textbox', { name: 'Project location' }).fill(folder)
  await dialog.getByRole('button', { name: 'Create project' }).click()
  await expect(dialog).toBeHidden()
  await expect(page.getByText(/Moves to .*Split off.* when this turn ends/)).toBeVisible()

  agent.release('working')
  await expect(page.getByText('Carrying on in the new project.', { exact: true })).toBeVisible()
  await expect(page.getByRole('complementary').getByRole('button', { name: 'Split off', exact: true })).toBeVisible()
  expect(readFileSync(path.join(folder, 'probe.txt'), 'utf8')).toBe('probe\n')
})
