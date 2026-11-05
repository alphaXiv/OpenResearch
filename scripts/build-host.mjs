import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync, copyFileSync } from 'node:fs'
import { resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../', import.meta.url))
const targets = [
  'x86_64-unknown-linux-musl',
  'aarch64-unknown-linux-musl',
  'x86_64-apple-darwin',
  'aarch64-apple-darwin',
]

try {
  let output = resolve(root, 'target/host-binaries')
  const selected = []
  for (let i = 2; i < process.argv.length; i++) {
    const arg = process.argv[i]
    if (arg === '--out-dir' && process.argv[i + 1]) output = resolve(process.argv[++i])
    else if (arg === '--target' && targets.includes(process.argv[i + 1])) selected.push(process.argv[++i])
    else throw new Error(`Unknown argument: ${arg}; use --out-dir <directory> or --target <Rust target>`)
  }
  if (!selected.length) selected.push(...targets.filter(target => process.platform === 'darwin' || !target.includes('darwin')))
  const darwinTargets = selected.filter(target => target.includes('apple-darwin'))
  const missingDarwinTarget = targets.find(target => target.includes('apple-darwin') && !selected.includes(target))
  if (darwinTargets.length && missingDarwinTarget) {
    throw new Error('Universal Darwin orx-host requires both --target aarch64-apple-darwin and --target x86_64-apple-darwin')
  }
  const installed = spawnSync('rustup', ['target', 'list', '--installed'], { encoding: 'utf8' })
  if (installed.status !== 0) throw new Error('Missing rustup; install Rust from https://rustup.rs')
  const darwin = []
  const zigbuild = spawnSync('cargo', ['zigbuild', '--help'], { stdio: 'ignore' }).status === 0
  for (const target of selected) {
    if (!installed.stdout.split(/\s+/).includes(target)) throw new Error(`Missing Rust target ${target}; install with: rustup target add ${target}`)
    const musl = target.endsWith('linux-musl')
    if (musl && !zigbuild) throw new Error('Missing cargo-zigbuild; install with: cargo install --locked cargo-zigbuild')
    if (musl && spawnSync('zig', ['version'], { stdio: 'ignore' }).status !== 0) throw new Error('Missing Zig; install Zig and put zig on PATH (https://ziglang.org/download/)')
    const result = spawnSync('cargo', [musl ? 'zigbuild' : 'build', '-p', 'orx-host', '--profile', 'host', '--locked', '--target', target, '--target-dir', join(root, 'target')], { cwd: root, stdio: 'inherit' })
    if (result.status !== 0) throw new Error(`orx-host build failed for ${target}; check the compiler/linker diagnostics above`)
    const source = join(root, 'target', target, 'host/orx-host')
    if (target.includes('apple-darwin')) darwin.push(source)
    else writeArtifact(source, `orx-host-linux-${target.split('-')[0]}`, output)
  }
  if (darwin.length) {
    const universal = join(root, 'target/orx-host-darwin-universal')
    const result = spawnSync('lipo', ['-create', ...darwin, '-output', universal], { stdio: 'inherit' })
    if (result.error?.code === 'ENOENT') throw new Error('Missing lipo; install Apple command-line tools with: xcode-select --install')
    if (result.status !== 0) throw new Error('Failed to combine Darwin orx-host binaries with lipo')
    writeArtifact(universal, 'orx-host-darwin-universal', output)
  }
} catch (error) {
  console.error(error.message)
  process.exitCode = 1
}

function writeArtifact(source, name, output) {
  const bytes = readFileSync(source)
  if (!bytes.length || bytes.length > 4 * 1024 * 1024) throw new Error(`${name} must be between 1 byte and 4 MiB`)
  mkdirSync(output, { recursive: true })
  copyFileSync(source, join(output, name))
  console.log(`${name}: ${bytes.length} bytes`)
}
