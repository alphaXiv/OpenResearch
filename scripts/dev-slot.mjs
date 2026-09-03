#!/usr/bin/env node

import { execFileSync, spawn } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import { closeSync, cpSync, existsSync, mkdirSync, openSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import http from 'node:http'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const SLOT_VERSION = 1
const FIRST_SLOT = 1
const LAST_SLOT = 9
const BACKEND_TIMEOUT_MS = 5 * 60 * 1000
const UI_TIMEOUT_MS = 60 * 1000
const SCRIPT_PATH = fileURLToPath(import.meta.url)

function usage() {
  return `Usage:
  scripts/dev-slot.mjs start --db empty|copy [--worktree PATH] [--open]
  scripts/dev-slot.mjs status [--worktree PATH]
  scripts/dev-slot.mjs stop [--worktree PATH]
  scripts/dev-slot.mjs cleanup [--worktree PATH]

Database modes:
  empty  Start with a new database and no projects.
  copy   Take a WAL-safe snapshot of the normal local CLI database and copy run logs.`
}

export function parseArgs(argv) {
  const [command, ...rest] = argv
  if (!['start', 'status', 'stop', 'cleanup'].includes(command)) {
    throw new Error(usage())
  }

  const options = { command, worktree: process.cwd(), db: null, open: false }
  for (let index = 0; index < rest.length; index += 1) {
    const arg = rest[index]
    if (arg === '--worktree') {
      const value = rest[index + 1]
      if (!value) throw new Error('--worktree requires a path')
      options.worktree = value
      index += 1
    } else if (arg === '--db') {
      const value = rest[index + 1]
      if (!['empty', 'copy'].includes(value)) throw new Error('--db must be empty or copy')
      options.db = value
      index += 1
    } else if (arg === '--open') {
      options.open = true
    } else if (arg === '--help' || arg === '-h') {
      throw new Error(usage())
    } else {
      throw new Error(`Unknown argument: ${arg}\n\n${usage()}`)
    }
  }

  if (command === 'start' && options.db === null) {
    throw new Error('start requires --db empty or --db copy')
  }
  if (command !== 'start' && (options.db !== null || options.open)) {
    throw new Error(`--db and --open are only valid with start`)
  }
  return options
}

function run(command, args, options = {}) {
  return execFileSync(command, args, { encoding: 'utf8', ...options }).trim()
}

function worktreeInfo(worktreeInput) {
  const worktreePath = realpathSync(worktreeInput)
  const manifestPath = path.join(worktreePath, 'Cargo.toml')
  if (!existsSync(manifestPath)) throw new Error(`Not an openresearch-cli worktree: ${worktreePath}`)

  const records = run('git', ['-C', worktreePath, 'worktree', 'list', '--porcelain'])
    .split(/\n\n+/)
    .map((block) => Object.fromEntries(block.split('\n').map((line) => {
      const split = line.indexOf(' ')
      return split === -1 ? [line, true] : [line.slice(0, split), line.slice(split + 1)]
    })))
  const mainWorktrees = records.filter((record) => record.branch === 'refs/heads/main')
  if (mainWorktrees.length !== 1) throw new Error('Expected exactly one worktree on refs/heads/main')

  return {
    worktreePath,
    manifestPath,
    worktreeKey: createHash('sha256').update(worktreePath).digest('hex').slice(0, 16),
    label: path.basename(worktreePath).replaceAll(/[^a-zA-Z0-9._-]/g, '-'),
    mainCheckout: realpathSync(mainWorktrees[0].worktree),
  }
}

function pathsFor(info, slot = null) {
  const allocatorRoot = path.join(os.homedir(), '.local', 'share', 'openresearch-dev')
  const base = {
    allocatorRoot,
    cargoTargetDir: path.join(allocatorRoot, 'cargo-target'),
    launchRegistry: path.join(info.mainCheckout, '.claude', 'launch.json'),
    lockDir: path.join(allocatorRoot, 'slot-allocator.lock'),
    lifecycleLockDir: path.join(allocatorRoot, 'lifecycle-locks', info.worktreeKey),
  }
  if (slot === null) return base
  const slotKey = `slot-${slot}`
  return {
    ...base,
    slot,
    slotKey,
    backendPort: 4900 + slot,
    uiPort: 5200 + slot,
    dataDir: path.join(allocatorRoot, slotKey),
    cacheDir: path.join(allocatorRoot, slotKey, 'cache'),
    configDir: path.join(allocatorRoot, `${slotKey}-config`),
    statePath: path.join(allocatorRoot, `${slotKey}-state.json`),
    logsDir: path.join(allocatorRoot, 'logs', slotKey),
  }
}

export function slotEnvironment(slotPaths) {
  return {
    ORX_DATA_DIR: slotPaths.dataDir,
    ORX_CACHE_DIR: slotPaths.cacheDir,
    XDG_CONFIG_HOME: slotPaths.configDir,
    CARGO_TARGET_DIR: slotPaths.cargoTargetDir,
  }
}

function atomicWriteJson(destination, value, mode = 0o600) {
  mkdirSync(path.dirname(destination), { recursive: true, mode: 0o700 })
  const temporary = `${destination}.tmp-${process.pid}-${Date.now()}`
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode })
  renameSync(temporary, destination)
}

function readJson(file, fallback = null) {
  if (!existsSync(file)) return fallback
  return JSON.parse(readFileSync(file, 'utf8'))
}

function processExists(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    if (error.code === 'EPERM') return true
    if (error.code === 'ESRCH') return false
    throw error
  }
}

function processGroupExists(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false
  try {
    process.kill(-pid, 0)
    return true
  } catch (error) {
    if (error.code === 'EPERM') return true
    if (error.code === 'ESRCH') return false
    throw error
  }
}

function listenerPids(port) {
  try {
    return run('lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-t'])
      .split('\n')
      .filter(Boolean)
      .map(Number)
      .filter(Number.isInteger)
  } catch (error) {
    if (error.status === 1) return []
    throw error
  }
}

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds))

export async function acquireAdvisoryLock(lockPath) {
  if (process.platform === 'win32') throw new Error('Dev-slot lifecycle management requires macOS or Linux')
  mkdirSync(path.dirname(lockPath), { recursive: true, mode: 0o700 })
  const token = randomUUID()
  const utility = process.platform === 'darwin' ? 'lockf' : 'flock'
  const args = process.platform === 'darwin'
    ? ['-t', '60', lockPath, process.execPath, SCRIPT_PATH, '_hold-lock', token]
    : ['-w', '60', lockPath, process.execPath, SCRIPT_PATH, '_hold-lock', token]
  const holder = spawn(utility, args, { stdio: ['pipe', 'pipe', 'pipe'] })

  await new Promise((resolve, reject) => {
    let stdout = ''
    let stderr = ''
    const fail = (message) => reject(new Error(`Could not acquire ${lockPath}: ${message}`))
    holder.stdout.on('data', (chunk) => {
      stdout += chunk
      if (stdout.includes(`LOCKED ${token}\n`)) resolve()
    })
    holder.stderr.on('data', (chunk) => { stderr += chunk })
    holder.once('error', (error) => fail(error.message))
    holder.once('exit', (code) => fail(stderr.trim() || `${utility} exited with status ${code}`))
  })

  let released = false
  return async () => {
    if (released) return
    released = true
    holder.stdin.end()
    if (holder.exitCode !== null || holder.signalCode !== null) return
    await new Promise((resolve) => holder.once('exit', resolve))
  }
}

async function holdAdvisoryLock(token) {
  console.log(`LOCKED ${token}`)
  process.stdin.resume()
  await new Promise((resolve) => process.stdin.once('end', resolve))
}

async function acquireAllocatorLock(info) {
  return acquireAdvisoryLock(pathsFor(info).lockDir)
}

async function acquireLifecycleLock(info) {
  return acquireAdvisoryLock(pathsFor(info).lifecycleLockDir)
}

export function resolveLiveDataDir(environment = process.env, home = os.homedir()) {
  const configHome = environment.XDG_CONFIG_HOME || path.join(home, '.config')
  let settings = null
  try {
    settings = readJson(path.join(configHome, 'openresearch', 'settings.json'))
  } catch {
    settings = null
  }
  if (typeof settings?.dataDir === 'string' && settings.dataDir.length > 0) return settings.dataDir
  const dataHome = environment.XDG_DATA_HOME || path.join(home, '.local', 'share')
  return path.join(dataHome, 'openresearch')
}

export function sqliteBackupCommand(destination) {
  const escaped = destination.replaceAll('\\', '\\\\').replaceAll('"', '\\"')
  return `.backup "${escaped}"`
}

export function initializeDatabase(mode, destination, source = resolveLiveDataDir()) {
  mkdirSync(destination, { recursive: true, mode: 0o700 })
  if (mode === 'empty') return { sourceDb: null, copiedRunLogs: false }

  const sourceDb = path.join(source, 'orx.db')
  if (!existsSync(sourceDb)) throw new Error(`Normal CLI database not found: ${sourceDb}`)
  const destinationDb = path.join(destination, 'orx.db')
  const temporaryDb = path.join(destination, `.orx.db.backup-${process.pid}`)
  run('sqlite3', [sourceDb, sqliteBackupCommand(temporaryDb)])
  renameSync(temporaryDb, destinationDb)

  const sourceLogs = path.join(source, 'run-logs')
  const destinationLogs = path.join(destination, 'run-logs')
  if (existsSync(sourceLogs)) cpSync(sourceLogs, destinationLogs, { recursive: true })
  return { sourceDb, copiedRunLogs: existsSync(sourceLogs) }
}

function loadRegistry(info) {
  const { launchRegistry } = pathsFor(info)
  mkdirSync(path.dirname(launchRegistry), { recursive: true, mode: 0o700 })
  return readJson(launchRegistry, { version: '0.0.1', configurations: [] })
}

function slotFromPort(port) {
  if (Number.isInteger(port) && port >= 4901 && port <= 4909) return port - 4900
  if (Number.isInteger(port) && port >= 5201 && port <= 5209) return port - 5200
  return null
}

function manifestFromConfiguration(configuration, mainCheckout) {
  const args = configuration.runtimeArgs || []
  const index = args.indexOf('--manifest-path')
  if (index === -1 || !args[index + 1]) return null
  const manifest = args[index + 1]
  return path.isAbsolute(manifest) ? manifest : path.resolve(mainCheckout, manifest)
}

function configurationOwnsSlot(configuration, slot) {
  return slotFromPort(configuration.port) === slot
}

export function managedStateMatches(state, info, slotPaths) {
  return state?.version === SLOT_VERSION
    && state.slotKey === slotPaths.slotKey
    && state.worktreePath === info.worktreePath
    && state.manifestPath === info.manifestPath
    && ['empty', 'copy'].includes(state.dbMode)
    && state.backendPort === slotPaths.backendPort
    && state.uiPort === slotPaths.uiPort
}

function configurationMatchesDirectories(slotPaths, configuration) {
  const args = configuration.runtimeArgs || []
  return args.includes(`ORX_DATA_DIR=${slotPaths.dataDir}`)
    && args.includes(`XDG_CONFIG_HOME=${slotPaths.configDir}`)
}

function removeManagedDirectories(slotPaths, configuration, mainCheckout) {
  const state = readJson(slotPaths.statePath)
  const manifestPath = manifestFromConfiguration(configuration, mainCheckout)
  const owner = manifestPath ? { worktreePath: path.dirname(manifestPath), manifestPath } : null
  if (!owner || !managedStateMatches(state, owner, slotPaths)) return false
  if (!configurationMatchesDirectories(slotPaths, configuration)) return false
  rmSync(slotPaths.dataDir, { recursive: true, force: true })
  rmSync(slotPaths.configDir, { recursive: true, force: true })
  rmSync(slotPaths.statePath, { force: true })
  rmSync(slotPaths.logsDir, { recursive: true, force: true })
  return true
}

function reclaimStaleReservations(info, registry) {
  const configurations = [...registry.configurations]
  const removed = new Set()
  for (let slot = FIRST_SLOT; slot <= LAST_SLOT; slot += 1) {
    const owned = configurations.filter((configuration) => configurationOwnsSlot(configuration, slot))
    if (owned.length === 0) continue
    const backend = owned.find((configuration) => configuration.port === 4900 + slot)
    const manifest = backend ? manifestFromConfiguration(backend, info.mainCheckout) : null
    if (!manifest || existsSync(path.dirname(manifest))) continue
    if (listenerPids(4900 + slot).length > 0 || listenerPids(5200 + slot).length > 0) continue
    const slotPaths = pathsFor(info, slot)
    const state = readJson(slotPaths.statePath)
    const owner = { worktreePath: path.dirname(manifest), manifestPath: manifest }
    if (!managedStateMatches(state, owner, slotPaths)) continue
    if (processGroupExists(state.backendProcess?.pid) || processGroupExists(state.uiProcess?.pid)) continue
    if (!removeManagedDirectories(slotPaths, backend, info.mainCheckout)) continue
    removed.add(slot)
  }
  if (removed.size > 0) {
    registry.configurations = configurations.filter((configuration) => !removed.has(slotFromPort(configuration.port)))
  }
}

function configurationFor(info, slotPaths) {
  return [
    {
      name: `orx-${slotPaths.slot}-${info.label}`,
      runtimeExecutable: 'env',
      runtimeArgs: [
        ...Object.entries(slotEnvironment(slotPaths)).map(([key, value]) => `${key}=${value}`),
        'cargo', 'run', '--manifest-path', info.manifestPath,
        '--', 'up', '--no-browser', '--port', String(slotPaths.backendPort),
      ],
      port: slotPaths.backendPort,
    },
    {
      name: `ui-${slotPaths.slot}-${info.label}`,
      runtimeExecutable: 'env',
      runtimeArgs: [
        `ORX_BACKEND=http://127.0.0.1:${slotPaths.backendPort}`,
        'pnpm', '-C', path.join(info.worktreePath, 'ui'),
        'dev', '--port', String(slotPaths.uiPort), '--strictPort',
      ],
      port: slotPaths.uiPort,
    },
  ]
}

function stateFor(info, slotPaths, dbMode, database) {
  return {
    version: SLOT_VERSION,
    slotKey: slotPaths.slotKey,
    worktreePath: info.worktreePath,
    manifestPath: info.manifestPath,
    dbMode,
    sourceDb: database.sourceDb,
    copiedRunLogs: database.copiedRunLogs,
    phase: 'stopped',
    backendPort: slotPaths.backendPort,
    uiPort: slotPaths.uiPort,
    backendProcess: null,
    uiProcess: null,
    backendLog: path.join(slotPaths.logsDir, 'backend.log'),
    uiLog: path.join(slotPaths.logsDir, 'ui.log'),
  }
}

async function reserveSlot(info, dbMode) {
  const release = await acquireAllocatorLock(info)
  try {
    const registry = loadRegistry(info)
    reclaimStaleReservations(info, registry)
    const existing = registry.configurations.find((configuration) =>
      configuration.port >= 4901
      && configuration.port <= 4909
      && manifestFromConfiguration(configuration, info.mainCheckout) === info.manifestPath)
    if (existing) {
      const slotPaths = pathsFor(info, existing.port - 4900)
      const state = readJson(slotPaths.statePath)
      if (!managedStateMatches(state, info, slotPaths)) {
        throw new Error(`Existing ${slotPaths.slotKey} is legacy or unmanaged; stop it and run cleanup before using the helper`)
      }
      if (state.dbMode !== dbMode) {
        throw new Error(`${slotPaths.slotKey} already uses the ${state.dbMode} database mode`)
      }
      atomicWriteJson(pathsFor(info).launchRegistry, registry)
      return { slotPaths, state, reused: true }
    }

    let slot = null
    for (let candidate = FIRST_SLOT; candidate <= LAST_SLOT; candidate += 1) {
      const reserved = registry.configurations.some((configuration) => configurationOwnsSlot(configuration, candidate))
      const listening = listenerPids(4900 + candidate).length > 0 || listenerPids(5200 + candidate).length > 0
      if (reserved || listening) continue
      const candidatePaths = pathsFor(info, candidate)
      const hasDirectories = existsSync(candidatePaths.dataDir)
        || existsSync(candidatePaths.configDir)
        || existsSync(candidatePaths.statePath)
      if (hasDirectories) {
        const orphan = readJson(candidatePaths.statePath)
        const recoverable = managedStateMatches(orphan, info, candidatePaths)
          && !processGroupExists(orphan.backendProcess?.pid)
          && !processGroupExists(orphan.uiProcess?.pid)
        if (!recoverable) continue
      }
      slot = candidate
      break
    }
    if (slot === null) throw new Error('No free OpenResearch dev slots')

    const slotPaths = pathsFor(info, slot)
    if (existsSync(slotPaths.dataDir) || existsSync(slotPaths.configDir) || existsSync(slotPaths.statePath)) {
      const orphan = readJson(slotPaths.statePath)
      const noListeners = listenerPids(slotPaths.backendPort).length === 0 && listenerPids(slotPaths.uiPort).length === 0
      const noProcesses = !processGroupExists(orphan?.backendProcess?.pid) && !processGroupExists(orphan?.uiProcess?.pid)
      if (!managedStateMatches(orphan, info, slotPaths) || !noListeners || !noProcesses) {
        throw new Error(`Unregistered directories exist for ${slotPaths.slotKey}; refusing to overwrite them`)
      }
      rmSync(slotPaths.dataDir, { recursive: true, force: true })
      rmSync(slotPaths.configDir, { recursive: true, force: true })
      rmSync(slotPaths.statePath, { force: true })
      rmSync(slotPaths.logsDir, { recursive: true, force: true })
    }

    let state
    try {
      const sourceDb = dbMode === 'copy' ? path.join(resolveLiveDataDir(), 'orx.db') : null
      state = stateFor(info, slotPaths, dbMode, { sourceDb, copiedRunLogs: false })
      state.phase = 'preparing'
      atomicWriteJson(slotPaths.statePath, state)
      const database = initializeDatabase(dbMode, slotPaths.dataDir)
      mkdirSync(slotPaths.cacheDir, { recursive: true, mode: 0o700 })
      mkdirSync(slotPaths.configDir, { recursive: true, mode: 0o700 })
      mkdirSync(slotPaths.cargoTargetDir, { recursive: true, mode: 0o700 })
      mkdirSync(slotPaths.logsDir, { recursive: true, mode: 0o700 })
      state.sourceDb = database.sourceDb
      state.copiedRunLogs = database.copiedRunLogs
      state.phase = 'stopped'
      atomicWriteJson(slotPaths.statePath, state)
      registry.configurations.push(...configurationFor(info, slotPaths))
      atomicWriteJson(slotPaths.launchRegistry, registry)
      return { slotPaths, state, reused: false }
    } catch (error) {
      rmSync(slotPaths.dataDir, { recursive: true, force: true })
      rmSync(slotPaths.configDir, { recursive: true, force: true })
      rmSync(slotPaths.statePath, { force: true })
      rmSync(slotPaths.logsDir, { recursive: true, force: true })
      throw error
    }
  } finally {
    await release()
  }
}

async function supervise(token, command, args) {
  if (!token || !command) throw new Error('Invalid supervisor invocation')
  const child = spawn(command, args, { stdio: 'inherit' })
  const handlers = new Map()
  for (const signal of ['SIGINT', 'SIGTERM']) {
    const handler = () => {
      if (processExists(child.pid)) child.kill(signal)
    }
    handlers.set(signal, handler)
    process.on(signal, handler)
  }
  await new Promise((resolve, reject) => {
    const cleanup = () => {
      for (const [name, handler] of handlers) process.removeListener(name, handler)
    }
    child.once('error', (error) => {
      cleanup()
      reject(error)
    })
    child.once('exit', (code, signal) => {
      cleanup()
      process.exitCode = signal ? 1 : (code ?? 1)
      resolve()
    })
  })
}

export function supervisorCommandMatches(command, token, scriptPath = SCRIPT_PATH) {
  return typeof command === 'string'
    && path.isAbsolute(scriptPath)
    && command.includes(scriptPath)
    && command.includes(`_supervise ${token} `)
}

function supervisorMatches(record) {
  if (!record || !processGroupExists(record.pid)) return false
  try {
    const command = run('ps', ['-ww', '-p', String(record.pid), '-o', 'command='])
    return supervisorCommandMatches(command, record.token, record.scriptPath)
  } catch (error) {
    if (error.status === 1) return false
    throw error
  }
}

function listenerOwnedBy(port, record) {
  if (!supervisorMatches(record)) return false
  return listenerPids(port).some((pid) => {
    try {
      return Number(run('ps', ['-p', String(pid), '-o', 'pgid='])) === record.pid
    } catch (error) {
      if (error.status === 1) return false
      throw error
    }
  })
}

function spawnSupervised(command, args, options, logPath) {
  mkdirSync(path.dirname(logPath), { recursive: true, mode: 0o700 })
  const log = openSync(logPath, 'w', 0o600)
  const token = randomUUID()
  const child = spawn(process.execPath, [SCRIPT_PATH, '_supervise', token, command, ...args], {
    ...options,
    detached: true,
    stdio: ['ignore', log, log],
  })
  closeSync(log)
  child.on('error', (error) => writeFileSync(logPath, `${error.message}\n`, { flag: 'a' }))
  if (!child.pid) throw new Error(`Failed to launch ${command}`)
  child.unref()
  return { pid: child.pid, token, scriptPath: SCRIPT_PATH }
}

function httpReady(url) {
  return new Promise((resolve) => {
    const request = http.get(url, (response) => {
      response.resume()
      resolve(response.statusCode >= 200 && response.statusCode < 300)
    })
    request.setTimeout(1000, () => request.destroy())
    request.on('error', () => resolve(false))
  })
}

function logTail(logPath) {
  if (!existsSync(logPath)) return '(no log output)'
  return readFileSync(logPath, 'utf8').trim().split('\n').slice(-20).join('\n') || '(no log output)'
}

async function waitFor(description, timeout, predicate, processRecord = null, logPath = null) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (await predicate()) return
    if (processRecord && !supervisorMatches(processRecord)) {
      throw new Error(`${description} exited before readiness. Log: ${logPath}\n${logTail(logPath)}`)
    }
    await delay(250)
  }
  const details = logPath ? ` Log: ${logPath}\n${logTail(logPath)}` : ''
  throw new Error(`Timed out waiting for ${description}.${details}`)
}

async function waitForBackend(state) {
  await waitFor('the backend readiness message', BACKEND_TIMEOUT_MS, () =>
    existsSync(state.backendLog) && readFileSync(state.backendLog, 'utf8').includes('dashboard on'),
  state.backendProcess, state.backendLog)
  await waitFor('the backend API', BACKEND_TIMEOUT_MS, () =>
    httpReady(`http://127.0.0.1:${state.backendPort}/api/projects`),
  state.backendProcess, state.backendLog)
}

async function waitForUi(state) {
  await waitFor('the UI', UI_TIMEOUT_MS, () => httpReady(`http://localhost:${state.uiPort}/`),
    state.uiProcess, state.uiLog)
}

function ensureUiDependencies(info) {
  if (existsSync(path.join(info.worktreePath, 'ui', 'node_modules'))) return
  execFileSync('pnpm', ['-C', path.join(info.worktreePath, 'ui'), 'install', '--frozen-lockfile'], { stdio: 'inherit' })
}

function saveState(slotPaths, state) {
  atomicWriteJson(slotPaths.statePath, state)
}

async function startUnlocked(info, dbMode, openBrowser) {
  const reservation = await reserveSlot(info, dbMode)
  const { slotPaths } = reservation
  const state = reservation.state

  let backendListeners = listenerPids(slotPaths.backendPort)
  let uiListeners = listenerPids(slotPaths.uiPort)
  if (uiListeners.length > 0 && backendListeners.length === 0) {
    throw new Error(`Port ${slotPaths.uiPort} is listening without its backend`)
  }
  if (backendListeners.length > 0 && !listenerOwnedBy(slotPaths.backendPort, state.backendProcess)) {
    throw new Error(`Port ${slotPaths.backendPort} is not owned by this helper; refusing to reuse it`)
  }
  if (uiListeners.length > 0 && !listenerOwnedBy(slotPaths.uiPort, state.uiProcess)) {
    throw new Error(`Port ${slotPaths.uiPort} is not owned by this helper; refusing to reuse it`)
  }

  ensureUiDependencies(info)
  state.phase = 'starting'
  saveState(slotPaths, state)
  if (backendListeners.length === 0 && !supervisorMatches(state.backendProcess)) {
    console.log(`Starting ${slotPaths.slotKey} backend; log: ${state.backendLog}`)
    state.backendProcess = spawnSupervised('cargo', [
      'run', '--manifest-path', info.manifestPath,
      '--', 'up', '--no-browser', '--port', String(slotPaths.backendPort),
    ], {
      cwd: info.worktreePath,
      env: {
        ...process.env,
        ...slotEnvironment(slotPaths),
        ORX_UI_DEV_ORIGIN: `http://localhost:${slotPaths.uiPort}`,
      },
    }, state.backendLog)
    saveState(slotPaths, state)
  }
  await waitForBackend(state)
  backendListeners = listenerPids(slotPaths.backendPort)
  if (!listenerOwnedBy(slotPaths.backendPort, state.backendProcess)) {
    throw new Error(`Backend listener on ${slotPaths.backendPort} is outside its recorded process group`)
  }

  uiListeners = listenerPids(slotPaths.uiPort)
  if (uiListeners.length === 0 && !supervisorMatches(state.uiProcess)) {
    console.log(`Starting ${slotPaths.slotKey} UI; log: ${state.uiLog}`)
    state.uiProcess = spawnSupervised('pnpm', [
      '-C', path.join(info.worktreePath, 'ui'),
      'dev', '--port', String(slotPaths.uiPort), '--strictPort',
    ], {
      cwd: info.worktreePath,
      env: { ...process.env, ORX_BACKEND: `http://127.0.0.1:${slotPaths.backendPort}` },
    }, state.uiLog)
    saveState(slotPaths, state)
  }
  await waitForUi(state)
  if (!listenerOwnedBy(slotPaths.uiPort, state.uiProcess)) {
    throw new Error(`UI listener on ${slotPaths.uiPort} is outside its recorded process group`)
  }
  state.phase = 'running'
  saveState(slotPaths, state)

  if (openBrowser) {
    const url = `http://localhost:${slotPaths.uiPort}/`
    const opener = process.platform === 'darwin'
      ? ['open', [url]]
      : process.platform === 'win32'
        ? ['cmd', ['/c', 'start', '', url]]
        : ['xdg-open', [url]]
    const child = spawn(opener[0], opener[1], { detached: true, stdio: 'ignore' })
    child.on('error', (error) => console.warn(`Could not open the browser: ${error.message}`))
    child.unref()
  }

  console.log(`Dev slot ready (${reservation.reused ? 'reused' : dbMode})`)
  console.log(`  Worktree: ${info.worktreePath}`)
  console.log(`  Database: ${state.dbMode}`)
  console.log(`  Backend:  http://127.0.0.1:${slotPaths.backendPort}`)
  console.log(`  UI:       http://localhost:${slotPaths.uiPort}/`)
  console.log(`  State:    ${slotPaths.statePath}`)
}

async function start(info, dbMode, openBrowser) {
  const release = await acquireLifecycleLock(info)
  try {
    await startUnlocked(info, dbMode, openBrowser)
  } finally {
    await release()
  }
}

function reservationFor(info) {
  const registry = loadRegistry(info)
  const backend = registry.configurations.find((configuration) =>
    configuration.port >= 4901
    && configuration.port <= 4909
    && manifestFromConfiguration(configuration, info.mainCheckout) === info.manifestPath)
  if (!backend) return null
  const slotPaths = pathsFor(info, backend.port - 4900)
  return { registry, backend, slotPaths, state: readJson(slotPaths.statePath) }
}

function printStatus(info) {
  const reservation = reservationFor(info)
  if (!reservation) {
    console.log(`No dev slot is registered for ${info.worktreePath}`)
    return
  }
  const { slotPaths, state } = reservation
  console.log(`${slotPaths.slotKey} (${state?.dbMode || 'unknown database mode'})`)
  console.log(`  Backend: ${listenerPids(slotPaths.backendPort).length > 0 ? 'running' : 'stopped'} on ${slotPaths.backendPort}`)
  console.log(`  UI:      ${listenerPids(slotPaths.uiPort).length > 0 ? 'running' : 'stopped'} on ${slotPaths.uiPort}`)
  console.log(`  Data:    ${slotPaths.dataDir}`)
  console.log(`  Cache:   ${slotPaths.cacheDir}`)
  console.log(`  State:   ${state ? slotPaths.statePath : 'legacy/unmanaged'}`)
}

async function stopOwnedProcessesUnlocked(info) {
  const reservation = reservationFor(info)
  if (!reservation) return null
  const { slotPaths, state } = reservation
  if (!managedStateMatches(state, info, slotPaths)) {
    if (listenerPids(slotPaths.backendPort).length > 0 || listenerPids(slotPaths.uiPort).length > 0) {
      throw new Error(`${slotPaths.slotKey} was not launched by this helper; refusing to stop unowned processes`)
    }
    return reservation
  }

  for (const [record, port] of [
    [state.uiProcess, slotPaths.uiPort],
    [state.backendProcess, slotPaths.backendPort],
  ]) {
    const listeners = listenerPids(port)
    if (listeners.length > 0 && !listenerOwnedBy(port, record)) {
      throw new Error(`Port ${port} is not owned by this helper; refusing to stop it`)
    }
    if (!processGroupExists(record?.pid)) continue
    if (!supervisorMatches(record)) {
      throw new Error(`Process group ${record.pid} no longer matches its recorded launch identity`)
    }
    try {
      process.kill(-record.pid, 'SIGTERM')
    } catch (error) {
      if (error.code !== 'ESRCH') throw error
    }
  }
  await waitFor('dev-slot processes to stop', 10_000, () =>
    !processGroupExists(state.backendProcess?.pid)
    && !processGroupExists(state.uiProcess?.pid)
    && listenerPids(slotPaths.backendPort).length === 0
    && listenerPids(slotPaths.uiPort).length === 0)
  state.backendProcess = null
  state.uiProcess = null
  state.phase = 'stopped'
  saveState(slotPaths, state)
  console.log(`Stopped ${slotPaths.slotKey}`)
  return reservation
}

async function stopOwnedProcesses(info) {
  const release = await acquireLifecycleLock(info)
  try {
    return await stopOwnedProcessesUnlocked(info)
  } finally {
    await release()
  }
}

async function cleanupUnlocked(info) {
  let reservation = await stopOwnedProcessesUnlocked(info)
  if (!reservation) {
    console.log(`No dev slot is registered for ${info.worktreePath}`)
    return
  }

  const release = await acquireAllocatorLock(info)
  try {
    reservation = reservationFor(info)
    if (!reservation) return
    const { slotPaths, state, backend } = reservation
    if (listenerPids(slotPaths.backendPort).length > 0 || listenerPids(slotPaths.uiPort).length > 0) {
      throw new Error(`Refusing to clean ${slotPaths.slotKey} while its ports are listening`)
    }
    reservation.registry.configurations = reservation.registry.configurations.filter((configuration) =>
      !configurationOwnsSlot(configuration, slotPaths.slot))
    atomicWriteJson(slotPaths.launchRegistry, reservation.registry)
    if (managedStateMatches(state, info, slotPaths) && configurationMatchesDirectories(slotPaths, backend)) {
      rmSync(slotPaths.dataDir, { recursive: true, force: true })
      rmSync(slotPaths.configDir, { recursive: true, force: true })
      rmSync(slotPaths.statePath, { force: true })
      rmSync(slotPaths.logsDir, { recursive: true, force: true })
      console.log(`Cleaned ${slotPaths.slotKey}`)
    } else {
      console.log(`Removed unmanaged ${slotPaths.slotKey} launch entries; data directories were left unchanged`)
    }
  } finally {
    await release()
  }
}

async function cleanup(info) {
  const release = await acquireLifecycleLock(info)
  try {
    await cleanupUnlocked(info)
  } finally {
    await release()
  }
}

async function main() {
  if (process.argv[2] === '_hold-lock') {
    await holdAdvisoryLock(process.argv[3])
    return
  }
  if (process.argv[2] === '_supervise') {
    const [, token, command, ...args] = process.argv.slice(2)
    await supervise(token, command, args)
    return
  }
  const options = parseArgs(process.argv.slice(2))
  const info = worktreeInfo(options.worktree)
  if (options.command === 'start') await start(info, options.db, options.open)
  if (options.command === 'status') printStatus(info)
  if (options.command === 'stop') await stopOwnedProcesses(info)
  if (options.command === 'cleanup') await cleanup(info)
}

const isMain = process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)
if (isMain) {
  main().catch((error) => {
    console.error(`dev-slot: ${error.message}`)
    process.exitCode = 1
  })
}
