/**
 * An ACP agent the real chat host drives over stdio, as it drives any agent the user adds. Each
 * prompt asks the test's script server what to do: stream text, run a command through orx's
 * terminal, hold until the test releases it, or crash. @module
 */
import { appendFileSync } from 'node:fs'
import { createInterface } from 'node:readline'
import type { Step } from './agent.ts'

type Message = { jsonrpc: '2.0'; id?: number | string; method?: string; params?: any; result?: any; error?: any }

const control = process.env.ORX_E2E_AGENT
const log = process.env.ORX_E2E_AGENT_LOG
const reverse = new Map<string, (message: Message) => void>()
let nextReverse = 0
let turn: { abort: AbortController } | undefined
// orx gives every chat session its MCP servers and a one-shot (title, starter prompts, summary) none. One-shots end with
// no text, so they keep orx's fallbacks and never take a script meant for a chat turn.
const oneShots = new Set<string>()

function send(message: Omit<Message, 'jsonrpc'>) {
  const line = JSON.stringify({ jsonrpc: '2.0', ...message })
  if (log) appendFileSync(log, `${process.pid} → ${line}\n`)
  process.stdout.write(`${line}\n`)
}

function update(sessionId: string, value: object) {
  send({ method: 'session/update', params: { sessionId, update: value } })
}

function ask(method: string, params: object): Promise<Message> {
  const id = `agent-${nextReverse++}`
  send({ id, method, params })
  return new Promise((resolve) => reverse.set(id, resolve))
}

async function script(text: string): Promise<Step[]> {
  if (!control) return [{ say: 'ORX_E2E_AGENT is not set' }]
  const response = await fetch(`${control}/turn`, { method: 'POST', body: JSON.stringify({ text }) })
  return (await response.json()) as Step[]
}

/** Runs `command` in a terminal orx owns, the way an agent with no shell of its own would. */
async function run(sessionId: string, step: Extract<Step, { run: string }>) {
  const toolCallId = `run-${nextReverse}`
  update(sessionId, { sessionUpdate: 'tool_call', toolCallId, title: step.run, kind: 'execute', status: 'in_progress' })
  const created = await ask('terminal/create', { sessionId, command: '/bin/sh', args: ['-c', step.run] })
  const terminalId = created.result.terminalId
  update(sessionId, { sessionUpdate: 'tool_call_update', toolCallId, content: [{ type: 'terminal', terminalId }] })
  const exit = await ask('terminal/wait_for_exit', { sessionId, terminalId })
  const output = await ask('terminal/output', { sessionId, terminalId })
  await ask('terminal/release', { sessionId, terminalId })
  const failed = exit.result?.exitCode !== 0
  update(sessionId, {
    sessionUpdate: 'tool_call_update',
    toolCallId,
    status: failed ? 'failed' : 'completed',
    rawOutput: { output: output.result.output, exitCode: exit.result?.exitCode },
  })
}

async function prompt(request: Message) {
  const { sessionId, prompt: blocks } = request.params
  if (oneShots.has(sessionId)) return send({ id: request.id, result: { stopReason: 'end_turn' } })
  const text = blocks.map((block: { text?: string }) => block.text ?? '').join('\n')
  const abort = new AbortController()
  turn = { abort }
  try {
    for (const step of await script(text)) {
      if ('say' in step) update(sessionId, { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: step.say } })
      else if ('think' in step) update(sessionId, { sessionUpdate: 'agent_thought_chunk', content: { type: 'text', text: step.think } })
      else if ('hold' in step) await fetch(`${control}/hold/${encodeURIComponent(step.hold)}`, { method: 'POST', signal: abort.signal })
      else if ('run' in step) await run(sessionId, step)
      else if ('exit' in step) process.exit(step.exit)
    }
    send({ id: request.id, result: { stopReason: 'end_turn' } })
  } catch (error) {
    if (!abort.signal.aborted) throw error
    send({ id: request.id, result: { stopReason: 'cancelled' } })
  } finally {
    turn = undefined
  }
}

function handle(message: Message) {
  if (message.method === undefined) {
    reverse.get(String(message.id))?.(message)
    reverse.delete(String(message.id))
    return
  }
  switch (message.method) {
    case 'initialize':
      return send({
        id: message.id,
        result: { protocolVersion: 1, agentInfo: { name: 'orx e2e agent', version: '1.0.0' }, agentCapabilities: { loadSession: true } },
      })
    case 'session/new': {
      const sessionId = `e2e-${crypto.randomUUID()}`
      if (!message.params?.mcpServers?.length) oneShots.add(sessionId)
      const models = { currentModelId: 'e2e', availableModels: [{ modelId: 'e2e', name: 'E2E Model' }] }
      return send({ id: message.id, result: { sessionId, models } })
    }
    case 'session/load':
    case 'session/set_model':
      return send({ id: message.id, result: {} })
    case 'session/prompt':
      return void prompt(message)
    case 'session/cancel':
      return turn?.abort.abort()
    default:
      if (message.id !== undefined) send({ id: message.id, error: { code: -32601, message: `${message.method} is not scripted` } })
  }
}

const input = createInterface({ input: process.stdin })
input.on('line', (line) => {
  if (log) appendFileSync(log, `${process.pid} ← ${line}\n`)
  handle(JSON.parse(line))
})
input.on('close', () => process.exit(0))
