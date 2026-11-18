/**
 * The real `orx up` in a sandboxed home: its own config, data and cache directories, a PATH with
 * the system's tools and signed-out `claude` and `gh`, and every request off the machine sent to
 * the outside server. @module
 */
import { spawn, type ChildProcess } from 'node:child_process'
import { copyFileSync, createWriteStream, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { createServer, type AddressInfo } from 'node:net'
import { createInterface } from 'node:readline'
import { expect, type Page } from '@playwright/test'
import type { Agent } from './agent.ts'
import type { Outside } from './outside.ts'

const ORX = process.env.ORX_E2E_BINARY!
export const FAKE_AGENT = path.join(import.meta.dirname, 'fake-agent.ts')
export const SPEEDUP = 20

export type Orx = Awaited<ReturnType<typeof startOrx>>
export type Project = { id: string; open: (page: Page) => Promise<void> }

/** Copies the `orx` under test and the `orx-host` beside it, which debug builds extract keepers from. */
function copyBuild(from: string, to: string) {
  mkdirSync(to, { recursive: true })
  for (const name of ['orx', 'orx-host']) copyFileSync(path.join(path.dirname(from), name), path.join(to, name))
  return path.join(to, 'orx')
}

async function freePort() {
  const server = createServer()
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address() as AddressInfo
  await new Promise((resolve) => server.close(resolve))
  return port
}

/**
 * `claude` replaces the sandbox's signed-out Claude Code with this script, `env` adds to the
 * server's environment, and `ownBuild` runs from a copy no other server shares, for a test that
 * replaces the executable. `appWindow` starts the server as the desktop window does, with the test
 * holding its stdin as the window's lease.
 */
export async function startOrx({ agent, outside, claude, env: extra = {}, ownBuild, appWindow }: { agent: Agent; outside: Outside; claude?: string; env?: Record<string, string>; ownBuild?: boolean; appWindow?: boolean }) {
  // Short and outside the checkout: orx keeps Unix sockets under its data directory.
  const root = realpathSync(mkdtempSync('/tmp/orx-e2e-'))
  const home = path.join(root, 'home')
  mkdirSync(path.join(home, 'projects'), { recursive: true })
  mkdirSync(path.join(root, 'tmp'))
  writeFileSync(path.join(home, '.gitconfig'), '[user]\n\tname = E2E Researcher\n\temail = e2e@example.com\n')
  // Signed-out CLIs that shadow the host's, so every host looks the same: macOS finds ChatGPT.app's
  // Codex outside PATH. Linux runners ship a gh that would reach GitHub's API.
  const bin = path.join(root, 'bin')
  mkdirSync(bin)
  for (const [name, version, status] of [['claude', '2.1.0 (Claude Code)', '{"loggedIn":false}'], ['gh', 'gh version 2.80.0']]) {
    const auth = status ? `[ "$1" = auth ] && echo '${status}'\n` : ''
    writeFileSync(path.join(bin, name), `#!/bin/sh\n[ "$1" = --version ] && echo "${version}" && exit 0\n${auth}exit 1\n`, { mode: 0o755 })
  }
  if (claude) writeFileSync(path.join(bin, 'claude'), claude, { mode: 0o755 })
  const log = path.join(root, 'orx.log')
  const env = {
    HOME: home,
    XDG_CONFIG_HOME: path.join(home, '.config'),
    XDG_DATA_HOME: path.join(home, '.local', 'share'),
    XDG_CACHE_HOME: path.join(home, '.cache'),
    TMPDIR: path.join(root, 'tmp'),
    PATH: `${bin}:/usr/bin:/bin:/usr/sbin:/sbin`,
    LANG: 'en_US.UTF-8',
    SHELL: '/bin/sh',
    OPENRESEARCH_API_URL: outside.url,
    ...Object.fromEntries(['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY'].flatMap((name) => [[name, outside.url], [name.toLowerCase(), outside.url]])),
    NO_PROXY: '127.0.0.1,localhost',
    no_proxy: '127.0.0.1,localhost',
    // Debug builds divide run supervision and turn-lease timers by this: a crashed turn's lease
    // lapses in 3 s instead of a minute.
    ORX_TEST_SPEEDUP: String(SPEEDUP),
    ORX_E2E_AGENT: agent.url,
    ORX_E2E_AGENT_LOG: path.join(root, 'agent.log'),
    ...extra,
  }

  let binary = ownBuild ? copyBuild(ORX, path.join(root, 'builds', '0')) : ORX
  let builds = 0
  let child: ChildProcess | undefined
  let port = 0
  const start = async () => {
    // A window names its port, which a restart into an update keeps; `0` would bind another.
    if (appWindow && port === 0) port = await freePort()
    const args = appWindow ? ['up', '--app-window', '--port', String(port)] : ['up', '--no-browser', '--port', String(port)]
    const started = spawn(binary, args, { env, stdio: [appWindow ? 'pipe' : 'ignore', 'pipe', 'pipe'] })
    child = started
    const output = createWriteStream(log, { flags: 'a' })
    started.stdout!.pipe(output)
    const lines = createInterface({ input: started.stderr! })
    const exited = new Promise<never>((_, reject) =>
      started.once('exit', (code, signal) => reject(new Error(`orx up exited (${signal ?? code}) before serving; see ${log}`))),
    )
    const serving = new Promise<number>((resolve) =>
      lines.on('line', (line) => {
        output.write(`${line}\n`)
        const match = /^orx up: dashboard on http:\/\/127\.0\.0\.1:(\d+)$/.exec(line)
        if (match) resolve(Number(match[1]))
      }),
    )
    port = await Promise.race([serving, exited])
  }

  const stop = async (signal: NodeJS.Signals = 'SIGTERM') => {
    const running = child
    child = undefined
    if (!running || running.exitCode !== null) return
    const exited = new Promise((resolve) => running.once('exit', resolve))
    running.kill(signal)
    await exited
  }

  await start()
  const url = `http://127.0.0.1:${port}`

  /** A request as the dashboard sends it, for arranging what the test is not about. */
  const api = async <T = unknown>(route: string, body?: object): Promise<T> => {
    const response = await fetch(`${url}${route}`, {
      method: body ? 'POST' : 'GET',
      headers: { 'content-type': 'application/json', origin: url },
      body: body && JSON.stringify(body),
    })
    if (!response.ok) throw new Error(`${route}: ${response.status} ${await response.text()}`)
    return (await response.json()) as T
  }

  return {
    url,
    root,
    log,
    /** The server's process, which a restart into an update execs in place. */
    pid: () => child?.pid,
    /** Where the server keeps its settings and update state. */
    config: path.join(env.XDG_CONFIG_HOME, 'openresearch'),
    agentLog: env.ORX_E2E_AGENT_LOG,
    /** Connects the fake agent and finishes onboarding with it, as the onboarding screens do. */
    async onboard() {
      const agent = { name: 'E2E Agent', executable: process.execPath, arguments: [FAKE_AGENT] }
      const { id: harness } = await api<{ id: string }>('/api/harnesses/acp', agent)
      let model: string | undefined
      await expect
        .poll(
          async () => {
            const { harnesses } = await api<{ harnesses: { id: string; defaultModel?: string }[] }>('/api/harnesses')
            model = harnesses.find((h) => h.id === harness)?.defaultModel
            return model
          },
          { message: `${agent.name} reports its default model`, timeout: 30_000 },
        )
        .toBeTruthy()
      await api('/api/onboarding/complete', { harness, model })
      await api('/api/settings/ui-state', { tourCompleted: true })
    },
    /** A blank project, as the New project dialog creates one. */
    async createProject(name: string): Promise<Project> {
      const path = `${home}/projects/${name}`
      const body = { name, path, createFolder: true, requireNewFolder: true, initializeGit: true }
      const { project } = await api<{ project: { id: string } }>('/api/projects', body)
      return { id: project.id, open: async (page) => void (await page.goto(`${url}/projects/${project.id}`)) }
    },
    stop,
    /**
     * Stops `orx up` and starts it again on the same port and data, from `binary` if given,
     * doing `meanwhile` while it is down.
     */
    async restart(signal: NodeJS.Signals = 'SIGTERM', { binary: next, meanwhile }: { binary?: string; meanwhile?: () => Promise<void> } = {}) {
      await stop(signal)
      if (next) binary = next
      await meanwhile?.()
      await start()
    },
    /**
     * Copies the running build to a path of its own, then deletes the running build's files as an
     * update does, and returns the copy's `orx` for `restart`. Needs `ownBuild`.
     */
    replaceBuild() {
      if (!ownBuild) throw new Error('replaceBuild needs a server started with ownBuild')
      const next = copyBuild(binary, path.join(root, 'builds', String(++builds)))
      rmSync(path.dirname(binary), { recursive: true })
      return next
    },
  }
}
