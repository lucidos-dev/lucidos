import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Offline rendering uses production styles and SVGs, without preference/SSE races.
const app = fileURLToPath(new URL('..', import.meta.url));
const css = ['global/shared-components.css', 'header.css', 'header-mark.css', 'panels/shell.css', 'badges.css']
  .map(path => readFileSync(resolve(app, 'src/styles', path), 'utf8')).join('\n');
const icons = readFileSync(resolve(app, 'src/components/shared/icons.tsx'), 'utf8');
const glyphs = ['ReloadIcon', 'HourglassIcon'].map(name => {
  const svg = icons.match(new RegExp(`export function ${name}\\(\\)[\\s\\S]*?(<svg[\\s\\S]*?</svg>)`))?.[1];
  if (!svg) throw new Error(`Missing ${name}`);
  return svg;
});

test.use({ deviceScaleFactor: 2, reducedMotion: 'no-preference' });

for (const trim of [true, false]) test(`icon squares and padded ink, text trim ${trim}`, async ({ page }) => {
  await page.setContent(`<style>${trim ? css : css.replace('@supports (text-box-trim: trim-both)', '@supports (unknown-property: 1)')}</style>
    <style>
      body { margin: 0; background: white; }
      [data-icon-probe] { position: fixed; left: 40px; top: 40px; width: 64px; height: 64px; }
      [data-icon-probe] .badge { background: black; color: red; box-shadow: none; }
    </style>
    <div class="app-header"><div class="brand-mark-slot" data-icon-probe></div></div>`);

  for (const scale of [75, 100, 125, 137.5, 150, 200]) {
    const measurements = await page.evaluate(({ scale, glyphs }) => {
      document.documentElement.style.fontSize = `${16 * scale / 100}px`;
      const host = document.querySelector<HTMLElement>('[data-icon-probe]')!;
      const samples = [];
      for (const [index, svg] of glyphs.entries()) {
        for (const font of ['Arial', 'Times New Roman', 'monospace']) {
          host.innerHTML = `<span class="badge brand-badge badge-icon" aria-hidden="true"
            style="font-family:${font}"><span class="brand-badge-${index ? 'queued' : 'spinner'}">${svg}</span></span>`;
          document.documentElement.dataset.motion = 'reduce';
          const badge = host.querySelector<HTMLElement>('.badge')!;
          const box = badge.getBoundingClientRect();
          const icon = badge.querySelector('svg')!.getBoundingClientRect();
          samples.push({ font, index, width: box.width, height: box.height,
            dx: icon.x + icon.width / 2 - box.x - box.width / 2,
            dy: icon.y + icon.height / 2 - box.y - box.height / 2,
            iconWidth: icon.width, iconHeight: icon.height,
            animation: getComputedStyle(badge.firstElementChild!).animationName });
        }
      }
      return samples;
    }, { scale, glyphs });
    expect(new Set(measurements.map(m => m.height)).size, `font-dependent box at ${scale}%`).toBe(1);
    for (const m of measurements) {
      // WebKit reports a square box as float32 rects, about 5e-6px apart.
      expect(Math.abs(m.width - m.height)).toBeLessThan(0.001);
      expect(Math.abs(m.iconWidth - m.iconHeight)).toBeLessThan(0.001);
      expect(Math.abs(m.dx)).toBeLessThan(0.02);
      expect(Math.abs(m.dy)).toBeLessThan(0.02);
      expect(m.animation).toBe('none');
    }
  }

  for (const scale of [100, 137.5]) for (const [index, svg] of glyphs.entries()) {
    for (const angle of index ? [0] : [0, 45, 90, 135]) for (const offset of [0, 0.25, 0.5, 0.75]) {
      const rotation = await page.evaluate(({ scale, svg, index, angle, offset }) => {
        document.documentElement.style.fontSize = `${16 * scale / 100}px`;
        document.documentElement.dataset.motion = 'full';
        const host = document.querySelector<HTMLElement>('[data-icon-probe]')!;
        host.style.left = `${40 + offset}px`;
        host.style.top = `${40 + offset}px`;
        host.innerHTML = `<span class="badge brand-badge badge-icon"><span
          class="brand-badge-${index ? 'queued' : 'spinner'}">${svg}</span></span>`;
        const spin = host.querySelector<HTMLElement>('.brand-badge-spinner');
        if (spin) {
          spin.style.animationDelay = `-${angle / 360}s`;
          spin.style.animationPlayState = 'paused';
          const matrix = new DOMMatrixReadOnly(getComputedStyle(spin).transform);
          return { a: matrix.a, b: matrix.b };
        }
        return null;
      }, { scale, svg, index, angle, offset });
      if (!index) {
        expect(rotation).not.toBeNull();
        expect(rotation!.a).toBeCloseTo(Math.cos(angle * Math.PI / 180), 4);
        expect(rotation!.b).toBeCloseTo(Math.sin(angle * Math.PI / 180), 4);
      }
      const png = await page.locator('[data-icon-probe]').screenshot({ animations: 'allow' });
      const ink = await page.evaluate(async b64 => {
        const img = new Image();
        img.src = `data:image/png;base64,${b64}`;
        await img.decode();
        const canvas = document.createElement('canvas');
        canvas.width = img.width;
        canvas.height = img.height;
        const ctx = canvas.getContext('2d')!;
        ctx.drawImage(img, 0, 0);
        const data = ctx.getImageData(0, 0, img.width, img.height).data;
        const pill = [Infinity, Infinity, -1, -1], ink = [...pill];
        const points: number[][] = [];
        for (let y = 0; y < img.height; y++) for (let x = 0; x < img.width; x++) {
          const i = (y * img.width + x) * 4;
          if (data[i + 1] > 50 || data[i + 2] > 50) continue;
          for (const bounds of data[i] > 128 ? [pill, ink] : [pill]) {
            bounds[0] = Math.min(bounds[0], x); bounds[1] = Math.min(bounds[1], y);
            bounds[2] = Math.max(bounds[2], x); bounds[3] = Math.max(bounds[3], y);
          }
          if (data[i] > 128) points.push([x, y]);
        }
        const cx = (pill[0] + pill[2]) / 2, cy = (pill[1] + pill[3]) / 2;
        const diameter = pill[2] - pill[0] + 1;
        return { pixels: points.length, dx: ((ink[0] + ink[2]) / 2 - cx) / 2,
          dy: ((ink[1] + ink[3]) / 2 - cy) / 2,
          clearance: (diameter / 2 - Math.max(...points.map(([x, y]) => Math.hypot(x - cx, y - cy)))) / 2,
          diameter: diameter / 2 };
      }, png.toString('base64'));
      const label = `${scale}% icon ${index} angle ${angle} offset ${offset}`;
      expect(ink.pixels, label).toBeGreaterThan(0);
      expect(Math.abs(ink.dx), label).toBeLessThanOrEqual(0.5);
      expect(Math.abs(ink.dy), label).toBeLessThanOrEqual(0.5);
      // Half a CSS pixel is the raster boundary's uncertainty at 2x.
      expect(ink.clearance, label).toBeGreaterThanOrEqual(ink.diameter * 0.075 - 0.5);
    }
  }
});
