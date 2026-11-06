import { lstatSync, mkdirSync, symlinkSync } from 'node:fs'
import { join } from 'node:path'

// Claude Code reads repo skills only from `.claude/skills`; the other harnesses read `.agents/skills`, their one home.
const root = join(import.meta.dirname, '../..')
const link = join(root, '.claude/skills')

try {
  lstatSync(link)
} catch {
  mkdirSync(join(root, '.claude'), { recursive: true })
  const windows = process.platform === 'win32'
  symlinkSync(windows ? join(root, '.agents/skills') : '../.agents/skills', link, windows ? 'junction' : 'dir')
}
