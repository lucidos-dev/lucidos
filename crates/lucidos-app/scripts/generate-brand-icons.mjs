#!/usr/bin/env node

import { deflateSync } from 'node:zlib';
import { writeFileSync, readFileSync, existsSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const PUBLIC = resolve(__dirname, '..', 'public');
const ICONS = resolve(PUBLIC, 'icons');
const NATIVE_ICONS = resolve(__dirname, '..', 'icons');

const GRAD_HIGHLIGHT = '#2d83e0';
const GRAD_EDGE = '#0a4ea8';
const THEME_COLOR = '#0969da';
const GRAD_CX = 0.30,
  GRAD_CY = 0.22,
  GRAD_R = 1.25;

const TILES = [
  { x: 17, y: 17, w: 29, h: 29, r: 7 },
  { x: 17, y: 54, w: 29, h: 29, r: 7 },
  { x: 54, y: 54, w: 29, h: 29, r: 7 },
];
const SPARK_D =
  'M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 ' +
  'C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z';
const SPARK_BEZIERS = [
  [[68.5, 12], [71, 25], [74, 28.5], [87, 31]],
  [[87, 31], [74, 33.5], [71, 37], [68.5, 50]],
  [[68.5, 50], [66, 37], [63, 33.5], [50, 31]],
  [[50, 31], [63, 28.5], [66, 25], [68.5, 12]],
];

const SCALE_FULLBLEED = 0.74;
const SCALE_MASKABLE = 0.72;
const SCALE_FAVICON = 0.74;
const FAVICON_SVG_RADIUS = 0.22;

const MACOS_MARGIN = 100 / 1024;
const MACOS_CORNER = 185.4 / 824;

const ANDROID_DENSITIES = [
  'mipmap-mdpi',
  'mipmap-hdpi',
  'mipmap-xhdpi',
  'mipmap-xxhdpi',
  'mipmap-xxxhdpi',
];

function hexToRgb(hex) {
  const h = hex.replace('#', '');
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}
const HI = hexToRgb(GRAD_HIGHLIGHT);
const ED = hexToRgb(GRAD_EDGE);

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

function roundedRectSDF(px, py, X, Y, W, H, R) {
  const cx = X + W / 2,
    cy = Y + H / 2;
  const qx = Math.abs(px - cx) - (W / 2 - R);
  const qy = Math.abs(py - cy) - (H / 2 - R);
  const ax = Math.max(qx, 0),
    ay = Math.max(qy, 0);
  return Math.hypot(ax, ay) + Math.min(Math.max(qx, qy), 0) - R;
}

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

function inMark(px, py, size, scale) {
  const span = scale * size;
  const off = (size - span) / 2;
  const k = span / 100;
  const gx = (px - off) / k;
  const gy = (py - off) / k;
  if (gx < -2 || gx > 102 || gy < -2 || gy > 102) return false;
  for (const t of TILES) {
    if (roundedRectSDF(gx, gy, t.x, t.y, t.w, t.h, t.r) <= 0) return true;
  }
  return pointInPolygon(gx, gy, SPARK_POLY);
}

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
function encodePNG(size, pixels, channels = 3) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = channels === 4 ? 6 : 2;
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;
  const stride = size * channels;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0;
    pixels.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride);
  }
  const idat = deflateSync(raw, { level: 9 });
  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  return Buffer.concat([sig, chunk('IHDR', ihdr), chunk('IDAT', idat), chunk('IEND', Buffer.alloc(0))]);
}

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

function renderShapedRGBA(size, scale, ss = 4) {
  const margin = Math.round(size * MACOS_MARGIN);
  const art = size - 2 * margin;
  const rgb = renderRGB(art, scale, ss);
  const radius = art * MACOS_CORNER;
  const rgba = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let covered = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const px = x + (sx + 0.5) / ss;
          const py = y + (sy + 0.5) / ss;
          if (roundedRectSDF(px, py, margin, margin, art, art, radius) <= 0) covered++;
        }
      }
      if (!covered) continue;
      const ax = Math.min(art - 1, Math.max(0, x - margin));
      const ay = Math.min(art - 1, Math.max(0, y - margin));
      const src = (ay * art + ax) * 3;
      const dst = (y * size + x) * 4;
      rgba[dst] = rgb[src];
      rgba[dst + 1] = rgb[src + 1];
      rgba[dst + 2] = rgb[src + 2];
      rgba[dst + 3] = Math.round((255 * covered) / (ss * ss));
    }
  }
  return rgba;
}

function pngIconShaped(size, scale) {
  return encodePNG(size, renderShapedRGBA(size, scale), 4);
}

function buildICO(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  const dir = Buffer.alloc(16 * entries.length);
  let offset = 6 + dir.length;
  const blobs = [];
  entries.forEach((e, idx) => {
    const o = idx * 16;
    dir[o] = e.size >= 256 ? 0 : e.size;
    dir[o + 1] = e.size >= 256 ? 0 : e.size;
    dir[o + 2] = 0;
    dir[o + 3] = 0;
    dir.writeUInt16LE(1, o + 4);
    dir.writeUInt16LE(32, o + 6);
    dir.writeUInt32LE(e.png.length, o + 8);
    dir.writeUInt32LE(offset, o + 12);
    offset += e.png.length;
    blobs.push(e.png);
  });
  return Buffer.concat([header, dir, ...blobs]);
}

const ICNS_MEMBERS = [
  ['ic11', 32],
  ['ic12', 64],
  ['ic07', 128],
  ['ic13', 256],
  ['ic08', 256],
  ['ic14', 512],
  ['ic09', 512],
  ['ic10', 1024],
];
function buildICNS(pngBySize) {
  const members = ICNS_MEMBERS.map(([type, size]) => {
    const png = pngBySize.get(size);
    if (!png) throw new Error(`buildICNS: no ${size}px raster for member ${type}`);
    const head = Buffer.alloc(8);
    head.write(type, 0, 'ascii');
    head.writeUInt32BE(8 + png.length, 4);
    return Buffer.concat([head, png]);
  });
  const body = Buffer.concat(members);
  const header = Buffer.alloc(8);
  header.write('icns', 0, 'ascii');
  header.writeUInt32BE(8 + body.length, 4);
  return Buffer.concat([header, body]);
}

function markSVG(scale) {
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
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
${gradientDefSVG('lucidosBrand')}
  <rect x="0" y="0" width="100" height="100" fill="url(#lucidosBrand)"/>
${markSVG(SCALE_FULLBLEED)}
</svg>
`;
}

mkdirSync(ICONS, { recursive: true });

writeFileSync(resolve(PUBLIC, 'favicon.svg'), faviconSVG());
writeFileSync(resolve(ICONS, 'icon-source.svg'), masterSVG());

const fav16 = pngIcon(16, SCALE_FAVICON);
const fav32 = pngIcon(32, SCALE_FAVICON);
const fav48 = pngIcon(48, SCALE_FAVICON);
writeFileSync(resolve(PUBLIC, 'favicon-16.png'), fav16);
writeFileSync(resolve(PUBLIC, 'favicon-32.png'), fav32);
writeFileSync(resolve(PUBLIC, 'favicon-48.png'), fav48);
writeFileSync(resolve(PUBLIC, 'favicon.png'), fav32);
writeFileSync(resolve(PUBLIC, 'favicon.ico'), buildICO([
  { size: 16, png: fav16 },
  { size: 32, png: fav32 },
  { size: 48, png: fav48 },
]));

writeFileSync(resolve(ICONS, 'apple-touch-icon.png'), pngIcon(180, SCALE_FULLBLEED));
writeFileSync(resolve(ICONS, 'icon-192.png'), pngIconShaped(192, SCALE_FULLBLEED));
writeFileSync(resolve(ICONS, 'icon-512.png'), pngIconShaped(512, SCALE_FULLBLEED));
writeFileSync(resolve(ICONS, 'icon-192-maskable.png'), pngIcon(192, SCALE_MASKABLE));
writeFileSync(resolve(ICONS, 'icon-512-maskable.png'), pngIcon(512, SCALE_MASKABLE));
mkdirSync(NATIVE_ICONS, { recursive: true });
writeFileSync(resolve(NATIVE_ICONS, 'app-icon.png'), pngIcon(1024, SCALE_FULLBLEED));
const macShaped = new Map(
  [...new Set(ICNS_MEMBERS.map(([, size]) => size))].map((size) => [
    size,
    pngIconShaped(size, SCALE_FULLBLEED),
  ]),
);
writeFileSync(resolve(NATIVE_ICONS, 'icon.icns'), buildICNS(macShaped));
writeFileSync(resolve(NATIVE_ICONS, '256x256.png'), pngIcon(256, SCALE_FULLBLEED));
writeFileSync(resolve(NATIVE_ICONS, '512x512.png'), pngIcon(512, SCALE_FULLBLEED));

let androidStamped = 0;
for (const density of ANDROID_DENSITIES) {
  const fg = resolve(NATIVE_ICONS, 'android', density, 'ic_launcher_foreground.png');
  if (!existsSync(fg)) continue;
  const size = readFileSync(fg).readUInt32BE(16);
  writeFileSync(fg, pngIcon(size, SCALE_MASKABLE));
  androidStamped++;
}

console.log('Brand icons generated.');
console.log('  gradient :', GRAD_HIGHLIGHT, '→', GRAD_EDGE, '  theme-color:', THEME_COLOR);
console.log('  scales   : fullbleed', SCALE_FULLBLEED, ' maskable', SCALE_MASKABLE, ' favicon', SCALE_FAVICON);
console.log('  public/  : favicon.svg, favicon.ico, favicon.png, favicon-16/32/48.png');
console.log('  icons/   : icon-source.svg, apple-touch-icon.png, icon-192/512(.maskable).png, app-icon.png');
console.log('  macOS    : icon.icns + PWA "any" shaped at margin', MACOS_MARGIN.toFixed(5), ' corner', MACOS_CORNER.toFixed(4));
console.log(`  android  : re-stamped ${androidStamped} adaptive foreground(s) at maskable scale`);
console.log('  native   : run `cargo tauri icon crates/lucidos-app/icons/app-icon.png --ios-color "#0a4ea8"` then re-run this script');
