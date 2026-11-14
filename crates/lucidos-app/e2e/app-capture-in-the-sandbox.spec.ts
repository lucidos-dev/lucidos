import { readFileSync } from 'fs';
import { fileURLToPath } from 'url';
import type { Frame, Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';

// An app frame is sandboxed at an opaque origin (ADR 0227). The capture the
// agent reads runs inside it, and any child frame it made would get an opaque
// origin of its own. A capture that cannot paint falls back to the DOM
// snapshot, silently.
//
// This spec asks a frame the real shell built for a capture, the way the host
// does, and reads the picture back. Each check names a way it went wrong: no
// picture, a colour the rasterizer cannot paint, a font or an image the SVG
// snapshot could not load. See docs/plans/2026-10-08-app-capture-without-an-iframe.md.

const APP_ID = 'e2e-capture-app';

/** A CSS Color 4 colour, which a rasterizer must paint and not parse. */
const SWATCH = 'oklch(0.7 0.15 150)';

/** Narrow pixel glyphs, so a fallback face lands far from where VT323 does. */
const PROBE_TEXT = 'MMMMMMMMMMMM';

/** A few hundred styled nodes, so the deadline check is about a real app. */
const ROWS = Array.from({ length: 300 }, (_, i) =>
  `<li style="padding: ${i % 7}px; color: hsl(${(i * 37) % 360} 60% 40%)">Row ${i}</li>`).join('');

const LOGO = readFileSync(fileURLToPath(new URL('../public/favicon-48.png', import.meta.url)));

const FRAME_SELECTOR = `iframe[data-role="app-ui-frame"][src*="${APP_ID}"]`;

/** Open the fixture app in the real shell, and return its frame once it paints. */
async function openCaptureApp(page: Page): Promise<Frame> {
  await page.addInitScript((id) => {
    // An init script runs in EVERY frame, and the app frame's storage throws.
    if (window.parent !== window) return;
    localStorage.setItem('app-window-open', id);
  }, APP_ID);
  await gotoWithRetry(page, '/');

  const frameElement = page.locator(FRAME_SELECTOR).first();
  await expect(frameElement).toHaveCount(1, { timeout: 15_000 });
  expect(await frameElement.getAttribute('sandbox'), 'the frame must be opaque-origin').not.toContain(
    'allow-same-origin',
  );
  const frame = await (await frameElement.elementHandle())?.contentFrame();
  if (!frame) throw new Error('the app frame has no content');
  await expect(frame.locator('#probe')).toBeVisible({ timeout: 15_000 });
  return frame;
}

/** Ask the frame for a capture the way the host does: a host request it answers. */
function askCapture(page: Page, args: { format?: string }) {
  return page.evaluate(async ({ selector, args }) => {
    const iframe = document.querySelector(selector) as HTMLIFrameElement;
    const id = `e2e-capture-${performance.now()}`;
    const started = performance.now();
    const value = await new Promise<{ screenshot: string; dom: string }>((resolve, reject) => {
      const onMessage = (event: MessageEvent) => {
        const data = event.data as { type?: string; id?: string; ok?: boolean; value?: unknown; error?: string };
        if (data?.type !== 'lucidos:bridge:host:reply' || data.id !== id) return;
        removeEventListener('message', onMessage);
        if (data.ok) resolve(data.value as { screenshot: string; dom: string });
        else reject(new Error(data.error));
      };
      addEventListener('message', onMessage);
      iframe.contentWindow?.postMessage({ type: 'lucidos:bridge:host', id, op: 'capture', args }, '*');
    });
    return { ...value, ms: performance.now() - started };
  }, { selector: FRAME_SELECTOR, args });
}

/** Undefined until `beforeAll` builds it, so a `beforeAll` that throws part way
 *  still leaves the `afterAll` below able to run. */
let fixture: { dir: string; cleanup: () => void } | undefined;

test.describe('an app frame captures itself', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Capture app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>Capture app</title>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<link rel="stylesheet" href="/api/v1/fonts/vt323.css">
<script src="/api/v1/sdk.js"></script>
<style>
  html, body { margin: 0; background: #fff !important; }
  #swatch { position: absolute; left: 20px; top: 20px; width: 120px; height: 120px; background: ${SWATCH}; }
  #logo { position: absolute; left: 160px; top: 20px; width: 48px; height: 48px; }
  #probe { position: absolute; left: 20px; top: 180px; font: 48px/1 'VT323'; color: #000; white-space: nowrap; }
</style>
</head>
<body>
<div id="swatch"></div>
<img id="logo" src="logo.png" alt="">
<div id="probe">${PROBE_TEXT}</div>
<ul id="rows" style="position: absolute; top: 260px; margin: 0">${ROWS}</ul>
</body>
</html>
`,
      js: '',
      extra: { 'logo.png': LOGO },
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
  });

  test('the picture shows the colour, font and image the frame paints', async ({ page }) => {
    const frame = await openCaptureApp(page);

    const layout = await frame.evaluate(async (text) => {
      await document.fonts.load("48px 'VT323'");
      await (document.getElementById('logo') as HTMLImageElement).decode();
      const box = (id: string) => {
        const r = (document.getElementById(id) as HTMLElement).getBoundingClientRect();
        return { left: r.left, top: r.top, right: r.right, width: r.width, height: r.height };
      };
      const ctx = document.createElement('canvas').getContext('2d') as CanvasRenderingContext2D;
      ctx.font = '48px serif';
      return {
        viewportWidth: document.documentElement.clientWidth,
        vt323Loaded: document.fonts.check("48px 'VT323'"),
        fallbackWidth: ctx.measureText(text).width,
        swatch: box('swatch'),
        logo: box('logo'),
        probe: box('probe'),
      };
    }, PROBE_TEXT);
    expect(layout.vt323Loaded, 'the frame itself must have VT323, or the check proves nothing').toBe(true);
    expect(
      Math.abs(layout.fallbackWidth - layout.probe.width) / layout.probe.width,
      'VT323 and the fallback face must differ enough to tell apart',
    ).toBeGreaterThan(0.15);

    // Ask the way the host does: a host request the frame answers. The frame
    // reads its font and image over the real host bridge.
    const result = await askCapture(page, {});

    expect(result.dom, 'the capture fell back to the DOM snapshot').not.toMatch(/^\[screenshot unavailable/);
    expect(result.screenshot.length, 'the capture carries no picture').toBeGreaterThan(0);
    expect(result.ms, 'the host gives a capture 5 s').toBeLessThan(5_000);

    const picture = await page.evaluate(async ({ shot, swatchColour, viewportWidth, swatch, logo, probe }) => {
      const img = new Image();
      img.src = `data:image/jpeg;base64,${shot}`;
      await img.decode();
      const canvas = document.createElement('canvas');
      canvas.width = img.width;
      canvas.height = img.height;
      const ctx = canvas.getContext('2d') as CanvasRenderingContext2D;
      ctx.drawImage(img, 0, 0);
      const scale = img.width / viewportWidth;
      const at = (x: number, y: number) => Array.from(ctx.getImageData(Math.round(x * scale), Math.round(y * scale), 1, 1).data);
      /** The mean colour over a box, since one pixel of an icon may be white. */
      const mean = (box: typeof logo) => {
        const data = ctx.getImageData(
          Math.round(box.left * scale), Math.round(box.top * scale),
          Math.max(1, Math.round(box.width * scale)), Math.max(1, Math.round(box.height * scale)),
        ).data;
        const sum = [0, 0, 0];
        for (let i = 0; i < data.length; i += 4) for (let c = 0; c < 3; c++) sum[c] += data[i + c];
        return sum.map((total) => total / (data.length / 4));
      };

      const reference = document.createElement('canvas').getContext('2d') as CanvasRenderingContext2D;
      reference.fillStyle = swatchColour;
      reference.fillRect(0, 0, 1, 1);

      // The rightmost dark pixel on a row through the middle of the text.
      const row = ctx.getImageData(0, Math.round((probe.top + probe.height / 2) * scale), canvas.width, 1).data;
      let inkRight = -1;
      for (let x = 0; x < canvas.width; x++) if (row[x * 4] < 100) inkRight = x;

      return {
        swatch: at(swatch.left + swatch.width / 2, swatch.top + swatch.height / 2),
        swatchWanted: Array.from(reference.getImageData(0, 0, 1, 1).data),
        logo: mean(logo),
        inkRight: inkRight / scale,
      };
    }, {
      shot: result.screenshot,
      swatchColour: SWATCH,
      viewportWidth: layout.viewportWidth,
      swatch: layout.swatch,
      logo: layout.logo,
      probe: layout.probe,
    });

    for (let channel = 0; channel < 3; channel++) {
      expect(
        Math.abs(picture.swatch[channel] - picture.swatchWanted[channel]),
        `the ${SWATCH} swatch came out ${picture.swatch} instead of ${picture.swatchWanted}`,
      ).toBeLessThan(16);
    }
    expect(
      picture.logo.some((value) => value < 200),
      `the app's own image painted nothing, its box averages ${picture.logo}`,
    ).toBe(true);
    expect(
      Math.abs(picture.inkRight - layout.probe.right),
      `the text ends at ${picture.inkRight}, not at VT323's ${layout.probe.right}: the font was not embedded`,
    ).toBeLessThan(layout.probe.width * 0.06);
  });
  // A saved capture is a file for a plugin's media/ folder, so it must be
  // sharp: the device pixel ratio, in the format the engine asked for.
  test.describe('a saved capture', () => {
    test.use({ deviceScaleFactor: 2 });

    test('renders sharper than 1x, as a PNG', async ({ page }) => {
      const frame = await openCaptureApp(page);
      const viewportWidth = await frame.evaluate(() => document.documentElement.clientWidth);

      const result = await askCapture(page, { format: 'png' });
      expect(result.dom, 'the capture fell back to the DOM snapshot').not.toMatch(/^\[screenshot unavailable/);
      const bytes = Buffer.from(result.screenshot, 'base64');
      expect([...bytes.subarray(0, 8)], 'the picture is not a PNG').toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
      // IHDR holds the width at bytes 16 to 19. A wide viewport meets the
      // long-edge cap before 2x, so the check is "above 1x, at most 2x".
      const width = bytes.readUInt32BE(16);
      expect(width, 'the picture is still at 1x').toBeGreaterThan(viewportWidth);
      expect(width, 'the picture is past 2x').toBeLessThanOrEqual(viewportWidth * 2);
    });
  });
});
