/**
 * Builds this checkout's `orx` and dashboard, then runs the Playwright suite against them.
 * Arguments pass through to Playwright: `node e2e/run.ts chat.spec.ts --headed`. @module
 */
import { execFileSync, spawnSync } from 'node:child_process'
import { copyFileSync, mkdirSync, readdirSync, statSync } from 'node:fs'
import path from 'node:path'

const E2E = import.meta.dirname
const ROOT = path.dirname(E2E)
const UI = path.join(ROOT, 'ui')
const run = (command: string, args: string[], cwd = ROOT) => execFileSync(command, args, { cwd, stdio: 'inherit' })

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
if (!statSync(path.join(E2E, 'node_modules'), { throwIfNoEntry: false })) run('pnpm', ['install', '--frozen-lockfile'], E2E)
// After the install: the linter needs TypeScript.
const { lint } = await import('./lint.ts')
const problems = ['src', 'tests'].flatMap((dir) => files(path.join(E2E, dir))).flatMap((file) => lint(path.relative(ROOT, file)))
if (problems.length) {
  for (const problem of problems) console.error(problem)
  process.exit(1)
}

// The debug binary serves ui/dist from disk, so the dashboard only needs rebuilding when its sources change.
if (newest(UI) > newest(path.join(UI, 'dist'))) {
  if (!statSync(path.join(UI, 'node_modules'), { throwIfNoEntry: false })) run('pnpm', ['install', '--frozen-lockfile'], UI)
  run('pnpm', ['exec', 'vite', 'build'], UI)
}
run('cargo', ['build', '--locked'])

// A copy, so a rebuild during the run cannot swap the binary the servers and agents execute.
const target = process.env.CARGO_TARGET_DIR ?? path.join(ROOT, 'target')
const binary = path.join(E2E, '.out', 'bin', 'orx')
mkdirSync(path.dirname(binary), { recursive: true })
copyFileSync(path.join(target, 'debug', 'orx'), binary)

const playwright = spawnSync(path.join(E2E, 'node_modules', '.bin', 'playwright'), ['test', ...process.argv.slice(2)], {
  cwd: E2E,
  stdio: 'inherit',
  env: { ...process.env, ORX_E2E_BINARY: binary },
})
process.exitCode = playwright.status ?? 1
