import { expect, send, test } from '../src/test.ts'
import type { Page } from '@playwright/test'
import { readFileSync } from 'node:fs'

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
  await page.getByRole('menuitemradio', { name: /^Full access(?:\s|$)/ }).click()
  // A pending request is reconsidered on the ACP host's five-second status tick.
  await expect(page.getByText('Both requested actions completed.', { exact: true })).toBeVisible({ timeout: 15_000 })
  await expect(page.getByRole('button', { name: 'Allow this action', exact: true })).toBeHidden()
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeHidden()

  await orx.restart()
  await page.reload()
  await expect(page.getByRole('button', { name: /^Permissions: Full access/ })).toBeVisible()
  agent.script('Verify the saved setting.', [{ permission: 'Inspect the saved setting' }, { say: 'Saved approval setting worked.' }])
  await send(page, 'Verify the saved setting.')
  await expect(page.getByText('Saved approval setting worked.', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: /^Permissions:/ }).click()
  await expect(page.getByRole('menuitemradio', { name: /^Full access(?:\s|$)/ })).toHaveAttribute('aria-checked', 'true')
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
  await selectApproval(page, 'Full access')
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


test('ACP auto-approval cancels permission requests received after the turn reply', async ({ page, project, agent, orx }) => {
  agent.script('Prepare the late-request test.', [{ say: 'Ready for a late request.' }])
  await project.open(page)
  await send(page, 'Prepare the late-request test.')
  await expect(page.getByText('Ready for a late request.', { exact: true })).toBeVisible()
  await selectApproval(page, 'Full access')
  const offset = readFileSync(orx.agentLog).length
  agent.script('Reply before asking.', [{ permissionAfterReply: 'Action after turn completion' }])
  await send(page, 'Reply before asking.')
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeHidden()
  await expect.poll(() => readFileSync(orx.agentLog).subarray(offset).toString()).toContain('"outcome":"cancelled"')
  expect(readFileSync(orx.agentLog).subarray(offset).toString()).not.toContain('"outcome":"selected"')
  await expect(page.getByRole('button', { name: 'Allow late action', exact: true })).toBeHidden()
})


test('a saved ACP chat can disable automatic approval after its launch is removed', async ({ page, agent, launchOrx }) => {
  const own = await launchOrx()
  await own.onboard()
  const project = await own.createProject('removed-acp-approval')
  const launches = await page.request.get(`${own.url}/api/harnesses/acp`)
  expect(launches.ok()).toBe(true)
  const [launch] = await launches.json()
  agent.script('Start the saved-chat check.', [{ say: 'Ready to remove the launch.' }])
  await project.open(page)
  await send(page, 'Start the saved-chat check.')
  await expect(page.getByText('Ready to remove the launch.', { exact: true })).toBeVisible()
  await selectApproval(page, 'Full access')
  const removed = await page.request.delete(`${own.url}/api/harnesses/acp/${encodeURIComponent(launch.id)}`, { headers: { origin: own.url } })
  expect(removed.ok()).toBe(true)
  await page.reload()
  await expect(page.getByRole('button', { name: /^Permissions: Full access/ })).toBeVisible()
  await selectApproval(page, 'Ask')
  await page.reload()
  await expect(page.getByRole('button', { name: /^Permissions: Ask/ })).toBeVisible()
  agent.script('Check approval after removal.', [{ permission: 'Confirm saved-chat action' }, { say: 'Saved chat asked for approval.' }])
  await send(page, 'Check approval after removal.')
  await page.getByRole('button', { name: 'Allow this action', exact: true }).click()
  await expect(page.getByText('Saved chat asked for approval.', { exact: true })).toBeVisible()
})
