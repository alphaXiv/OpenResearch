import { execFileSync } from 'node:child_process'
import { appendFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const names = ['frontend', 'rust', 'windows', 'linux', 'macos', 'dev', 'chart', 'release_notes']
const all = () => Object.fromEntries(names.map(name => [name, true]))

export function classifyChanges(paths) {
  const checks = Object.fromEntries(names.map(name => [name, false]))
  const rust = () => Object.assign(checks, { rust: true, windows: true, linux: true, macos: true })
  for (const path of paths) {
    if (path.startsWith('.github/workflows/') || path.startsWith('.github/actions/') || path === '.github/build-setup.yml' || path.startsWith('.cargo/') || /^scripts\/ci-/.test(path)) return all()
    if (/^ui\/(README\.md|project\.inlang\/README\.md)$/.test(path)) continue
    if (path.startsWith('ui/')) checks.frontend = true
    else if (/^(src\/|crates\/|tests\/|agent-skills\/|demo\/|Cargo\.(toml|lock)$|build\.rs$|SKILL\.md$|SYSTEM_PROMPT\.md$)/.test(path)) {
      rust()
      if (path.startsWith('agent-skills/orx-figures/assets/')) checks.chart = true
      if (path.startsWith('tests/contracts/')) checks.frontend = true
    }
    else if (path.startsWith('.github/readme-assets/') || path.startsWith('.github/ISSUE_TEMPLATE/') || path.startsWith('docs/') || /\.(md|txt)$/.test(path) || /^(LICENSE|NOTICE|\.gitignore)$/.test(path)) continue
    else if (path.startsWith('windows/')) checks.windows = true
    else if (path.startsWith('linux/') || /scripts\/(build|smoke-test)-linux/.test(path)) checks.linux = true
    else if (path.startsWith('macos/') || /scripts\/(build|package)-macos/.test(path)) Object.assign(checks, { rust: true, macos: true })
    else if (/^(dev|scripts\/dev\.test)\.ts$/.test(path)) checks.dev = true
    else if (/^scripts\/(test_orx_chart\.py|orx_chart\.py|orx-chart\.test\.mjs)$/.test(path)) checks.chart = true
    else if (/^scripts\/(test_release_notes|release_notes)\.py$/.test(path)) checks.release_notes = true
    else return all()
  }
  return checks
}

export function changedChecks({ base, head = 'HEAD', forceAll = false }, git = args => execFileSync('git', args, { encoding: 'utf8' })) {
  if (forceAll) return all()
  try {
    if (!base || /^0+$/.test(base)) return all()
    try {
      git(['rev-parse', '--verify', '--quiet', `${base}^{commit}`])
    } catch {
      git(['fetch', '--no-tags', '--depth=1', 'origin', base])
    }
    git(['rev-parse', '--verify', `${head}^{commit}`])
    const paths = git(['diff', '--name-only', '--no-renames', '-z', base, head, '--']).split('\0').filter(Boolean)
    return classifyChanges(paths)
  } catch {
    return all()
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const checks = changedChecks({ base: process.env.BASE_SHA, head: process.env.HEAD_SHA || 'HEAD', forceAll: process.env.FORCE_ALL === 'true' })
  const output = Object.entries(checks).map(([name, enabled]) => `${name}=${enabled}\n`).join('')
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, output)
  else process.stdout.write(output)
}
