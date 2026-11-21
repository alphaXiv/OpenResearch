import type { Page } from '@playwright/test'
import { composer, expect, send, test } from '../src/test.ts'
import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'

const commit = 'git -c user.name=E2E -c user.email=e2e@example.com commit -qm'

async function projectsOf(page: Page, orx: { url: string }) {
  const { projects } = await (await page.request.get(`${orx.url}/api/projects`)).json()
  return projects as { id: string; name: string }[]
}

/** The name of a test's project, which the picker's trigger shows beside its icon. */
async function nameOf(page: Page, orx: { url: string }, project: { id: string }) {
  return (await projectsOf(page, orx)).find((candidate) => candidate.id === project.id)!.name
}

test('a new project becomes the project its agent proposes', async ({ page, project, agent, orx }, info) => {
  const folder = path.join(orx.root, 'kept', 'tokenizer-probes')
  agent.script('Jot down some ideas.', [
    { run: `printf 'ideas\\n' > notes.md && printf 'draft\\n' > draft.md && git add notes.md && ${commit} 'Add notes'` },
    { say: 'Wrote notes.md.' },
  ])
  agent.script('Keep this as a project.', [
    { run: `orx project create --name 'Tokenizer probes' --path '${folder}'` },
    { say: 'Proposed the project.' },
  ])
  await page.addInitScript(() => localStorage.setItem('sidebar-grouping', 'projects'))
  await project.open(page)

  await test.step('the composer starts a new project that asks for a description', async () => {
    await page.getByRole('button', { name: /^e2e-/, expanded: false }).click()
    await page.getByRole('dialog', { name: 'Projects' }).getByRole('button', { name: 'New project' }).click()
    await expect(page.getByRole('textbox', { name: 'Project name (optional)' })).toBeVisible()
    // The new draft focuses its composer once it is ready for a message.
    await expect(composer(page)).toBeFocused()
    await expect(composer(page)).toHaveAccessibleName(/^Describe your project goal/)
    await send(page, 'Jot down some ideas.')
    await expect(page.getByText('Wrote notes.md.', { exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Choose folder' })).toBeVisible()
    // A project nobody named is just its chat in the sidebar, with no project row to add a second chat to.
    // The chat still gets an icon, the empty chat bubble.
    const chat = page.getByRole('complementary').getByRole('button', { name: /^Jot down some ideas\./ })
    await expect(chat).toBeVisible()
    await expect(chat.getByRole('img', { name: 'New project' })).toBeVisible()
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
    await expect(page.getByRole('complementary').getByText('Tokenizer probes', { exact: true })).toBeVisible()
    // The folder gets what the chat committed; its unsaved draft stays with the chat.
    expect(readFileSync(path.join(folder, 'notes.md'), 'utf8')).toBe('ideas\n')
    expect(existsSync(path.join(folder, 'draft.md'))).toBe(false)
  })
})

test('naming a new project makes it a project of its own, which outlives its last chat', async ({ page, project, agent, orx }) => {
  agent.script('Compare tokenizers on a small corpus.', [{ say: 'Comparing tokenizers.' }])
  await project.open(page)
  await page.getByRole('button', { name: /^e2e-/, expanded: false }).click()
  await page.getByRole('dialog', { name: 'Projects' }).getByRole('button', { name: 'New project' }).click()
  await expect(composer(page)).toBeFocused()

  const name = page.getByRole('textbox', { name: 'Project name (optional)' })
  await name.fill('Tokenizer probes')
  await name.press('Enter')
  // The name is a field of its own; the switcher keeps saying this is a new project.
  await expect(page.getByRole('button', { name: 'New project', expanded: false })).toBeVisible()
  await send(page, 'Compare tokenizers on a small corpus.')
  await expect(page.getByText('Comparing tokenizers.', { exact: true })).toBeVisible()
  const chat = page.getByRole('complementary').getByRole('button', { name: /^Compare tokenizers on a small corpus\./ })
  // A chat of an unnamed project reads as just its title; a named one says which project it is in.
  await expect(chat).toContainText('Tokenizer probes')

  await chat.click({ button: 'right' })
  await page.getByRole('menuitem', { name: 'Delete' }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Delete' }).click()
  await expect(chat).toBeHidden()
  expect((await projectsOf(page, orx)).map((candidate) => candidate.name)).toContain('Tokenizer probes')
  // With its chat gone it is an ordinary empty project, not a new one again.
  await expect(composer(page)).toHaveAccessibleName(/^Describe what to try next/)
  await expect(page.getByRole('textbox', { name: 'Project name (optional)' })).toBeHidden()
})

test('a new project offers a paper, a folder or an .orx file to start from', async ({ page, project }) => {
  await project.open(page)
  await page.getByRole('button', { name: /^e2e-/, expanded: false }).click()
  await page.getByRole('dialog', { name: 'Projects' }).getByRole('button', { name: 'New project' }).click()

  await page.getByRole('button', { name: 'Import', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Import' })
  await expect(dialog.getByRole('button', { name: 'Existing folder' })).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'From a paper' })).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Open .orx file' })).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Blank project' })).toBeHidden()
})

test('a project picked from the picker is an ordinary project, even one nobody has chatted in', async ({ page, project, orx }) => {
  await project.open(page)
  const own = await nameOf(page, orx, project)
  const picker = page.getByRole('dialog', { name: 'Projects' })
  await page.getByRole('button', { name: own, exact: true, expanded: false }).click()
  await picker.getByRole('button', { name: 'New project' }).click()
  // A project that opens takes the composer's focus, which would close a picker opened before it lands.
  await expect(composer(page)).toBeFocused()
  const name = page.getByRole('textbox', { name: 'Project name (optional)' })
  await name.fill('Picker rename check')
  await name.press('Enter')
  // The project being named stays a new project in its row.
  await expect(name).toBeVisible()

  await page.getByRole('button', { name: 'New project', expanded: false }).click()
  await picker.getByRole('button', { name: own, exact: true }).click()
  await expect(composer(page)).toBeFocused()
  await page.getByRole('button', { name: own, exact: true, expanded: false }).click()
  await picker.getByRole('button', { name: /^Picker rename check/ }).click()
  await expect(page.getByRole('button', { name: /^Picker rename check/, expanded: false })).toBeVisible()
  await expect(name).toBeHidden()
  await expect(page.getByRole('button', { name: 'Import', exact: true })).toBeHidden()
})

test('the project picker searches by name and opens a project\'s settings', async ({ page, project, orx }) => {
  await project.open(page)
  const own = await nameOf(page, orx, project)
  await page.getByRole('button', { name: own, exact: true, expanded: false }).click()
  const picker = page.getByRole('dialog', { name: 'Projects' })
  const search = picker.getByRole('textbox', { name: 'Search projects…' })
  await search.fill('no such project')
  await expect(picker.getByText('No projects match your search.')).toBeVisible()
  await search.fill(own)
  await expect(picker.getByRole('button', { name: own, exact: true })).toBeVisible()
  // Enter opens the first match, even the project already open, and lands in the message box.
  await search.press('Enter')
  await expect(composer(page)).toBeFocused()
  await page.getByRole('button', { name: own, exact: true, expanded: false }).click()
  await picker.getByRole('button', { name: `Project settings · ${own}` }).click()
  await expect(page).toHaveURL(new RegExp(`/projects/${project.id}/settings/project`))
})

test('project folders show a custom symbol, and Default clears it', async ({ page, project, orx }) => {
  await page.addInitScript(() => localStorage.setItem('sidebar-grouping', 'projects'))
  await project.open(page)
  const name = await nameOf(page, orx, project)
  const row = page.getByRole('complementary').getByRole('button', { name, exact: true })
  await page.getByRole('button', { name: `Project details · ${name}` }).click()
  await page.getByRole('menuitem', { name: 'Project settings' }).click()
  const changeIcon = page.getByRole('button', { name: 'Change icon' })
  await changeIcon.click()

  const icons = page.getByRole('radiogroup', { name: 'Icons' })
  const first = icons.getByRole('radio').first()
  await expect(first).toHaveAccessibleName('Default')
  await expect(first).toBeChecked()
  await icons.getByRole('radio', { name: 'Rocket' }).click()
  await expect(row.locator('svg.lucide-folder-open')).toBeVisible()
  await expect(row.locator('svg.lucide-rocket')).toBeVisible()
  await page.keyboard.press('Escape')
  await row.click()
  await expect(row.locator('svg.lucide-folder')).toBeVisible()
  await expect(row.locator('svg.lucide-rocket')).toBeVisible()

  await changeIcon.click()
  await icons.getByRole('radio', { name: 'Initials' }).click()
  await expect(icons.getByRole('radio', { name: 'Initials' })).toBeChecked()
  await expect(row).toHaveText(name)
  await expect(row.locator('svg.lucide-rocket')).toHaveCount(0)

  const saved = page.waitForResponse((response) => response.url().endsWith(`/api/projects/${project.id}`) && response.request().postDataJSON()?.icon === null)
  await first.click()
  expect((await saved).ok()).toBe(true)
  await page.reload()
  await expect(changeIcon.locator('svg.lucide-folder')).toBeVisible()
  await changeIcon.click()
  await expect(first).toBeChecked()
})

test('switching projects keeps the new chat in place while the project loads', async ({ page, project, orx }) => {
  const other = await orx.createProject('other-project')
  await project.open(page)
  await expect(page.getByRole('heading', { name: 'What should we research?' })).toBeVisible()
  let release!: () => void
  let asked!: () => void
  const held = new Promise<void>((resolve) => { release = resolve })
  const requested = new Promise<void>((resolve) => { asked = resolve })
  await page.route(`**/api/projects/${other.id}/ui-state`, async (route) => {
    if (route.request().method() !== 'GET') return route.fallback()
    asked()
    await held
    await route.continue()
  })

  await page.getByRole('button', { name: /^e2e-/, expanded: false }).click()
  await page.getByRole('dialog', { name: 'Projects' }).getByRole('button', { name: /^other-project/ }).click()
  await expect(page.getByRole('button', { name: /^other-project/, expanded: false })).toBeVisible()
  await requested
  await expect(page.getByRole('heading', { name: 'What should we research?' })).toBeVisible()
  await expect(page.getByRole('status', { name: 'Loading conversation…' })).toBeHidden()
  release()
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
  await expect(composer(page)).toHaveAccessibleName(/^Ask a follow-up/)

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
  await expect(page.getByRole('complementary').getByText('Split off', { exact: true })).toBeVisible()
  expect(readFileSync(path.join(folder, 'probe.txt'), 'utf8')).toBe('probe\n')
})
