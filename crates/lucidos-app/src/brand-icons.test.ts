// Guards the shape contract of the generated brand icons.
//
// macOS draws an app icon exactly as authored, so the macOS-facing assets carry
// Apple's rounded rect and its transparent margin. Android and iOS mask their
// own, so those stay full-bleed and opaque. Regenerate every asset with:
//   node crates/lucidos-app/scripts/generate-brand-icons.mjs
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { inflateSync } from 'node:zlib';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const asset = (rel: string): Uint8Array => readFileSync(resolve(__dirname, rel));

// Apple's template geometry. Mirrors MACOS_MARGIN and MACOS_CORNER in
// scripts/generate-brand-icons.mjs, which is the source of truth. Deliberately
// copied, not imported: a guard that reads the value it guards cannot fail.
const MACOS_MARGIN = 100 / 1024;
const MACOS_CORNER = 185.4 / 824;

// The icns member types the generator writes, paired with their pixel size.
// Two codes share a size: a retina variant and a larger @1x one.
const ICNS_MEMBERS: [string, number][] = [
  ['ic11', 32],
  ['ic12', 64],
  ['ic07', 128],
  ['ic13', 256],
  ['ic08', 256],
  ['ic14', 512],
  ['ic09', 512],
  ['ic10', 1024],
];

function u32(b: Uint8Array, at: number): number {
  return ((b[at] << 24) | (b[at + 1] << 16) | (b[at + 2] << 8) | b[at + 3]) >>> 0;
}

function ascii(b: Uint8Array, at: number, len: number): string {
  return String.fromCharCode(...b.subarray(at, at + len));
}

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

interface Png {
  width: number;
  height: number;
  channels: number;
  alphaAt(x: number, y: number): number;
}

// Decode enough of a PNG to read its alpha channel: colour type 2 (opaque RGB)
// or 6 (RGBA), 8-bit, non-interlaced, which is all the generator emits.
function decodePng(buf: Uint8Array): Png {
  const width = u32(buf, 16);
  const height = u32(buf, 20);
  const colorType = buf[25];
  expect([2, 6]).toContain(colorType);
  const channels = colorType === 6 ? 4 : 3;

  const idat: Uint8Array[] = [];
  let at = 8;
  while (at < buf.length) {
    const len = u32(buf, at);
    if (ascii(buf, at + 4, 4) === 'IDAT') idat.push(buf.subarray(at + 8, at + 8 + len));
    at += 12 + len;
  }
  const raw: Uint8Array = inflateSync(concat(idat));

  // Undo the per-scanline filter. Filter bytes 0 to 4 are none, sub, up,
  // average and Paeth. The generator writes 0, but decode all five so the guard
  // survives an encoder that starts filtering.
  const stride = width * channels;
  const rows: Uint8Array[] = [];
  let prev = new Uint8Array(stride);
  let p = 0;
  for (let y = 0; y < height; y++) {
    const filter = raw[p++];
    const line = raw.slice(p, p + stride);
    p += stride;
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? line[i - channels] : 0;
      const b = prev[i];
      const c = i >= channels ? prev[i - channels] : 0;
      if (filter === 1) line[i] = (line[i] + a) & 255;
      else if (filter === 2) line[i] = (line[i] + b) & 255;
      else if (filter === 3) line[i] = (line[i] + ((a + b) >> 1)) & 255;
      else if (filter === 4) {
        const pa = Math.abs(b - c);
        const pb = Math.abs(a - c);
        const pc = Math.abs(a + b - 2 * c);
        line[i] = (line[i] + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c)) & 255;
      }
    }
    rows.push(line);
    prev = line;
  }

  return {
    width,
    height,
    channels,
    alphaAt: (x, y) => (channels === 3 ? 255 : rows[y][x * 4 + 3]),
  };
}

function icnsMembers(buf: Uint8Array): { type: string; png: Uint8Array }[] {
  expect(ascii(buf, 0, 4)).toBe('icns');
  expect(u32(buf, 4)).toBe(buf.length);
  const out: { type: string; png: Uint8Array }[] = [];
  let at = 8;
  while (at < buf.length) {
    const len = u32(buf, at + 4);
    out.push({ type: ascii(buf, at, 4), png: buf.subarray(at + 8, at + len) });
    at += len;
  }
  return out;
}

// Apple's shape: a transparent margin all round, opaque art inside it.
function expectMacosShaped(png: Png): void {
  const margin = Math.round(png.width * MACOS_MARGIN);
  const mid = png.width >> 1;
  expect(png.channels).toBe(4);
  expect(png.alphaAt(0, 0)).toBe(0);
  expect(png.alphaAt(png.width - 1, 0)).toBe(0);
  expect(png.alphaAt(0, png.height - 1)).toBe(0);
  expect(png.alphaAt(png.width - 1, png.height - 1)).toBe(0);
  expect(png.alphaAt(mid, margin - 1)).toBe(0);
  expect(png.alphaAt(mid, margin)).toBe(255);
  expect(png.alphaAt(margin - 1, mid)).toBe(0);
  expect(png.alphaAt(margin, mid)).toBe(255);
  expect(png.alphaAt(mid, mid)).toBe(255);
  // The corner is cut, not merely inset. Without this pair a plain inset
  // square passes every assertion above. The art's own corner pixel sits
  // outside the arc, and the arc's centre sits inside it.
  const radius = Math.round((png.width - 2 * margin) * MACOS_CORNER);
  expect(png.alphaAt(margin, margin)).toBe(0);
  expect(png.alphaAt(margin + radius, margin + radius)).toBe(255);
}

// No alpha channel at all, so no pixel can be transparent and no margin exists.
function expectFullBleed(png: Png): void {
  expect(png.channels).toBe(3);
}

describe('brand icons', () => {
  describe('macOS-shaped, because nothing masks these for us', () => {
    it.each([
      ['icon-192.png', 192],
      ['icon-512.png', 512],
    ])('public/icons/%s carries the margin and rounded corners', (file, size) => {
      const png = decodePng(asset(`../public/icons/${file}`));
      expect(png.width).toBe(size);
      expectMacosShaped(png);
    });

    it('icons/icon.icns holds the expected members, every one shaped', () => {
      const members = icnsMembers(asset('../icons/icon.icns'));
      expect(members.map((m) => m.type)).toEqual(ICNS_MEMBERS.map(([type]) => type));
      members.forEach(({ png }, i) => {
        const decoded = decodePng(png);
        expect(decoded.width).toBe(ICNS_MEMBERS[i][1]);
        expectMacosShaped(decoded);
      });
    });
  });

  describe('full-bleed, because the platform masks these itself', () => {
    it.each([
      'apple-touch-icon.png', // iOS masks the home-screen icon
      'icon-192-maskable.png', // Android crops to its adaptive shape
      'icon-512-maskable.png',
    ])('public/icons/%s stays opaque corner to corner', (file) => {
      expectFullBleed(decodePng(asset(`../public/icons/${file}`)));
    });

    it.each([
      'app-icon.png', // the Tauri source for the non-macOS native set
      '256x256.png',
      '512x512.png',
    ])('icons/%s stays opaque corner to corner', (file) => {
      expectFullBleed(decodePng(asset(`../icons/${file}`)));
    });
  });
});
