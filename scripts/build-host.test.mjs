import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const script = fileURLToPath(new URL('build-host.mjs', import.meta.url))

test('host builder validates prerequisites and Darwin targets without checksum sidecars', { skip: process.platform === 'win32' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'orx-build-host-'))
  try {
    const bin = join(root, 'bin')
    mkdirSync(bin)
    const shim = name => `#!${process.execPath}\n${name}\n`
    writeFileSync(join(bin, 'rustup'), shim(`console.log(process.env.INSTALLED_TARGETS || '')`), { mode: 0o755 })
    writeFileSync(join(bin, 'zig'), shim('process.exit(0)'), { mode: 0o755 })
    writeFileSync(join(bin, 'lipo'), shim(`
const fs = require('node:fs'), args = process.argv.slice(2), split = args.indexOf('-output');
fs.writeFileSync(args[split + 1], Buffer.concat(args.slice(1, split).map(source => fs.readFileSync(source))));
`), { mode: 0o755 })
    writeFileSync(join(bin, 'cargo'), shim(`
const fs = require('node:fs'), path = require('node:path'), args = process.argv.slice(2);
if (args.includes('--help')) process.exit(process.env.NO_ZIGBUILD ? 1 : 0);
const target = args[args.indexOf('--target') + 1];
const dir = path.join(args[args.indexOf('--target-dir') + 1], target, 'host');
fs.mkdirSync(dir, {recursive:true});
fs.writeFileSync(path.join(dir, 'orx-host'), 'native stand-in');
`), { mode: 0o755 })
    // Run a copy so the fake cargo outputs cannot replace real build products.
    mkdirSync(join(root, 'scripts'))
    const copy = join(root, 'scripts/build-host.mjs')
    writeFileSync(copy, readFileSync(script))
    const target = 'x86_64-unknown-linux-musl'
    const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, INSTALLED_TARGETS: target }
    const run = (overrides, args = ['--target', target]) => execFileSync(process.execPath, [copy, ...args], { env: { ...env, ...overrides }, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
    assert.throws(() => run({ INSTALLED_TARGETS: '' }), /rustup target add x86_64-unknown-linux-musl/)
    assert.throws(() => run({ NO_ZIGBUILD: '1' }), /cargo install --locked cargo-zigbuild/)
    run({})
    const bytes = readFileSync(join(root, 'target/host-binaries/orx-host-linux-x86_64'))
    assert.equal(bytes.toString(), 'native stand-in')
    for (const darwin of ['aarch64-apple-darwin', 'x86_64-apple-darwin']) {
      assert.throws(() => run({}, ['--target', darwin]), /requires both --target aarch64-apple-darwin and --target x86_64-apple-darwin/)
    }
    run({ INSTALLED_TARGETS: 'aarch64-apple-darwin x86_64-apple-darwin' }, ['--target', 'aarch64-apple-darwin', '--target', 'x86_64-apple-darwin'])
    const universal = readFileSync(join(root, 'target/host-binaries/orx-host-darwin-universal'))
    assert.equal(universal.toString(), 'native stand-innative stand-in')
    assert.deepEqual(readdirSync(join(root, 'target/host-binaries')).sort(), ['orx-host-darwin-universal', 'orx-host-linux-x86_64'])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
