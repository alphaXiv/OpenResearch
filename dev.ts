#!/usr/bin/env node

import { execFileSync, spawn, spawnSync, type ChildProcess, type SpawnOptions } from 'node:child_process'
import { createHash } from 'node:crypto'
import { closeSync, constants, copyFileSync, cpSync, existsSync, linkSync, lstatSync, mkdirSync, openSync, readdirSync, readFileSync, readSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { createInterface } from 'node:readline'
import { backup, DatabaseSync } from 'node:sqlite'
import { fileURLToPath } from 'node:url'
import { styleText } from 'node:util'

const SCRIPT = fileURLToPath(import.meta.url)
const ROOT = path.dirname(SCRIPT)
const DEV = path.join(ROOT, '.dev')
const LIVE = path.join(DEV, 'live')
const STATES = path.join(DEV, 'states')
const WINDOWS = process.platform === 'win32'
const EXE = WINDOWS ? '.exe' : ''
const BINARY = path.join(DEV, 'bin', `orx${EXE}`)
const SERVER_LOG = path.join(DEV, 'server.log')
const WORKTREE_HASH = createHash('sha256').update(ROOT).digest()
/**
 * A named pipe on Windows. Elsewhere a socket outside the worktree, which can be nested deeper than a
 * Unix socket path's ~100 bytes allow, and not under TMPDIR, which differs between a person's terminal
 * and an agent's sandbox.
 */
const SOCKET = WINDOWS
  ? `\\\\.\\pipe\\orx-dev-${WORKTREE_HASH.toString('hex', 0, 8)}`
  : path.join(os.homedir(), '.cache', 'orx-dev', `${WORKTREE_HASH.toString('hex', 0, 8)}.sock`)
const LABEL = path.basename(ROOT).replaceAll(/[^a-zA-Z0-9._-]/g, '-')
const APP_NAME = `OpenResearch ${LABEL}`
const APP = path.join(DEV, `${APP_NAME}.app`, 'Contents')
/** The dashboard's orange accent, so a dev window never passes for the installed app. */
const ICON_COLOR = '#da642c'

/** Draws `label` in white into the empty top-right half of the 1024px mark at `png`. Run with `osascript -l JavaScript`. */
const LABEL_ICON = String.raw`
ObjC.import('AppKit')
function run([png, label]) {
  const size = 1024
  const rep = $.NSBitmapImageRep.alloc.initWithBitmapDataPlanesPixelsWidePixelsHighBitsPerSampleSamplesPerPixelHasAlphaIsPlanarColorSpaceNameBytesPerRowBitsPerPixel(null, size, size, 8, 4, true, false, $.NSDeviceRGBColorSpace, 0, 0)
  $.NSGraphicsContext.saveGraphicsState
  $.NSGraphicsContext.setCurrentContext($.NSGraphicsContext.graphicsContextWithBitmapImageRep(rep))
  $.NSImage.alloc.initWithContentsOfFile(png).drawInRect($.NSMakeRect(0, 0, size, size))

  // AppKit's coordinates run bottom-up.
  const box = { x: 500, y: 560, width: 370, height: 300 }
  const text = $(label)
  let font, attributes
  for (let fontSize = 300; fontSize >= 90; fontSize -= 6) {
    font = $.NSFont.systemFontOfSizeWeight(fontSize, $.NSFontWeightHeavy)
    attributes = $.NSDictionary.dictionaryWithObjectsForKeys($([font, $.NSColor.whiteColor]), $([$.NSFontAttributeName, $.NSForegroundColorAttributeName]))
    if (text.sizeWithAttributes(attributes).width <= box.width) break
  }
  const width = text.sizeWithAttributes(attributes).width
  // Centers the cap height, so the glyphs rather than the line box sit in the middle.
  const baseline = box.y + (box.height - font.capHeight) / 2
  text.drawAtPointWithAttributes($.NSMakePoint(box.x + (box.width - width) / 2, baseline + font.descender), attributes)
  $.NSGraphicsContext.restoreGraphicsState
  rep.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $()).writeToFileAtomically(png, true)
}`

const USAGE = `Usage:
  node dev.ts [desktop] [--onboarding | --copy | --state NAME]
  node dev.ts status [--json]
  node dev.ts logs [api|web|app|build] [-n LINES]
  node dev.ts restart | stop
  node dev.ts save NAME

Each worktree has one dev server: the backend and Vite, with data in .dev/live. It runs in the
background and any number of terminals and agents share it. node dev.ts starts it when it is
down; in a terminal it then attaches, otherwise it waits until the server is ready, prints its
URLs, and exits. desktop also opens the app window. restart rebuilds the backend.

The first run starts with onboarding done. A state flag replaces .dev/live, keeping the old one
as .dev/states/previous:
  --onboarding   start empty, at onboarding
  --copy         copy your installed app's database and run logs
  --state NAME   load a state saved with \`save NAME\``

const env = {
  ...process.env,
  ORX_DATA_DIR: path.join(LIVE, 'data'),
  XDG_CONFIG_HOME: path.join(LIVE, 'config'),
  ORX_CACHE_DIR: path.join(DEV, 'cache'),
}

const SOURCES = ['api', 'web', 'app', 'build'] as const
type Source = (typeof SOURCES)[number] | 'dev'
type Seed = { kind: 'onboarding' | 'copy' } | { kind: 'state'; name: string }
type Options =
  | { command: 'help' }
  | { command: 'save'; name: string }
  | { command: 'status'; json: boolean }
  | { command: 'logs'; source: Source | null; lines: number }
  | { command: 'restart' | 'stop' }
  | { command: 'run'; desktop: boolean; seed: Seed | null }
type Ports = { ui: number; backend: number }
type Owner = { pid: number; ppid: number; command: string }

type Entry = { seq: number; source: Source; text: string }
type Status = { pid: number; ready: boolean; problem: string | null; web: string; api: string; data: string; window: boolean; clients: number }
/** The first line a client sends. `attach` keeps the connection open for key frames; everything else ends at `code`. */
type AttachRequest = { command: 'attach'; owner: boolean; window: boolean; seed: Seed | null }
type Request =
  | { command: 'ping' | 'status' | 'wait' | 'restart' | 'stop' | 'window' }
  | { command: 'logs'; source: Source | null; lines: number }
  | { command: 'load'; seed: Seed }
  | AttachRequest
type Client = { owner: boolean; send: (frame: Frame) => void }
type Frame = { out?: string; entry?: Entry; status?: Status; key?: string; detached?: true; code?: number }

export function parseArgs(argv: string[]): Options {
  if (argv.includes('--help') || argv.includes('-h')) return { command: 'help' }
  const [command, ...rest] = argv
  const stateName = (name: string | undefined) => {
    if (!name || !/^[\w-][\w.-]*$/.test(name)) throw new Error(`Invalid state name: ${name ?? '(missing)'}`)
    return name
  }
  const noArguments = () => {
    if (rest.length > 0) throw new Error(`Unknown argument ${rest[0]}; see node dev.ts --help`)
  }

  if (command === 'save') {
    if (rest.length !== 1) throw new Error('Usage: node dev.ts save NAME')
    return { command: 'save', name: stateName(rest[0]) }
  }
  if (command === 'status') {
    const json = rest[0] === '--json'
    if (rest.length > (json ? 1 : 0)) throw new Error('Usage: node dev.ts status [--json]')
    return { command: 'status', json }
  }
  if (command === 'logs') {
    let source: Source | null = null
    let lines = 200
    for (let index = 0; index < rest.length; index += 1) {
      const arg = rest[index]
      if (arg === '-n') lines = Number(rest[++index])
      else if ((SOURCES as readonly string[]).includes(arg)) source = arg as Source
      else throw new Error('Usage: node dev.ts logs [api|web|app|build] [-n LINES]')
    }
    if (!Number.isInteger(lines) || lines < 1) throw new Error('-n takes a positive number of lines')
    return { command: 'logs', source, lines }
  }
  if (command === 'restart' || command === 'stop') {
    noArguments()
    return { command }
  }

  const desktop = command === 'desktop'
  const flags = desktop ? rest : argv
  let seed: Seed | null = null
  for (let index = 0; index < flags.length; index += 1) {
    const flag = flags[index]
    if (seed) throw new Error('Pass at most one of --onboarding, --copy, and --state')
    if (flag === '--onboarding' || flag === '--copy') seed = { kind: flag === '--copy' ? 'copy' : 'onboarding' }
    else if (flag === '--state') seed = { kind: 'state', name: stateName(flags[++index]) }
    else throw new Error(`Unknown argument ${flag}; see node dev.ts --help`)
  }
  if (desktop && process.platform !== 'darwin') throw new Error('The desktop window requires macOS')
  return { command: 'run', desktop, seed }
}

function seedArgs(seed: Seed | null) {
  if (!seed) return []
  return seed.kind === 'state' ? ['--state', seed.name] : [`--${seed.kind}`]
}

const delay = (milliseconds: number) => new Promise((resolve) => setTimeout(resolve, milliseconds))
const elapsed = (since: number) => `${((performance.now() - since) / 1000).toFixed(1)}s`
const stripAnsi = (text: string) => text.replaceAll(/\x1b\[[0-9;]*m/g, '')
/** Cargo's per-crate lines, which only a person watching a cold build wants. */
const isProgress = (entry: Entry) => entry.source === 'build' && /^\s*(Compiling|Checking|Downloaded|Downloading|Locking|Adding|Updating|Blocking)\b/.test(stripAnsi(entry.text))
/** Styled regardless of where the server's output goes; each client strips color it can't show. */
const paint = (format: Parameters<typeof styleText>[0], text: string) => styleText(format, text, { validateStream: false })

// ---------------------------------------------------------------------------------------------
// Ports and state

/** Derived from the worktree, so its URL and the page's localStorage survive restarts. Below macOS's ephemeral range. */
function portsAt(offset: number): Ports {
  const n = (WORKTREE_HASH.readUInt32BE() + offset) % 3000
  return { ui: 43000 + n, backend: 46000 + n }
}

function portFree(port: number, host: string) {
  return new Promise<boolean>((resolve) => {
    const server = net.createServer()
    server.once('error', () => resolve(false))
    server.listen(port, host, () => server.close(() => resolve(true)))
  })
}

function listener(port: number): Owner | null {
  try {
    const pid = execFileSync('lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-t'], { encoding: 'utf8' }).trim().split('\n')[0]
    const [ppid, ...command] = execFileSync('ps', ['-o', 'ppid=,command=', '-p', pid], { encoding: 'utf8' }).trim().split(/\s+/)
    return { pid: Number(pid), ppid: Number(ppid), command: command.join(' ') }
  } catch {
    return null
  }
}

/** Skips ports another worktree holds, and reclaims ones a killed server of this worktree left to launchd. */
async function choosePorts() {
  const free = (ports: Ports) => Promise.all([portFree(ports.ui, 'localhost'), portFree(ports.backend, '127.0.0.1')])
  for (let offset = 0; offset < 3000; offset += 1) {
    const ports = portsAt(offset)
    const [uiFree, backendFree] = await free(ports)
    const busy: (Owner | null)[] = []
    if (!uiFree) busy.push(listener(ports.ui))
    if (!backendFree) busy.push(listener(ports.backend))
    if (busy.length === 0) return ports

    const ours = busy.filter((owner): owner is Owner => owner?.command.includes(ROOT + path.sep) ?? false)
    if (ours.length < busy.length) continue
    const running = ours.find((owner) => owner.ppid !== 1)
    if (running) throw new Error(`Port ${portsAt(offset).ui} is held by pid ${running.pid}, an older dev run of this worktree`)
    for (const owner of ours) process.kill(owner.pid, 'SIGTERM')
    while ((await free(ports)).includes(false)) await delay(100)
    say(paint('dim', 'Stopped servers a killed run left behind'))
    return ports
  }
  throw new Error('No free dev ports')
}

async function backupDatabase(from: string, to: string) {
  const source = new DatabaseSync(from)
  try {
    await backup(source, to)
  } finally {
    source.close()
  }
}

function isSqlite(file: string) {
  const header = Buffer.alloc(16)
  const fd = openSync(file, 'r')
  readSync(fd, header, 0, header.length, 0)
  closeSync(fd)
  return header.toString('latin1') === 'SQLite format 3\0'
}

/**
 * Keeps hard links linked (orx's lifecycle lock is two linked paths, git clones share objects), and takes
 * databases through SQLite's backup API so a running app can be snapshotted.
 */
export async function copyState(from: string, to: string) {
  const databases: string[] = []
  const inodes = new Map<number, string[]>()
  for (const file of readdirSync(from, { recursive: true, encoding: 'utf8' })) {
    const source = path.join(from, file)
    const stat = lstatSync(source)
    if (!stat.isFile()) continue
    if (stat.nlink > 1) inodes.set(stat.ino, [...(inodes.get(stat.ino) ?? []), file])
    else if (/\.(db|sqlite3?)$/.test(file) && isSqlite(source)) databases.push(file)
  }
  const linked = [...inodes.values()].filter((files) => files.length > 1)
  const skipped = new Set([
    ...databases.flatMap((file) => ['', '-wal', '-shm', '-journal'].map((suffix) => file + suffix)),
    ...linked.flat(),
  ].map((file) => path.join(from, file)))

  cpSync(from, to, { recursive: true, verbatimSymlinks: true, mode: constants.COPYFILE_FICLONE, filter: (source) => !skipped.has(source) })
  for (const file of databases) await backupDatabase(path.join(from, file), path.join(to, file))
  for (const [first, ...rest] of linked) {
    copyFileSync(path.join(from, first), path.join(to, first), constants.COPYFILE_FICLONE)
    for (const file of rest) linkSync(path.join(to, first), path.join(to, file))
  }
}

function installedDataDir() {
  const config = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config')
  try {
    const { dataDir } = JSON.parse(readFileSync(path.join(config, 'openresearch', 'settings.json'), 'utf8'))
    if (typeof dataDir === 'string' && dataDir) return dataDir
  } catch {}
  return path.join(process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share'), 'openresearch')
}

/** A run's detached `orx supervise` keeps writing into the live state, so loading another would move it out from under the run. */
function refuseWhileRunsAreSupervised() {
  const runLogs = path.join(env.ORX_DATA_DIR, 'run-logs')
  const locks = existsSync(runLogs) ? readdirSync(runLogs).filter((name) => name.endsWith('.supervisor.lock')) : []
  if (!locks.length) return

  if (WINDOWS) {
    // No lsof: a file another process holds open refuses an open that shares nothing.
    // libuv's UV_FS_O_EXLOCK, which Node does not export.
    const exclusive = 0x10000000
    const held = locks.filter((name) => {
      try {
        closeSync(openSync(path.join(runLogs, name), constants.O_RDONLY | exclusive))
        return false
      } catch {
        return true
      }
    })
    if (held.length) throw new Error(`An experiment is still running (a process holds ${held.join(', ')}); cancel it or let it finish, then load again`)
    return
  }
  const holders = spawnSync('lsof', ['-t', ...locks.map((name) => path.join(runLogs, name))], { encoding: 'utf8' })
  const pids = (holders.stdout ?? '').split('\n').filter(Boolean)
  if (pids.length) throw new Error(`An experiment is still running (orx supervise, pid ${pids.join(', ')}); cancel it or let it finish, then load again`)
}

async function replaceLive(seed: Seed) {
  const next = `${LIVE}.next`
  rmSync(next, { recursive: true, force: true })

  if (seed.kind === 'state') {
    const source = path.join(STATES, seed.name)
    if (!existsSync(source)) {
      const saved = existsSync(STATES) ? readdirSync(STATES) : []
      throw new Error(`No state named ${seed.name}. ${saved.length ? `Saved: ${saved.join(', ')}` : 'Save one with node dev.ts save NAME'}`)
    }
    await copyState(source, next)
  } else {
    mkdirSync(path.join(next, 'data'), { recursive: true })
    mkdirSync(path.join(next, 'config'))
  }

  if (seed.kind === 'copy') {
    const source = installedDataDir()
    if (!existsSync(path.join(source, 'orx.db'))) throw new Error(`No database at ${source}`)
    await backupDatabase(path.join(source, 'orx.db'), path.join(next, 'data', 'orx.db'))
    const runLogs = path.join(source, 'run-logs')
    if (existsSync(runLogs)) await copyState(runLogs, path.join(next, 'data', 'run-logs'))
  }

  const loaded = seed.kind === 'state' ? `Loaded ${seed.name}` : seed.kind === 'copy' ? "Copied your installed app's database" : 'Started fresh at onboarding'
  if (existsSync(LIVE)) {
    if (existsSync(BINARY) && readdirSync(env.ORX_DATA_DIR).some((name) => name.startsWith('ssh-service-'))) {
      execFileSync(BINARY, ['ssh-service', 'stop'], { env, stdio: 'ignore' })
    }
    const previous = path.join(STATES, 'previous')
    rmSync(previous, { recursive: true, force: true })
    mkdirSync(STATES, { recursive: true })
    renameSync(LIVE, previous)
  }
  renameSync(next, LIVE)
  say(paint('dim', `${loaded}; the replaced state is in .dev/states/previous`))
}

// ---------------------------------------------------------------------------------------------
// The server: one per worktree, detached from whoever started it

/** Recent output, replayed to clients that attach later. */
const log: Entry[] = []
const listeners = new Set<(entry: Entry) => void>()
let sequence = 0

function emit(source: Source, text: string) {
  const entry = { seq: ++sequence, source, text }
  log.push(entry)
  if (log.length > 5000) log.shift()
  // The server's own stdout is .dev/server.log.
  console.log(stripAnsi(text))
  for (const listener of listeners) listener(entry)
}

const say = (text: string) => emit('dev', text)

const children = new Set<ChildProcess>()
const clients = new Set<Client>()
const sockets = new Set<net.Socket>()
let ports: Ports
let vite: ChildProcess | null = null
let backend: ChildProcess | null = null
let appWindow: ChildProcess | null = null
let windowWanted = false
let stopping = false
/** Held for the server's lifetime: SQLite's exclusive lock makes the server single-instance per worktree. */
let instanceLock: DatabaseSync | null = null

const alive = (child: ChildProcess | null) => child !== null && child.exitCode === null && child.signalCode === null

function start(command: string, args: string[], options: SpawnOptions = {}) {
  if (stopping) throw new Error('Stopping')
  // The detached server has no console on Windows, so each console child would open a window.
  const child = spawn(command, args, { env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true, ...options })
  if (child.pid === undefined) {
    // Spawn failures also emit `error`, which would otherwise crash before the message below.
    child.once('error', () => {})
    throw new Error(`Could not run ${path.basename(command)}; is it installed?`)
  }
  children.add(child)
  child.once('exit', () => children.delete(child))
  return child
}

const GUTTERS = { api: 'magenta', web: 'cyan', app: 'yellow' } as const

/** Emits a child's output, under a labeled gutter except for build tools; `rewrite` returns null to drop a line. */
function forward(child: ChildProcess, source: Source, rewrite = (line: string): string | null => line) {
  const gutter = source in GUTTERS ? `${paint(GUTTERS[source as keyof typeof GUTTERS], source)} ${paint('dim', '│')} ` : ''
  for (const stream of [child.stdout, child.stderr]) {
    if (!stream) continue
    createInterface({ input: stream }).on('line', (line) => {
      const shown = rewrite(line)
      if (shown !== null) emit(source, gutter + shown)
    })
  }
}

function run(command: string, args: string[], options: SpawnOptions = {}) {
  const child = start(command, args, { env: { ...process.env, CARGO_TERM_COLOR: 'always' }, ...options })
  forward(child, 'build')
  return new Promise<void>((resolve, reject) => {
    child.once('exit', (code) => code === 0 ? resolve() : reject(new Error(`${path.basename(command)} ${args[0]} failed`)))
  })
}

function exited(child: ChildProcess) {
  if (!alive(child)) return Promise.resolve()
  return new Promise((resolve) => child.once('exit', resolve))
}

async function stop(child: ChildProcess) {
  child.kill('SIGTERM')
  const timeout = setTimeout(() => child.kill('SIGKILL'), 10_000)
  await exited(child)
  clearTimeout(timeout)
}

async function waitUntilReady(name: string, url: string, child: ChildProcess) {
  const deadline = Date.now() + 60_000
  while (Date.now() < deadline) {
    if (!alive(child)) throw new Error(`${name} exited before it was ready`)
    try {
      if ((await fetch(url, { signal: AbortSignal.timeout(1000) })).ok) return
    } catch {}
    await delay(100)
  }
  throw new Error(`${name} was not ready within a minute`)
}

async function build() {
  await run('cargo', ['build'], { cwd: ROOT })
  const target = process.env.CARGO_TARGET_DIR || path.join(ROOT, 'target')
  mkdirSync(path.dirname(BINARY), { recursive: true })
  if (WINDOWS && existsSync(BINARY)) {
    // Windows renames a running executable but won't replace it; old copies go once nothing runs them.
    const old = path.join(DEV, 'bin', 'old')
    mkdirSync(old, { recursive: true })
    for (const name of readdirSync(old)) {
      try {
        rmSync(path.join(old, name))
      } catch {}
    }
    renameSync(BINARY, path.join(old, `orx-${Date.now()}.exe`))
  }
  rmSync(BINARY, { force: true })
  // A copy, so a rebuild of the worktree can't swap the binary that the running backend's hooks execute.
  copyFileSync(path.join(target, 'debug', `orx${EXE}`), BINARY, constants.COPYFILE_FICLONE)
  // Debug builds embed no orx-host and look for one beside their executable to wrap local runs.
  if (!WINDOWS) {
    const host = path.join(path.dirname(BINARY), 'orx-host')
    rmSync(host, { force: true })
    copyFileSync(path.join(target, 'debug', 'orx-host'), host, constants.COPYFILE_FICLONE)
  }
}

async function startBackend() {
  const child = start(BINARY, ['up', '--no-browser', '--port', String(ports.backend)], {
    env: { ...env, ORX_UI_DEV_ORIGIN: `http://localhost:${ports.ui}` },
  })
  // Drops the startup lines that status already reports.
  forward(child, 'api', (line) => /^orx up: (dashboard on|agents:) /.test(line) ? null : line.replace(/^orx up: /, ''))
  child.once('exit', (code, signal) => {
    if (stopping || child !== backend) return
    say(`${paint('red', `The backend exited (${signal ?? `code ${code}`}).`)} Rebuild and restart it with r or node dev.ts restart.`)
  })
  await waitUntilReady('The backend', `http://127.0.0.1:${ports.backend}/api/projects`, child)
  return child
}

async function startUi() {
  const ui = path.join(ROOT, 'ui')
  // Inside node_modules, so deleting it forces an install.
  const installed = path.join(ui, 'node_modules', '.dev-installed')
  const installedAt = statSync(installed, { throwIfNoEntry: false })?.mtimeMs ?? 0
  if (['package.json', 'pnpm-lock.yaml', 'pnpm-workspace.yaml'].some((file) => statSync(path.join(ui, file)).mtimeMs > installedAt)) {
    // Windows installs pnpm as a .cmd, which only cmd.exe runs.
    const [pnpm, ...prefix] = WINDOWS ? ['cmd.exe', '/d', '/c', 'pnpm'] : ['pnpm']
    await run(pnpm, [...prefix, 'install', '--frozen-lockfile', '--reporter=silent'], { cwd: ui })
    writeFileSync(installed, '')
  }

  // Vite's warnings and errors are on stderr; stdout is the banner and per-edit chatter.
  const child = start(process.execPath, [path.join(ui, 'node_modules', 'vite', 'bin', 'vite.js'), '--port', String(ports.ui), '--strictPort'], {
    cwd: ui,
    env: { ...env, ORX_BACKEND: `http://127.0.0.1:${ports.backend}` },
    stdio: ['ignore', 'ignore', 'pipe'],
  })
  forward(child, 'web')
  child.once('exit', () => {
    if (!stopping && child === vite) say(`${paint('red', 'Vite exited.')} Restart it with r or node dev.ts restart.`)
  })
  vite = child
  await waitUntilReady('Vite', `http://localhost:${ports.ui}/`, child)
}

async function finishOnboarding() {
  say(paint('dim', 'First run: finishing onboarding for you (node dev.ts --onboarding shows it)'))
  const post = async (route: string, body: object) => {
    const response = await fetch(`http://127.0.0.1:${ports.backend}${route}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    if (!response.ok) throw new Error(`${route}: ${await response.text()}`)
  }
  // Onboarding picks the agent's default model, which waits for its model catalog.
  const deadline = Date.now() + 60_000
  while (true) {
    const response = await fetch(`http://127.0.0.1:${ports.backend}/api/harnesses?harness=claude-code`)
    if (!response.ok) throw new Error(`/api/harnesses: ${await response.text()}`)
    const { harnesses } = await response.json() as { harnesses: { id: string; catalogPending?: boolean }[] }
    if (!harnesses.find((harness) => harness.id === 'claude-code')?.catalogPending) break
    if (Date.now() > deadline) throw new Error('Claude Code was still checking its models after a minute')
    await delay(250)
  }
  await post('/api/onboarding/complete', { harness: 'claude-code', researchAreas: ['AI/ML'] })
  await post('/api/settings/ui-state', { tourCompleted: true })
}

function openWindow() {
  windowWanted = false
  const icon = path.join(APP, 'Resources', 'AppIcon.icns')
  const created = !existsSync(icon)
  if (created) {
    const iconset = path.join(DEV, 'cache', 'AppIcon.iconset')
    const png = path.join(iconset, 'icon_512x512@2x.png')
    mkdirSync(iconset, { recursive: true })
    execFileSync(process.execPath, [path.join(ROOT, 'scripts', 'generate-icon.mjs'), png, '1024', ICON_COLOR])
    execFileSync('osascript', ['-l', 'JavaScript', '-e', LABEL_ICON, png, LABEL])
    mkdirSync(path.dirname(icon), { recursive: true })
    execFileSync('iconutil', ['-c', 'icns', iconset, '-o', icon])
  }
  const executable = path.join(APP, 'MacOS', 'OpenResearch')
  mkdirSync(path.dirname(executable), { recursive: true })
  rmSync(executable, { force: true })
  copyFileSync(BINARY, executable, constants.COPYFILE_FICLONE)
  writeFileSync(path.join(APP, 'Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDisplayName</key><string>${APP_NAME}</string>
  <key>CFBundleExecutable</key><string>OpenResearch</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIdentifier</key><string>org.alphaxiv.openresearch.dev.${WORKTREE_HASH.toString('hex', 0, 8)}</string>
  <key>CFBundleName</key><string>${APP_NAME}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
`)
  // The Dock and menu bar read the name and icon from Launch Services, which caches them per bundle.
  if (created) execFileSync('/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister', ['-f', path.dirname(APP)])

  const child = start(executable, ['--dev-desktop', String(ports.ui)])
  forward(child, 'app')
  appWindow = child
  child.once('exit', () => {
    if (child === appWindow) appWindow = null
  })
}

function problem() {
  if (failure) return failure
  if (operations > 0) return null
  if (!alive(vite)) return 'Vite is not running'
  if (!alive(backend)) return 'The backend is not running'
  return null
}

function status(): Status {
  return {
    pid: process.pid,
    ready: operations === 0 && problem() === null,
    problem: problem(),
    web: `http://localhost:${ports.ui}/`,
    api: `http://127.0.0.1:${ports.backend}`,
    data: LIVE,
    window: appWindow !== null,
    clients: clients.size,
  }
}

let operations = 0
let failure: string | null = null
/** The first log entry of the operation in progress, so a client arriving mid-way sees it from the start. */
let operationStart = 0
let queue = Promise.resolve()
const settledWaiters = new Set<() => void>()

/** Runs startup, restarts, and state loads one at a time. A failure is reported and leaves running whatever still runs. */
function lifecycle(operation: () => Promise<void>) {
  operations += 1
  const result = queue.then(async () => {
    failure = null
    operationStart = sequence + 1
    try {
      await operation()
    } catch (error) {
      if (stopping) return
      failure = (error as Error).message
      say(paint('red', failure))
    }
  }).finally(() => {
    operations -= 1
    if (operations > 0 || stopping) return
    if (windowWanted && status().ready) {
      const refused = tryOpenWindow()
      if (refused) say(paint('red', refused))
    }
    for (const settle of settledWaiters) settle()
    settledWaiters.clear()
    for (const client of clients) client.send({ status: status() })
  })
  queue = result
  return result
}

function settled() {
  return operations === 0 ? Promise.resolve() : new Promise<void>((resolve) => settledWaiters.add(resolve))
}

async function startup(seed: Seed | null) {
  const started = performance.now()
  if (seed) {
    refuseWhileRunsAreSupervised()
    await replaceLive(seed)
  }
  const fresh = !existsSync(LIVE)
  for (const dir of [env.ORX_DATA_DIR, env.XDG_CONFIG_HOME, env.ORX_CACHE_DIR]) mkdirSync(dir, { recursive: true })
  await Promise.all([
    startUi(),
    (async () => {
      await build()
      backend = await startBackend()
      if (fresh) await finishOnboarding()
    })(),
  ])
  say(paint('dim', `Ready in ${elapsed(started)}`))
}

async function restart() {
  const started = performance.now()
  try {
    await build()
  } catch {
    throw new Error(`The build failed${alive(backend) ? '; the previous backend is still running' : ''}`)
  }
  if (!alive(vite)) await startUi()
  const previous = backend
  backend = null
  if (previous) await stop(previous)
  backend = await startBackend()

  // The window runs the old binary too.
  const previousWindow = appWindow
  if (previousWindow) {
    appWindow = null
    windowWanted = true
    await stop(previousWindow)
  }
  say(`${paint('green', '✓')} Restarted in ${elapsed(started)}`)
}

async function load(seed: Seed) {
  refuseWhileRunsAreSupervised()
  const previousWindow = appWindow
  const previousBackend = backend
  appWindow = null
  backend = null
  for (const child of [previousWindow, previousBackend]) if (child) await stop(child)
  await replaceLive(seed)
  if (!existsSync(BINARY)) await build()
  backend = await startBackend()
  if (previousWindow) windowWanted = true
}

function openBrowser() {
  spawn(process.platform === 'darwin' ? 'open' : WINDOWS ? 'explorer' : 'xdg-open', [status().web], { stdio: 'ignore', detached: true })
    .on('error', (error) => say(paint('red', `Could not open the browser. ${error.message}`)))
    .unref()
}

function tryOpenWindow() {
  try {
    openWindow()
    return null
  } catch (error) {
    return `Could not open the window. ${(error as Error).message}`
  }
}

/** Opens the window now, or once the operation in progress leaves the server ready. Returns why it can't. */
function requestWindow() {
  if (process.platform !== 'darwin') return 'The desktop window requires macOS'
  if (appWindow) return null
  windowWanted = true
  return operations === 0 && status().ready ? tryOpenWindow() : null
}

async function shutdown(code = 0) {
  if (stopping) return
  stopping = true
  say(paint('dim', 'Stopping'))
  await Promise.all([...children].map(stop))
  for (const socket of sockets) {
    socket.end(`${JSON.stringify({ code })}\n`)
  }
  if (!WINDOWS) rmSync(SOCKET, { force: true })
  process.exit(code)
}

/** Streams log entries to a client until `until` settles, then reports whether the server is ready. */
async function streamUntil(send: (frame: Frame) => void, until: Promise<unknown>, from: number) {
  for (const entry of log) if (entry.seq >= from) send({ entry })
  const listener = (entry: Entry) => send({ entry })
  listeners.add(listener)
  try {
    await until
  } finally {
    listeners.delete(listener)
  }
  const current = status()
  send({ status: current })
  return current.ready ? 0 : 1
}

function sender(socket: net.Socket) {
  return (frame: Frame) => {
    if (!socket.destroyed) socket.write(`${JSON.stringify(frame)}\n`)
  }
}

function handle(socket: net.Socket, request: Exclude<Request, AttachRequest>) {
  const send = sender(socket)
  const finish = (code: number, out?: string) => {
    if (out) send({ out })
    send({ code })
    socket.end()
  }
  const finishWith = (result: Promise<number>) => void result.then((code) => finish(code), (error) => finish(1, String(error)))

  switch (request.command) {
    case 'ping':
      return finish(0)
    case 'status':
      return finish(0, JSON.stringify(status()))
    case 'logs': {
      const entries = log.filter((entry) => request.source === null || entry.source === request.source).slice(-request.lines)
      return finish(0, entries.map((entry) => entry.text).join('\n'))
    }
    case 'wait':
      return finishWith(streamUntil(send, settled(), operations > 0 ? operationStart : sequence + 1))
    case 'restart':
      return finishWith(streamUntil(send, lifecycle(restart), sequence + 1))
    case 'load':
      return finishWith(streamUntil(send, lifecycle(() => load(request.seed)), sequence + 1))
    case 'window': {
      const refused = requestWindow()
      return refused ? finish(1, refused) : finish(0)
    }
    case 'stop':
      finish(0)
      return void shutdown()
  }
}

/** Replays recent output to a terminal, then streams it live; the connection stays open for keys. */
function attach(socket: net.Socket, request: AttachRequest): Client {
  const client = { owner: request.owner, send: sender(socket) }
  clients.add(client)
  for (const entry of log.slice(-300)) client.send({ entry })
  const listener = (entry: Entry) => client.send({ entry })
  listeners.add(listener)
  socket.once('close', () => {
    clients.delete(client)
    listeners.delete(listener)
  })

  const seed = request.seed
  if (seed) void lifecycle(() => load(seed))
  if (request.window) {
    const refused = requestWindow()
    if (refused) say(paint('red', refused))
  }
  if (operations === 0) client.send({ status: status() })
  return client
}

function handleKey(client: Client, socket: net.Socket, key: string) {
  if (key === 'r' && operations === 0) void lifecycle(restart)
  if (key === 'o') openBrowser()
  if (key === 'd') {
    const refused = requestWindow()
    if (refused) say(paint('red', refused))
  }
  if (key === 'q' || key === '\x03') {
    if (client.owner && clients.size === 1) return void shutdown()
    client.send({ detached: true })
    socket.end()
  }
}

async function serve(seed: Seed | null) {
  mkdirSync(DEV, { recursive: true })
  instanceLock = new DatabaseSync(path.join(DEV, 'server.lock'))
  try {
    instanceLock.exec('BEGIN EXCLUSIVE')
  } catch {
    console.error('Another dev server is running for this worktree')
    process.exit(3)
  }

  // A server killed outright leaves its window running; the ports below reclaim everything else.
  if (!WINDOWS) {
    for (const line of execFileSync('ps', ['-Ao', 'pid=,ppid=,command='], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 }).split('\n')) {
      const [pid, ppid, ...command] = line.trim().split(/\s+/)
      if (ppid === '1' && command.join(' ').startsWith(path.join(APP, 'MacOS') + path.sep)) process.kill(Number(pid), 'SIGTERM')
    }
  }
  try {
    ports = await choosePorts()
  } catch (error) {
    console.error((error as Error).message)
    process.exit(1)
  }

  if (!WINDOWS) {
    mkdirSync(path.dirname(SOCKET), { recursive: true, mode: 0o700 })
    rmSync(SOCKET, { force: true })
  }
  const server = net.createServer((socket) => {
    sockets.add(socket)
    socket.once('close', () => sockets.delete(socket))
    socket.on('error', () => {})
    let client: Client | null = null
    readFrames(socket, (raw) => {
      const frame = raw as Request & Frame
      if (client) {
        if (typeof frame.key === 'string') handleKey(client, socket, frame.key)
      } else if (frame.command === 'attach') {
        client = attach(socket, frame)
      } else {
        handle(socket, frame)
      }
    })
  })
  await new Promise<void>((resolve, reject) => server.once('error', reject).listen(SOCKET, resolve))
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, () => void shutdown())
  process.on('uncaughtException', (error) => {
    say(paint('red', error.stack ?? error.message))
    void shutdown(1)
  })
  void lifecycle(() => startup(seed))
}

// ---------------------------------------------------------------------------------------------
// Clients: the launcher, and commands for agents and scripts

function readFrames(socket: net.Socket, onFrame: (frame: unknown) => void) {
  let buffer = ''
  socket.setEncoding('utf8')
  socket.on('data', (chunk: string) => {
    buffer += chunk
    const lines = buffer.split('\n')
    buffer = lines.pop() ?? ''
    for (const line of lines) if (line) onFrame(JSON.parse(line))
  })
}

/** Sends one request, resolving with its exit code, or null when no server is listening. */
function request(message: Request, onFrame: (frame: Frame) => void = () => {}): Promise<number | null> {
  return new Promise((resolve) => {
    let code: number | null = null
    let connected = false
    const socket = net.connect(SOCKET)
    socket.on('connect', () => {
      connected = true
      socket.write(`${JSON.stringify(message)}\n`)
    })
    socket.on('error', () => {})
    socket.on('close', () => resolve(connected ? code ?? 1 : null))
    readFrames(socket, (raw) => {
      const frame = raw as Frame
      if (typeof frame.code === 'number') code = frame.code
      onFrame(frame)
    })
  })
}

async function fetchStatus(): Promise<Status | null> {
  let status: Status | null = null
  const code = await request({ command: 'status' }, (frame) => {
    if (frame.out) status = JSON.parse(frame.out)
  })
  return code === null ? null : status
}

function printEntry(entry: Entry) {
  if (process.stdout.isTTY) return console.log(entry.text)
  if (!isProgress(entry)) console.log(stripAnsi(entry.text))
}

function summary(status: Status) {
  const state = status.ready ? styleText('dim', 'ready') : styleText('red', status.problem ?? 'starting')
  return [
    '',
    `  ${styleText('bold', APP_NAME)}  ${state}`,
    '',
    `  ${styleText('dim', 'web ')}  ${styleText('cyan', status.web)}`,
    `  ${styleText('dim', 'api ')}  ${status.api}`,
    `  ${styleText('dim', 'data')}  ${path.relative(process.cwd(), status.data) || '.'}`,
    '',
  ].join('\n')
}

async function spawnServer(seed: Seed | null) {
  mkdirSync(DEV, { recursive: true })
  const output = openSync(SERVER_LOG, 'w')
  const child = spawn(process.execPath, [...process.execArgv, SCRIPT, '_serve', ...seedArgs(seed)], {
    cwd: ROOT,
    detached: true,
    stdio: ['ignore', output, output],
    windowsHide: true,
  })
  closeSync(output)
  child.unref()
  let exitCode: number | null = null
  child.once('exit', (code) => {
    exitCode = code ?? 1
  })
  const deadline = Date.now() + 60_000
  while ((await request({ command: 'ping' })) === null) {
    // Exit code 3 means a launcher racing this one started the server first.
    if ((exitCode !== null && exitCode !== 3) || Date.now() > deadline) {
      throw new Error(`The dev server did not start:\n${readFileSync(SERVER_LOG, 'utf8').trim().split('\n').slice(-20).join('\n')}`)
    }
    await delay(50)
  }
}

/** Mirrors the server's log on this terminal and sends it keys, until the server stops or detaches it. */
function attachTerminal(options: { owner: boolean; window: boolean; seed: Seed | null }) {
  const { stdin, stdout } = process
  const socket = net.connect(SOCKET)
  const send = (frame: Request | Frame) => socket.write(`${JSON.stringify(frame)}\n`)
  send({ command: 'attach', ...options })

  let code: number | null = null
  let detached = false
  let summarized = false
  readFrames(socket, (raw) => {
    const frame = raw as Frame
    if (frame.entry) printEntry(frame.entry)
    if (frame.status && !summarized) {
      summarized = true
      const keys = [
        ['r', 'rebuild'],
        ['o', 'open in browser'],
        ...(process.platform === 'darwin' ? [['d', 'open window']] : []),
        ['q', options.owner && frame.status.clients === 1 ? 'stop' : 'detach'],
      ]
      stdout.write(`${summary(frame.status)}\n  ${keys.map(([key, action]) => `${styleText('bold', key)} ${styleText('dim', action)}`).join('   ')}\n\n`)
    }
    if (frame.detached) detached = true
    if (typeof frame.code === 'number') code = frame.code
  })

  const onKey = (data: Buffer) => send({ key: data.toString() })
  stdin.setRawMode(true)
  stdin.on('data', onKey)
  stdin.resume()
  socket.on('error', () => {})
  return new Promise<number>((resolve) => {
    socket.on('close', () => {
      stdin.off('data', onKey)
      stdin.setRawMode(false)
      stdin.pause()
      if (detached) console.log(styleText('dim', 'Detached; the dev server keeps running. node dev.ts stop stops it.'))
      else if (code === null) console.error(`The dev server exited unexpectedly; see ${path.relative(process.cwd(), SERVER_LOG)}`)
      resolve(detached ? 0 : code ?? 1)
    })
  })
}

async function main() {
  const argv = process.argv.slice(2)
  if (argv[0] === '_serve') {
    const options = parseArgs(argv.slice(1))
    return serve(options.command === 'run' ? options.seed : null)
  }

  const options = parseArgs(argv)
  const notRunning = () => {
    console.error('The dev server is not running; start it with node dev.ts')
    process.exitCode = 2
  }
  const printFrame = (frame: Frame) => {
    if (frame.entry) printEntry(frame.entry)
    if (frame.out) console.log(process.stdout.isTTY ? frame.out : stripAnsi(frame.out))
  }

  switch (options.command) {
    case 'help':
      return console.log(USAGE)
    case 'save': {
      if (!existsSync(LIVE)) throw new Error('Nothing to save yet; run node dev.ts first')
      const destination = path.join(STATES, options.name)
      rmSync(destination, { recursive: true, force: true })
      await copyState(LIVE, destination)
      return console.log(`Saved .dev/states/${options.name}. Load it with node dev.ts --state ${options.name}`)
    }
    case 'status': {
      const status = await fetchStatus()
      if (!status) return notRunning()
      return console.log(options.json ? JSON.stringify(status, null, 2) : summary(status))
    }
    case 'logs': {
      const code = await request({ command: 'logs', source: options.source, lines: options.lines }, printFrame)
      if (code === null) return notRunning()
      return
    }
    case 'restart': {
      const code = await request({ command: 'restart' }, printFrame)
      if (code === null) return notRunning()
      process.exitCode = code
      return
    }
    case 'stop': {
      if ((await request({ command: 'stop' })) === null) console.log('The dev server is not running')
      return
    }
    case 'run': {
      const running = (await request({ command: 'ping' })) !== null
      if (!running) await spawnServer(options.seed)
      const seed = running ? options.seed : null
      if (process.stdin.isTTY && process.stdout.isTTY) {
        process.exitCode = await attachTerminal({ owner: !running, window: options.desktop, seed })
        return
      }

      const code = seed ? await request({ command: 'load', seed }, printFrame) : await request({ command: 'wait' }, printFrame)
      if (code === 0 && options.desktop) await request({ command: 'window' }, printFrame)
      const status = await fetchStatus()
      if (status) console.log(stripAnsi(summary(status)))
      process.exitCode = code ?? 1
    }
  }
}

if (process.argv[1] && realpathSync(process.argv[1]) === SCRIPT) {
  main().catch((error: Error) => {
    console.error(`${styleText('red', 'dev:')} ${error.message}`)
    process.exitCode = 1
  })
}
