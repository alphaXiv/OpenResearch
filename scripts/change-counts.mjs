import { execFileSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

/** Buckets a commit title by its conventional prefix. */
export function classify(subject) {
  const type = /^(\w+)(?:\([^)]*\))?!?:/.exec(subject)?.[1]
  return type === 'feat' ? 'features' : type === 'fix' ? 'fixes' : 'other'
}

/** Cumulative totals for every non-merge commit reachable from `commit`. */
export function changesAt(commit) {
  const subjects = execFileSync('git', ['log', '--no-merges', '--format=%s', commit], { encoding: 'utf8', maxBuffer: 1 << 28 })
    .split('\n')
    .filter(Boolean)
  const changes = { features: 0, fixes: 0, other: 0 }
  for (const subject of subjects) changes[classify(subject)]++
  return changes
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  console.log(JSON.stringify(changesAt(process.argv[2])))
}
