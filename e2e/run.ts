/**
 * Builds this checkout's `orx` and dashboard, then runs the Playwright suite against them.
 * Arguments pass through to Playwright: `node e2e/run.ts chat.spec.ts --headed`. @module
 */
import { spawn, spawnSync } from 'node:child_process'
import { copyFileSync, mkdirSync, readdirSync, statSync } from 'node:fs'
import path from 'node:path'

const E2E = import.meta.dirname
const ROOT = path.dirname(E2E)
const UI = path.join(ROOT, 'ui')
const run = (command: string, args: string[], cwd = ROOT) => new Promise<void>((resolve, reject) => {
  const child = spawn(command, args, { cwd, stdio: 'inherit' })
  child.once('error', reject)
  child.once('close', (code, signal) => {
    if (code === 0) resolve()
    else reject(new Error(`${command} ${args.join(' ')} exited with ${signal ?? code}`))
  })
})

/** The newest modification time under `entry`, skipping dependencies and build output. */
function newest(entry: string): number {
  const stats = statSync(entry, { throwIfNoEntry: false })
  if (!stats?.isDirectory()) return stats?.mtimeMs ?? 0
  const children = readdirSync(entry).filter((name) => !['node_modules', 'dist', 'paraglide'].includes(name))
  return Math.max(stats.mtimeMs, ...children.map((name) => newest(path.join(entry, name))))
}

const files = (dir: string): string[] =>
  readdirSync(dir, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? files(path.join(dir, entry.name)) : entry.name.endsWith('.ts') ? [path.join(dir, entry.name)] : [],
  )
if (!statSync(path.join(E2E, 'node_modules'), { throwIfNoEntry: false })) await run('pnpm', ['install', '--frozen-lockfile'], E2E)
// After the install: the linter needs TypeScript.
const { lint } = await import('./lint.ts')
const problems = ['src', 'tests'].flatMap((dir) => files(path.join(E2E, dir))).flatMap((file) => lint(path.relative(ROOT, file)))
if (problems.length) {
  for (const problem of problems) console.error(problem)
  process.exit(1)
}

// Debug builds read ui/dist from disk, so both builds can run together.
const rebuildUi = newest(UI) > newest(path.join(UI, 'dist'))
if (rebuildUi && !statSync(path.join(UI, 'node_modules'), { throwIfNoEntry: false })) {
  await run('pnpm', ['install', '--frozen-lockfile'], UI)
}

const builds = [run('cargo', ['build', '--locked'])]
if (rebuildUi) builds.push(run('pnpm', ['exec', 'vite', 'build'], UI))

const results = await Promise.allSettled(builds)
for (const result of results) {
  if (result.status === 'rejected') throw result.reason
}

// A copy, so a rebuild during the run cannot swap the binary the servers and agents execute.
const target = process.env.CARGO_TARGET_DIR ?? path.join(ROOT, 'target')
const binary = path.join(E2E, '.out', 'bin', 'orx')
mkdirSync(path.dirname(binary), { recursive: true })
copyFileSync(path.join(target, 'debug', 'orx'), binary)
// Debug builds start local runs with the orx-host beside their own executable.
copyFileSync(path.join(target, 'debug', 'orx-host'), path.join(path.dirname(binary), 'orx-host'))

const playwright = spawnSync(path.join(E2E, 'node_modules', '.bin', 'playwright'), ['test', ...process.argv.slice(2)], {
  cwd: E2E,
  stdio: 'inherit',
  env: { ...process.env, ORX_E2E_BINARY: binary },
})
process.exitCode = playwright.status ?? 1
