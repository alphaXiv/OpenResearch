import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { expect as baseExpect, send, test } from '../src/test.ts'

const expect = baseExpect.configure({ timeout: 30_000 })

const cases = [
  {
    kind: 'text',
    block: '<text>Loss is **{val_loss}**. Lower is better.</text>',
    visible: /Loss is .*2\.32.*Lower is better/,
    shown: ['Loss is 2.32. Lower is better.'],
  },
  {
    kind: 'cells',
    block: '<cells from="items" />',
    visible: /96.*correct.*48.*wrong.*24.*errors.*24.*not known/s,
    shown: ['192 retained rows; 96 correct, 48 wrong, 24 errors, 24 unknown'],
  },
  {
    kind: 'bars',
    block: '<bars from="items" by="category" />',
    visible: /algebra.*75%.*72.*of.*96.*geometry.*—.*0.*of.*0.*proofs.*33%.*24.*of.*72/s,
    shown: ['Bars from items by category: retained rows', 'algebra: 75% (72 of 96)', 'geometry: — (0 of 0)', 'proofs: 33% (24 of 72)'],
  },
]

for (const { kind, block, visible, shown } of cases) {
  test(`an agent authors, checks and cites a ${kind} block`, async ({ page, project, agent, orx }, info) => {
    test.slow()
    const dir = path.join(orx.root, `outcomes-${project.id}`)
    mkdirSync(dir)
    writeFileSync(path.join(dir, 'view.xml'), `<signal name="val_loss" digits="2" />\n<view>${block}</view>`)
    writeFileSync(path.join(dir, 'patterns.json'), JSON.stringify([{
      row: 'items', regex: '^sample (?P<category>\\w+) (?P<outcome>correct|wrong|error|unknown)$',
    }]))
    const script = path.join(dir, 'run.mjs')
    writeFileSync(script, `const row = (category, outcome) => console.log('::orx:: ' + JSON.stringify({ row: 'items', m: { category, outcome } }))
console.log('::orx:: {"phase":"eval","step":0,"of":192,"m":{"val_loss":2.32}}')
for (let i = 0; i < 24; i++) {
  for (const outcome of ['correct', 'correct', 'correct', 'wrong']) {
    row('algebra', outcome)
  }
  for (const sample of ['geometry unknown', 'proofs correct', 'proofs wrong']) {
    console.log('sample ' + sample)
  }
  row('proofs', 'error')
  console.log('::orx:: ' + JSON.stringify({ step: (i + 1) * 8 }))
}
`)

    agent.script('Launch the benchmark.', [{ run: [
      'set -e',
      `id=$(orx create-experiment ${project.id} --title 'Benchmark results' --run-command '${process.execPath} ${script}' | sed -n 's/^ *id: *//p')`,
      `orx exp view set "$id" --file ${dir}/view.xml --pattern ${dir}/patterns.json`,
      'orx exp run "$id" --backend local',
      'orx exp wake "$id"',
    ].join('\n') }, { say: 'Benchmark launched.' }])
    const hold = `outcome-result-${project.id}`
    agent.script('finished with status **done**', [{ hold }, { say: 'Benchmark finished.' }])
    await project.open(page)
    await send(page, 'Launch the benchmark.')
    await agent.reached(hold)
    const runs = await (await page.request.get(`${orx.url}/api/chat/shown-runs`)).json()
    const run = runs.find((entry: { experimentName: string }) => entry.experimentName === 'Benchmark results').run.id
    agent.release(hold)
    await expect(page.getByText('Benchmark finished.', { exact: true })).toBeVisible()

    agent.script('Cite the benchmark.', [{ run: [
      'set -e',
      `orx exp view check ${run} --json > ${dir}/check.json`,
      `orx exp view show ${run} > ${dir}/show.txt`,
    ].join('\n') }, { say: `Evaluation results. <run id="${run}" label="Benchmark result" figure/>` }])
    await send(page, 'Cite the benchmark.')
    const citation = page.locator('.md').filter({ hasText: 'Evaluation results.' })
    await expect(citation).toContainText(visible)
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeHidden()
    if (kind === 'cells') await expect(citation.getByRole('img', { name: visible })).toBeVisible()
    if (kind === 'text') await expect(citation.locator('strong')).toContainText('2.32')

    const check = JSON.parse(readFileSync(path.join(dir, 'check.json'), 'utf8'))
    expect(check.pending).toEqual([])
    expect(check.seen.tables.items).toEqual(expect.arrayContaining(['category', 'outcome']))
    expect(check.patterns[0].matched).toBe(72)
    const output = readFileSync(path.join(dir, 'show.txt'), 'utf8')
    for (const line of shown) expect(output).toContain(line)

    const runPage = page.locator('section', { has: page.getByRole('heading', { name: 'Latest run', exact: true }) })
    for (const surface of ['citation', 'page'] as const) {
      const target = surface === 'citation' ? citation : runPage
      if (surface === 'page') {
        await page.setViewportSize({ width: 1440, height: 950 })
        await page.getByRole('button', { name: 'Experiments', exact: true }).click()
        await page.getByRole('list', { name: 'Experiments' }).getByRole('button', { name: 'Benchmark results', exact: true }).click()
      }
      for (const width of [1440, 420]) {
        await page.setViewportSize({ width, height: 950 })
        for (const colorScheme of ['light', 'dark'] as const) {
          await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' })
          await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme)
          await expect(target).toContainText(visible)
          await expect(target).toBeInViewport({ ratio: 1 })
          await page.screenshot({ animations: 'disabled', path: info.outputPath(`${kind}-${surface}-${colorScheme}-${width}.png`) })
        }
      }
    }
  })
}
