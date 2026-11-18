import { composer, expect, send, test } from '../src/test.ts'
import { readFileSync } from 'node:fs'
import path from 'node:path'

test('a chat without a project becomes the project its agent proposes', async ({ page, project, agent, orx }, info) => {
  const folder = path.join(orx.root, 'kept', 'tokenizer-probes')
  agent.script('Jot down some ideas.', [
    { run: "printf 'ideas\\n' > notes.md" },
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
    expect(readFileSync(path.join(folder, 'notes.md'), 'utf8')).toBe('ideas\n')
  })
})
