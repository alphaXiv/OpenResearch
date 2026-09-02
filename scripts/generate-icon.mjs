// Rasterize the OpenResearch brand mark to a transparent-corner RGBA PNG.
//
// Source of truth for the shapes is ui/public/favicon.svg (a red squircle with
// a white right-triangle). QuickLook/`sips` flatten SVG transparency onto white,
// so we draw the geometry directly and emit straight-alpha RGBA — giving clean
// transparent corners for both the CLI Dock icon and the macOS .app iconset.
//
// Usage: node scripts/generate-icon.mjs <out.png> [size]   (default size 1024)
// Requires Node >= 22.2 (uses the built-in zlib.crc32).

import zlib from 'node:zlib';
import { writeFileSync } from 'node:fs';

const out = process.argv[2];
if (!out) {
  console.error('usage: node scripts/generate-icon.mjs <out.png> [size]');
  process.exit(1);
}
const N = Number(process.argv[3] ?? 1024);
const SS = 4; // supersample factor per axis (anti-aliasing)

// squircle inset within the canvas (macOS-icon-style padding), scaled to N
const s = N / 1024;
const X = 88 * s, Y = 88 * s, W = 848 * s, H = 848 * s, R = 188 * s;
// brand triangle (favicon path, translate 88 + scale 8.48), right angle at B
const A = [218.38 * s, 230.31 * s], B = [218.38 * s, 805.62 * s], C = [793.69 * s, 805.62 * s];
const RED = [0x9a, 0x20, 0x36];

const clamp = (v, lo, hi) => (v < lo ? lo : v > hi ? hi : v);
function inSquircle(px, py) {
  const cx = clamp(px, X + R, X + W - R), cy = clamp(py, Y + R, Y + H - R);
  const dx = px - cx, dy = py - cy;
  return dx * dx + dy * dy <= R * R;
}
function edge(p, a, b) {
  return (p[0] - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (p[1] - b[1]);
}
function inTriangle(px, py) {
  const p = [px, py];
  const d1 = edge(p, A, B), d2 = edge(p, B, C), d3 = edge(p, C, A);
  const neg = d1 < 0 || d2 < 0 || d3 < 0, pos = d1 > 0 || d2 > 0 || d3 > 0;
  return !(neg && pos);
}

const raw = Buffer.alloc(N * (N * 4 + 1)); // +1 filter byte per row
let o = 0;
for (let y = 0; y < N; y++) {
  raw[o++] = 0; // PNG filter: none
  for (let x = 0; x < N; x++) {
    let r = 0, g = 0, b = 0, cov = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const px = x + (sx + 0.5) / SS, py = y + (sy + 0.5) / SS;
        if (inTriangle(px, py)) { r += 255; g += 255; b += 255; cov++; }
        else if (inSquircle(px, py)) { r += RED[0]; g += RED[1]; b += RED[2]; cov++; }
      }
    }
    const S = SS * SS;
    raw[o++] = cov ? Math.round(r / cov) : 0;
    raw[o++] = cov ? Math.round(g / cov) : 0;
    raw[o++] = cov ? Math.round(b / cov) : 0;
    raw[o++] = Math.round((255 * cov) / S);
  }
}

function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(zlib.crc32(td) >>> 0);
  return Buffer.concat([len, td, crc]);
}
const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(N, 0); ihdr.writeUInt32BE(N, 4);
ihdr[8] = 8; ihdr[9] = 6; // 8-bit, RGBA
const png = Buffer.concat([
  sig,
  chunk('IHDR', ihdr),
  chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);
writeFileSync(out, png);
console.log(`wrote ${out} (${N}x${N}, ${png.length} bytes)`);
