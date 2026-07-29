#!/usr/bin/env node
// Generate the Lucidos brand icon / favicon asset set from the finalized mark.
//
// The "Lucidos mark" (L of apps + spark) is authored once, here, in a 0..100
// grid — three rounded app tiles forming a subtle "L" plus a 4-point spark in
// the top-right (the missing 4th tile). It sits white (#fff) on the brand
// radial gradient. This script is the SINGLE source of truth for the mark
// geometry, the gradient, and the safe-area sizing; it emits both the vector
// assets (favicon.svg, icon-source.svg) and every raster (PNG + multi-res ICO).
//
// Pure Node — no system rasterizer, no npm deps, no network. PNGs are rendered
// by supersampled per-pixel evaluation of the gradient + mark coverage and
// encoded via zlib; the ICO is a hand-built PNG-in-ICO container. Re-run with:
//   node crates/lucidos-app/scripts/generate-brand-icons.mjs
//
// To tweak the brand color, change GRAD_HIGHLIGHT / GRAD_EDGE / THEME_COLOR and
// re-run — every asset regenerates from these constants in one pass.

import { deflateSync } from 'node:zlib';
import { writeFileSync, readFileSync, existsSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const PUBLIC = resolve(__dirname, '..', 'public'); // web/PWA assets
const ICONS = resolve(PUBLIC, 'icons'); // web/PWA icon set
const NATIVE_ICONS = resolve(__dirname, '..', 'icons'); // native Tauri icon set (icon.icns/.ico, Square tiles, android/ios)

// ── Brand gradient ──────────────────────────────────────────────────────────
// Single-light-source azure→cobalt wash, light source upper-left. Anchored on
// the Lucidos accent (#0969da, --accent in light theme) and deepened — the
// earlier #3fa3ec→#2473d0 read as washed-out / too light.
//   CSS equivalent:
//   radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%)
const GRAD_HIGHLIGHT = '#2d83e0'; // 0%   — upper-left glow (lightened accent)
const GRAD_EDGE = '#0a4ea8'; //      100% — deep cobalt at the far edge
const THEME_COLOR = '#0969da'; //    the canonical Lucidos accent (brand blue)
// Gradient geometry, in fractions of the (square) canvas — matches the CSS
// `... at 30% 22%` center and `125% 125%` size (radius = 1.25 × side).
const GRAD_CX = 0.30,
  GRAD_CY = 0.22,
  GRAD_R = 1.25;

// ── The Lucidos mark, authored in a 0..100 grid ─────────────────────────────
const TILES = [
  { x: 17, y: 17, w: 29, h: 29, r: 7 },
  { x: 17, y: 54, w: 29, h: 29, r: 7 },
  { x: 54, y: 54, w: 29, h: 29, r: 7 },
];
const SPARK_D =
  'M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 ' +
  'C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z';
// Cubic-bezier control points matching SPARK_D, for the raster fill.
const SPARK_BEZIERS = [
  [[68.5, 12], [71, 25], [74, 28.5], [87, 31]],
  [[87, 31], [74, 33.5], [71, 37], [68.5, 50]],
  [[68.5, 50], [66, 37], [63, 33.5], [50, 31]],
  [[50, 31], [63, 28.5], [66, 25], [68.5, 12]],
];

// ── Safe-area / mark scale (two mask regimes) ───────────────────────────────
// How much room the mark leaves around itself is governed by how aggressively
// the destination surface masks the art — NOT by pixel size. The full-bleed and
// favicon surfaces share ONE scale so the installed-app icon and the browser-tab
// favicon read identically (they used to be 0.78 / 0.80 — close, but the maskable
// install variant at 0.62 made the "app" look far rounder than the "web page"):
//   • Full-bleed / lightly-masked surfaces — macOS squircle, iOS, apple-touch,
//     PWA purpose:"any", the master SVG — the mark runs at ~74%, a touch more
//     border padding than before, so it reads like a typical platform icon.
//   • Favicon family — browsers don't mask; it shares the full-bleed 74% so the
//     tab favicon matches the app icon exactly.
//   • Maskable / adaptive surfaces — Android adaptive foreground, PWA
//     purpose:"maskable" — raised from the old over-conservative 62% to 72% so
//     the install icon matches the rest, while the bottom-right tile's corner
//     still sits inside Android's standard 80%-diameter safe circle.
const SCALE_FULLBLEED = 0.74; // macOS / iOS / apple-touch / PWA "any" / master SVG
const SCALE_MASKABLE = 0.72; // Android adaptive foreground / PWA "maskable"
const SCALE_FAVICON = 0.74; // browser-tab favicon — matches SCALE_FULLBLEED
const FAVICON_SVG_RADIUS = 0.22; // self-rounded rounded-rect bg for favicon.svg
// Native Android adaptive foreground densities (`cargo tauri icon` output dirs).
const ANDROID_DENSITIES = [
  'mipmap-mdpi',
  'mipmap-hdpi',
  'mipmap-xhdpi',
  'mipmap-xxhdpi',
  'mipmap-xxxhdpi',
];

// ── color helpers ───────────────────────────────────────────────────────────
function hexToRgb(hex) {
  const h = hex.replace('#', '');
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}
const HI = hexToRgb(GRAD_HIGHLIGHT);
const ED = hexToRgb(GRAD_EDGE);

// Gradient color at canvas pixel (px,py) for a size×size canvas.
function gradientAt(px, py, size) {
  const cx = GRAD_CX * size,
    cy = GRAD_CY * size,
    rad = GRAD_R * size;
  const d = Math.hypot(px - cx, py - cy);
  let t = d / rad;
  if (t < 0) t = 0;
  else if (t > 1) t = 1;
  return [HI[0] + (ED[0] - HI[0]) * t, HI[1] + (ED[1] - HI[1]) * t, HI[2] + (ED[2] - HI[2]) * t];
}

// ── mark coverage (in canvas space) ─────────────────────────────────────────
// Signed distance to a rounded rect (≤0 means inside).
function roundedRectSDF(px, py, X, Y, W, H, R) {
  const cx = X + W / 2,
    cy = Y + H / 2;
  const qx = Math.abs(px - cx) - (W / 2 - R);
  const qy = Math.abs(py - cy) - (H / 2 - R);
  const ax = Math.max(qx, 0),
    ay = Math.max(qy, 0);
  return Math.hypot(ax, ay) + Math.min(Math.max(qx, qy), 0) - R;
}

// Flatten the spark beziers into a polygon (grid space), once.
function sparkPolygon(steps = 48) {
  const pts = [];
  for (const [p0, p1, p2, p3] of SPARK_BEZIERS) {
    for (let i = 0; i < steps; i++) {
      const t = i / steps;
      const u = 1 - t;
      const a = u * u * u,
        b = 3 * u * u * t,
        c = 3 * u * t * t,
        d = t * t * t;
      pts.push([
        a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
        a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
      ]);
    }
  }
  return pts;
}
const SPARK_POLY = sparkPolygon();

function pointInPolygon(px, py, poly) {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const xi = poly[i][0],
      yi = poly[i][1],
      xj = poly[j][0],
      yj = poly[j][1];
    const intersect = yi > py !== yj > py && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi;
    if (intersect) inside = !inside;
  }
  return inside;
}

// Is canvas point (px,py) inside the white mark? `scale` is the mark fraction
// of the canvas; the 0..100 grid maps to a centered scale×size square.
function inMark(px, py, size, scale) {
  const span = scale * size;
  const off = (size - span) / 2;
  const k = span / 100; // grid units → canvas px
  // grid-space point
  const gx = (px - off) / k;
  const gy = (py - off) / k;
  if (gx < -2 || gx > 102 || gy < -2 || gy > 102) return false;
  for (const t of TILES) {
    if (roundedRectSDF(gx, gy, t.x, t.y, t.w, t.h, t.r) <= 0) return true;
  }
  return pointInPolygon(gx, gy, SPARK_POLY);
}

// ── PNG encoding ────────────────────────────────────────────────────────────
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, 'ascii');
  const body = Buffer.concat([typeBuf, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}
// Encode an RGB (opaque) raster, full-bleed, as a PNG buffer.
function encodePNG(size, rgb) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // color type 2 = truecolor RGB
  ihdr[10] = 0; // deflate
  ihdr[11] = 0; // filter
  ihdr[12] = 0; // no interlace
  // filtered scanlines (filter byte 0 per row)
  const stride = size * 3;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0;
    rgb.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride);
  }
  const idat = deflateSync(raw, { level: 9 });
  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  return Buffer.concat([sig, chunk('IHDR', ihdr), chunk('IDAT', idat), chunk('IEND', Buffer.alloc(0))]);
}

// Render a full-bleed square icon → RGB buffer (supersampled mark + gradient).
function renderRGB(size, scale, ss = 4) {
  const rgb = Buffer.alloc(size * size * 3);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let r = 0,
        g = 0,
        b = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const px = x + (sx + 0.5) / ss;
          const py = y + (sy + 0.5) / ss;
          const grad = gradientAt(px, py, size);
          if (inMark(px, py, size, scale)) {
            r += 255;
            g += 255;
            b += 255;
          } else {
            r += grad[0];
            g += grad[1];
            b += grad[2];
          }
        }
      }
      const n = ss * ss;
      const i = (y * size + x) * 3;
      rgb[i] = Math.round(r / n);
      rgb[i + 1] = Math.round(g / n);
      rgb[i + 2] = Math.round(b / n);
    }
  }
  return rgb;
}

function pngIcon(size, scale) {
  return encodePNG(size, renderRGB(size, scale));
}

// ── ICO (PNG-in-ICO, supported by all modern browsers + Windows 7+) ─────────
function buildICO(entries) {
  // entries: [{ size, png }]
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type 1 = icon
  header.writeUInt16LE(entries.length, 4);
  const dir = Buffer.alloc(16 * entries.length);
  let offset = 6 + dir.length;
  const blobs = [];
  entries.forEach((e, idx) => {
    const o = idx * 16;
    dir[o] = e.size >= 256 ? 0 : e.size; // width
    dir[o + 1] = e.size >= 256 ? 0 : e.size; // height
    dir[o + 2] = 0; // palette
    dir[o + 3] = 0; // reserved
    dir.writeUInt16LE(1, o + 4); // planes
    dir.writeUInt16LE(32, o + 6); // bpp
    dir.writeUInt32LE(e.png.length, o + 8); // size
    dir.writeUInt32LE(offset, o + 12); // offset
    offset += e.png.length;
    blobs.push(e.png);
  });
  return Buffer.concat([header, dir, ...blobs]);
}

// ── SVG authoring ───────────────────────────────────────────────────────────
function markSVG(scale) {
  // Place the 0..100 mark grid into a centered scale×100-unit square within a
  // 0 0 100 100 viewBox via a <g transform>.
  const span = 100 * scale;
  const off = (100 - span) / 2;
  const tiles = TILES.map(
    (t) => `<rect x="${t.x}" y="${t.y}" width="${t.w}" height="${t.h}" rx="${t.r}"/>`,
  ).join('');
  return `  <g transform="translate(${off} ${off}) scale(${scale})" fill="#ffffff">
    ${tiles}
    <path d="${SPARK_D}"/>
  </g>`;
}
function gradientDefSVG(id) {
  // userSpaceOnUse in the 0..100 viewBox → exact match with the raster math
  // (center 30,22 ; radius 125 = 1.25 × the 100-unit side).
  return `  <defs>
    <radialGradient id="${id}" gradientUnits="userSpaceOnUse" cx="${GRAD_CX * 100}" cy="${GRAD_CY * 100}" r="${GRAD_R * 100}">
      <stop offset="0" stop-color="${GRAD_HIGHLIGHT}"/>
      <stop offset="1" stop-color="${GRAD_EDGE}"/>
    </radialGradient>
  </defs>`;
}
function faviconSVG() {
  const r = FAVICON_SVG_RADIUS * 100;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
${gradientDefSVG('lucidosBrand')}
  <rect x="0" y="0" width="100" height="100" rx="${r}" fill="url(#lucidosBrand)"/>
${markSVG(SCALE_FAVICON)}
</svg>
`;
}
function masterSVG() {
  // Full-bleed square master (no platform rounding baked in — masks are applied
  // by macOS/iOS/Android). Documents the source for the raster masters.
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
${gradientDefSVG('lucidosBrand')}
  <rect x="0" y="0" width="100" height="100" fill="url(#lucidosBrand)"/>
${markSVG(SCALE_FULLBLEED)}
</svg>
`;
}

// ── emit ────────────────────────────────────────────────────────────────────
mkdirSync(ICONS, { recursive: true });

writeFileSync(resolve(PUBLIC, 'favicon.svg'), faviconSVG());
writeFileSync(resolve(ICONS, 'icon-source.svg'), masterSVG());

// Favicon family (full-bleed square, enlarged mark for small-size legibility).
const fav16 = pngIcon(16, SCALE_FAVICON);
const fav32 = pngIcon(32, SCALE_FAVICON);
const fav48 = pngIcon(48, SCALE_FAVICON);
writeFileSync(resolve(PUBLIC, 'favicon-16.png'), fav16);
writeFileSync(resolve(PUBLIC, 'favicon-32.png'), fav32);
writeFileSync(resolve(PUBLIC, 'favicon-48.png'), fav48);
// favicon.png: legacy unreferenced fallback some tooling probes — overwrite the
// old brain with the new mark (32px).
writeFileSync(resolve(PUBLIC, 'favicon.png'), fav32);
writeFileSync(resolve(PUBLIC, 'favicon.ico'), buildICO([
  { size: 16, png: fav16 },
  { size: 32, png: fav32 },
  { size: 48, png: fav48 },
]));

// App-icon family. Full-bleed PNGs at SCALE_FULLBLEED for the lightly-masked /
// unmasked surfaces (macOS, iOS, apple-touch, PWA "any"); the manifest pairs the
// icon-NNN.png set with a matching icon-NNN-maskable.png at SCALE_MASKABLE for
// purpose:"maskable" (Android home-screen install crops the corners hard).
writeFileSync(resolve(ICONS, 'apple-touch-icon.png'), pngIcon(180, SCALE_FULLBLEED));
writeFileSync(resolve(ICONS, 'icon-192.png'), pngIcon(192, SCALE_FULLBLEED));
writeFileSync(resolve(ICONS, 'icon-512.png'), pngIcon(512, SCALE_FULLBLEED));
writeFileSync(resolve(ICONS, 'icon-192-maskable.png'), pngIcon(192, SCALE_MASKABLE));
writeFileSync(resolve(ICONS, 'icon-512-maskable.png'), pngIcon(512, SCALE_MASKABLE));
// app-icon.png — the 1024 full-bleed master and the canonical Tauri icon
// source, written to the NATIVE icon dir (next to tauri.conf.json's icons/).
// Regenerate the native desktop/mobile icon set (icon.icns, icon.ico, the
// Windows Square tiles, StoreLogo, android/, ios/) from it with:
//   cargo tauri icon crates/lucidos-app/icons/app-icon.png --ios-color "#0a4ea8"
// then re-run THIS script: `cargo tauri icon` regenerates the Android adaptive
// foregrounds full-bleed at the source scale, so the maskable re-stamp below
// must land after it (re-running rewrites app-icon.png byte-identically).
mkdirSync(NATIVE_ICONS, { recursive: true });
writeFileSync(resolve(NATIVE_ICONS, 'app-icon.png'), pngIcon(1024, SCALE_FULLBLEED));
// `cargo tauri icon` (v2) no longer emits these two sizes, but the repo still
// tracks them — render them here so the native set never drifts to the old mark.
writeFileSync(resolve(NATIVE_ICONS, '256x256.png'), pngIcon(256, SCALE_FULLBLEED));
writeFileSync(resolve(NATIVE_ICONS, '512x512.png'), pngIcon(512, SCALE_FULLBLEED));

// Android adaptive foreground — the layer a circular launcher mask crops hardest.
// Re-stamp it at SCALE_MASKABLE in place (matching each density's pixel size read
// from the IHDR) so the mark keeps Android's safe-circle room. Skipped silently
// on a fresh checkout where `cargo tauri icon` hasn't been run yet.
let androidStamped = 0;
for (const density of ANDROID_DENSITIES) {
  const fg = resolve(NATIVE_ICONS, 'android', density, 'ic_launcher_foreground.png');
  if (!existsSync(fg)) continue;
  const size = readFileSync(fg).readUInt32BE(16); // PNG IHDR width (offset 16)
  writeFileSync(fg, pngIcon(size, SCALE_MASKABLE));
  androidStamped++;
}

console.log('Brand icons generated.');
console.log('  gradient :', GRAD_HIGHLIGHT, '→', GRAD_EDGE, '  theme-color:', THEME_COLOR);
console.log('  scales   : fullbleed', SCALE_FULLBLEED, ' maskable', SCALE_MASKABLE, ' favicon', SCALE_FAVICON);
console.log('  public/  : favicon.svg, favicon.ico, favicon.png, favicon-16/32/48.png');
console.log('  icons/   : icon-source.svg, apple-touch-icon.png, icon-192/512(.maskable).png, app-icon.png');
console.log(`  android  : re-stamped ${androidStamped} adaptive foreground(s) at maskable scale`);
console.log('  native   : run `cargo tauri icon crates/lucidos-app/icons/app-icon.png --ios-color "#0a4ea8"` then re-run this script');
