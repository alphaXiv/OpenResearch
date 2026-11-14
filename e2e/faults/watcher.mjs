#!/usr/bin/env node
/**
 * A stand-in `claude` for the fault bed: it answers orx's watcher one-shots (`claude -p DIGEST …
 * --output-format json`) from the digest alone, by a fixed policy, so a scenario shows when orx
 * looks and what it does with the answers without a model. Each look is appended to
 * `$ORX_FAULTS_WATCH_LOG`. Any other call is the signed-out `claude` the e2e sandbox has.
 *
 * The policy: a run with something new that looks wrong (a warning, an error, a failure) is flagged
 * with a count of its kind; a run that changed otherwise gets its headline; a run with nothing new is
 * the same. @module
 */
import { appendFileSync } from 'node:fs'

const args = process.argv.slice(2)
if (args[0] === '--version') {
  console.log('2.1.0 (Claude Code)')
  process.exit(0)
}
if (args[0] === 'auth') {
  console.log('{"loggedIn":false}')
  process.exit(0)
}
const prompt = args[args.indexOf('-p') + 1]
if (args.indexOf('-p') < 0 || !prompt) process.exit(1)

const digest = JSON.parse(prompt.slice(prompt.indexOf('{')).split('\n\nYour last reply was:')[0])
const ALARM = /\b(WARN(ING)?|ERROR|Error|Traceback|FAILED|NaN|nan|timed out|retry(ing)?)\b/

/** The start of a line up to its first number, as a pattern that matches its kind. */
const kindOf = (text) => text.split(/[0-9]/)[0].replace(/[\s:,(=-]+$/, '').slice(0, 40)
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

const runs = {}
for (const run of digest.runs) {
  // The head is the same on every look; it alarms only the first.
  const lines = [...(run.previous?.length ? [] : run.log?.firstLines ?? []), ...(run.log?.sinceLastLook ?? [])]
  const alarm = lines.find((line) => ALARM.test(line.text))
  const changed = (run.lookingBecause ?? []).length > 0
  if (run.previous?.length && !alarm && !changed && run.state === 'running') {
    runs[run.key] = { same: true }
    continue
  }
  if (alarm) {
    const kind = kindOf(alarm.text)
    runs[run.key] = {
      status: 'flag',
      line: `**{count:${JSON.stringify(escape(kind))}}** lines like "${kind.slice(0, 40)}" so far.`,
      evidence: [{ from: alarm.from, to: alarm.to }],
    }
    continue
  }
  const signals = Object.entries(run.signals ?? {})
  const compared = signals.find(([, signal]) => signal.vsParentAtThisStep)
  const [name, signal] = compared ?? signals[0] ?? []
  const label = signal?.label ?? name
  runs[run.key] = !name
    ? { status: 'ok', line: `**{count:"."}** lines of output so far.` }
    : compared
      ? { status: 'ok', line: `${label} **{${name}}**, {${name}.vs}.` }
      : { status: 'ok', line: `${label} **{${name}}** at this point.` }
}
const reply = { runs }
if (digest.summary) {
  const [first, second] = digest.runs
  const shared = Object.keys(first.signals ?? {}).find((name) => second?.signals?.[name])
  reply.summary = shared
    ? { status: 'ok', line: `{${first.key}} reads **{${first.key}:${shared}}** and {${second.key}} {${second.key}:${shared}}.` }
    : { status: 'ok', line: `**${digest.runs.length}** runs watched together.` }
}
const text = JSON.stringify(reply)
if (process.env.ORX_FAULTS_WATCH_LOG) {
  const why = digest.runs.map((run) => `${run.key}: ${(run.lookingBecause ?? []).join('; ') || '-'}`).join(' | ')
  appendFileSync(process.env.ORX_FAULTS_WATCH_LOG, `${JSON.stringify({ at: Date.now(), bytes: prompt.length, retry: prompt.includes('Your last reply was'), why, reply: text })}\n`)
}
console.log(JSON.stringify({ type: 'result', is_error: false, result: text, total_cost_usd: 0.0005, usage: { input_tokens: Math.ceil(prompt.length / 4), output_tokens: 80 } }))
