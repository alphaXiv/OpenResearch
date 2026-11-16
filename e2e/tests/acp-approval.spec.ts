import { expect, send, test } from '../src/test.ts'
import type { Page } from '@playwright/test'

async function selectApproval(page: Page, name: string) {
  await page.getByRole('button', { name: /^Permissions:/ }).click()
  await page.getByRole('menuitemradio', { name: new RegExp(`^${name}(?:\\s|$)`) }).click()
  await expect(page.getByRole('button', { name: new RegExp(`^Permissions: ${name}`) })).toBeVisible()
}

test('ACP approval defaults to asking, changes during a wait and survives a restart', async ({ page, project, agent, orx }, info) => {
  agent.script('Check the approval workflow.', [
    { permission: 'Inspect the test directory' },
    { permission: 'Read the test configuration' },
    { say: 'Both requested actions completed.' },
  ])
  await project.open(page)
  await send(page, 'Check the approval workflow.')
  await expect(page.getByRole('button', { name: 'Allow this action', exact: true })).toBeVisible()
  await page.getByRole('button', { name: /^Permissions:/ }).click()
  await expect(page.getByRole('menuitemradio', { name: /^Ask(?:\s|$)/ })).toHaveAttribute('aria-checked', 'true')
  await page.screenshot({ path: info.outputPath('ask.png') })
  await page.getByRole('menuitemradio', { name: /^Auto-approve(?:\s|$)/ }).click()
  // A pending request is reconsidered on the ACP host's five-second status tick.
  await expect(page.getByText('Both requested actions completed.', { exact: true })).toBeVisible({ timeout: 15_000 })
  await expect(page.getByRole('button', { name: 'Allow this action', exact: true })).toBeHidden()
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeHidden()

  await orx.restart()
  await page.reload()
  await expect(page.getByRole('button', { name: /^Permissions: Auto-approve/ })).toBeVisible()
  agent.script('Verify the saved setting.', [{ permission: 'Inspect the saved setting' }, { say: 'Saved approval setting worked.' }])
  await send(page, 'Verify the saved setting.')
  await expect(page.getByText('Saved approval setting worked.', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: /^Permissions:/ }).click()
  await expect(page.getByRole('menuitemradio', { name: /^Auto-approve(?:\s|$)/ })).toHaveAttribute('aria-checked', 'true')
  await page.screenshot({ path: info.outputPath('auto-approve.png') })
  await page.keyboard.press('Escape')

  await selectApproval(page, 'Ask')
  agent.script('Verify asking again.', [{ permission: 'Confirm the next action' }, { say: 'Manual approval worked again.' }])
  await send(page, 'Verify asking again.')
  await page.getByRole('button', { name: 'Allow this action', exact: true }).click()
  await expect(page.getByText('Manual approval worked again.', { exact: true })).toBeVisible()
})

test('ACP auto-approval keeps persistent grants and mode changes manual', async ({ page, project, agent }) => {
  agent.script('Start the permission checks.', [{ say: 'Ready to check approvals.' }])
  await project.open(page)
  await send(page, 'Start the permission checks.')
  await expect(page.getByText('Ready to check approvals.', { exact: true })).toBeVisible()
  await selectApproval(page, 'Auto-approve')
  for (const [text, step] of [
    ['Check a persistent grant.', { permission: 'Grant lasting permission', grant: 'allow_always' as const }],
    ['Check a mode change.', { permission: 'Change agent mode', kind: 'switch_mode' }],
    ['Check a refusal.', { permission: 'An action without a grant', grant: 'none' as const }],
  ] as const) {
    agent.script(text, [step, { say: `Finished: ${text}` }])
    await send(page, text)
    await expect(page.getByRole('button', { name: 'Reject this action', exact: true })).toBeVisible()
    await page.getByRole('button', { name: 'Reject this action', exact: true }).click()
    await expect(page.getByText(`Finished: ${text}`, { exact: true })).toBeVisible()
  }
})
