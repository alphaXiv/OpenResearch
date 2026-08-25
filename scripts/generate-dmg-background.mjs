// Draw the OpenResearch DMG installer-window background to an opaque RGB PNG.
//
// The styled DMG (see scripts/package-macos-app.sh) shows the app icon on the
// left and an /Applications alias on the right; this background paints a soft
// gradient, with an arrow between them so "drag to install" reads at a glance. The
// icon labels ("OpenResearch", "Applications") are drawn by Finder, not here.
//
// Pure Node (no image deps), matching scripts/generate-icon.mjs: we emit a PNG
// by hand. Usage: node scripts/generate-dmg-background.mjs <out.png> [scale]
// scale 1 -> 640x320 (@1x), scale 2 -> 1280x640 (@2x). Requires Node >= 22.2.

import zlib from 'node:zlib';
import { writeFileSync } from 'node:fs';

const out = process.argv[2];
if (!out) {
  console.error('usage: node scripts/generate-dmg-background.mjs <out.png> [scale]');
  process.exit(1);
}
const SCALE = Number(process.argv[3] ?? 1);
// The canvas and arrow geometry below are tuned to the window size and icon
// positions (WIN_W/WIN_H, APP_X/APPS_X, ICON_Y) in scripts/package-macos-app.sh.
const LOGICAL_W = 640, LOGICAL_H = 320;
const W = LOGICAL_W * SCALE, H = LOGICAL_H * SCALE;
const SS = 3; // supersample factor per axis (anti-aliasing)
const s = SCALE; // logical-unit -> pixel scale (layout below is in @1x units)

const RED = [0x9a, 0x20, 0x36];
const RED_SOFT = [0xc0, 0x3a, 0x52];
// Gradient: warm near-white at the top easing to pure white lower down.
const TOP = [0xfb, 0xf1, 0xf3], BOT = [0xff, 0xff, 0xff];

// Arrow from the app icon toward /Applications. x0 is the round cap's center; the
// span is centered on the icons' artwork gap (213..420), not their 128px cells.
const ARROW = { y: 150, x0: 253, shaftX1: 351, tipX: 387, shaftH: 13, headH: 42 };

const clamp = (v, lo, hi) => (v < lo ? lo : v > hi ? hi : v);
const lerp = (a, b, t) => [
  Math.round(a[0] + (b[0] - a[0]) * t),
  Math.round(a[1] + (b[1] - a[1]) * t),
  Math.round(a[2] + (b[2] - a[2]) * t),
];

// --- arrow (rounded shaft + triangular head), logical coords ----------------
function inArrow(lx, ly) {
  const { y, x0, shaftX1, tipX, shaftH, headH } = ARROW;
  // shaft with rounded caps
  if (lx >= x0 && lx <= shaftX1 && Math.abs(ly - y) <= shaftH / 2) return true;
  const capR = shaftH / 2;
  if (Math.hypot(lx - x0, ly - y) <= capR) return true;
  // triangular head: linearly narrowing from headH at shaftX1 to 0 at tipX
  if (lx >= shaftX1 && lx <= tipX) {
    const t = (tipX - lx) / (tipX - shaftX1);
    if (Math.abs(ly - y) <= (headH / 2) * t) return true;
  }
  return false;
}

// --- compose ----------------------------------------------------------------
function sample(px, py) {
  const lx = px / s, ly = py / s;
  const base = lerp(TOP, BOT, clamp(ly / LOGICAL_H, 0, 1));
  if (inArrow(lx, ly)) {
    // subtle vertical shading on the arrow for a bit of depth
    return lerp(RED_SOFT, RED, clamp((ly - (ARROW.y - 22)) / 44, 0, 1));
  }
  return base;
}

const raw = Buffer.alloc(H * (W * 3 + 1)); // RGB + 1 filter byte per row
let o = 0;
for (let y = 0; y < H; y++) {
  raw[o++] = 0; // PNG filter: none
  for (let x = 0; x < W; x++) {
    let r = 0, g = 0, b = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const c = sample(x + (sx + 0.5) / SS, y + (sy + 0.5) / SS);
        r += c[0]; g += c[1]; b += c[2];
      }
    }
    const n = SS * SS;
    raw[o++] = Math.round(r / n);
    raw[o++] = Math.round(g / n);
    raw[o++] = Math.round(b / n);
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
ihdr.writeUInt32BE(W, 0); ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8; ihdr[9] = 2; // 8-bit, RGB
const png = Buffer.concat([
  sig,
  chunk('IHDR', ihdr),
  chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);
writeFileSync(out, png);
console.log(`wrote ${out} (${W}x${H}, ${png.length} bytes)`);
