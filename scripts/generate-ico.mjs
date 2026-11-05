// Pack the OpenResearch mark into a multi-size Windows .ico (PNG-compressed
// entries, supported since Vista) for the app's executables and installer.
//
// Usage: node scripts/generate-ico.mjs <out.ico>

import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const out = process.argv[2];
if (!out) {
  console.error('usage: node scripts/generate-ico.mjs <out.ico>');
  process.exit(1);
}
const SIZES = [16, 24, 32, 48, 64, 128, 256];
const generator = join(import.meta.dirname, 'generate-icon.mjs');
const tmp = mkdtempSync(join(tmpdir(), 'orx-ico-'));
try {
  const images = SIZES.map((size) => {
    const png = join(tmp, `${size}.png`);
    execFileSync(process.execPath, [generator, png, String(size)]);
    return readFileSync(png);
  });
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(images.length, 4);
  const entries = Buffer.alloc(16 * images.length);
  let offset = header.length + entries.length;
  images.forEach((png, i) => {
    const entry = i * 16;
    // A byte of 0 means 256 px.
    entries.writeUInt8(SIZES[i] % 256, entry);
    entries.writeUInt8(SIZES[i] % 256, entry + 1);
    entries.writeUInt16LE(1, entry + 4); // color planes
    entries.writeUInt16LE(32, entry + 6); // bits per pixel
    entries.writeUInt32LE(png.length, entry + 8);
    entries.writeUInt32LE(offset, entry + 12);
    offset += png.length;
  });
  writeFileSync(out, Buffer.concat([header, entries, ...images]));
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
