import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type Page } from './fixtures';
import {
  apiRequest, ensureMobileView, gotoWithRetry, isMobileViewport, setDevicePreference, waitForPaneAtRest,
  waitForVisibleInput,
} from './helpers';

// Two theme fixes in a real browser, on desktop Chromium and mobile WebKit:
//
//   - a theme with all three radius tokens at 0 squares every corner, and the
//     default theme keeps its round ones;
//   - VT323 renders at Fira Code's x-height with its line box centred on the
//     glyphs, paints bold with its own strokes, and shows bold as
//     --text-strong.

const SQUARE = 'e2e-square-phosphor';
const here = dirname(fileURLToPath(import.meta.url));
const fixture = readFileSync(resolve(here, 'themes', 'square-phosphor.json'), 'utf-8');

/** The thread pane with its composer, a line of bold prose and a few chips.
 *  It brings the scroll buttons too, which the new-thread view never mounts. */
async function openScene(page: Page): Promise<void> {
  await gotoWithRetry(page, '/');
  await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
  if (isMobileViewport(page)) {
    await ensureMobileView(page, 'thread');
    await waitForPaneAtRest(page);
  }
  const composer = await waitForVisibleInput(page);
  await composer.fill('ls -la');
  await page.evaluate(() => {
    document.querySelector('#sq-root')?.remove();
    const host = ['.thread-content', '.thread-pane']
      .flatMap(s => [...document.querySelectorAll<HTMLElement>(s)])
      .find(el => el.getBoundingClientRect().width > 0);
    if (!host) throw new Error('no visible thread pane');
    const root = document.createElement('div');
    root.id = 'sq-root';
    root.innerHTML = `
      <div class="response-body"><div class="response-content"><div class="response-chunk markdown-content">
        <p id="sq-prose">Plain text and <strong id="sq-strong">bold text</strong>.</p>
      </div></div></div>
      <span class="pill-bar-btn active" id="sq-pill">pill</span>
      <button class="action-btn" id="sq-button">Archive</button>
      <div style="position: relative; height: 6.5rem">
        <button class="scroll-to-top visible" style="top: 0.5rem" aria-label="Scroll to top">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2"><path d="M6 15l6-6 6 6"/></svg>
        </button>
        <button class="scroll-to-bottom visible" style="bottom: 0.5rem" aria-label="Scroll to bottom">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2"><path d="M6 9l6 6 6-6"/></svg>
        </button>
      </div>`;
    host.prepend(root);
  });
}

const radius = (page: Page, selector: string) =>
  page.locator(selector).first().evaluate(el => getComputedStyle(el).borderTopLeftRadius);

/** The corners the phone screenshots showed round under a square theme. */
const CORNERS = ['.prompt-box', '.send-cancel-round', '.scroll-to-top', '#sq-pill', '#sq-button'];

test.describe('square corners and single-weight fonts', () => {
  test.beforeEach(async ({ page }) => {
    await gotoWithRetry(page, '/');
    const res = await apiRequest(page).put(`/api/v1/data/themes/${SQUARE}.json`, { data: fixture });
    expect(res.ok(), `saving ${SQUARE}: ${await res.text()}`).toBe(true);
  });

  test.afterEach(async ({ page }) => {
    await setDevicePreference(page, 'theme', 'lucidos');
    await apiRequest(page).delete(`/api/v1/data/themes/${SQUARE}.json`);
  });

  test('a theme with every radius at 0 squares every corner', async ({ page }) => {
    await setDevicePreference(page, 'theme', SQUARE);
    await openScene(page);
    for (const corner of CORNERS) await expect.poll(() => radius(page, corner), corner).toBe('0px');
  });

  test('the default theme keeps its round corners', async ({ page }) => {
    await openScene(page);
    for (const corner of CORNERS) expect(parseFloat(await radius(page, corner)), corner).toBeGreaterThan(0);
    expect(await radius(page, '.scroll-to-top')).toBe('999px');
  });

  test('VT323 renders at Fira Code\'s x-height, centred in its line box', async ({ page }) => {
    await openScene(page);
    const m = await page.evaluate(async () => {
      await Promise.all([document.fonts.load('100px VT323'), document.fonts.load('100px "Fira Code"')]);
      const ctx = document.createElement('canvas').getContext('2d')!;
      const read = (family: string) => {
        ctx.font = `100px ${family}`;
        const x = ctx.measureText('x');
        const ink = ctx.measureText('Hg');
        return {
          xHeight: x.actualBoundingBoxAscent,
          inkTop: ink.actualBoundingBoxAscent,
          inkBottom: ink.actualBoundingBoxDescent,
          boxTop: ink.fontBoundingBoxAscent,
          boxBottom: ink.fontBoundingBoxDescent,
        };
      };
      return { vt: read('VT323'), fira: read('"Fira Code"') };
    });
    expect(m.vt.xHeight / m.fira.xHeight).toBeGreaterThan(0.97);
    expect(m.vt.xHeight / m.fira.xHeight).toBeLessThan(1.03);
    // The caret spans the font box, so its centre sits on the ink's centre.
    const boxCentre = (m.vt.boxTop - m.vt.boxBottom) / 2;
    const inkCentre = (m.vt.inkTop - m.vt.inkBottom) / 2;
    expect(Math.abs(boxCentre - inkCentre), JSON.stringify(m.vt)).toBeLessThan(2);
  });

  test('VT323 paints bold with its own strokes, and shows it brighter', async ({ page }) => {
    await setDevicePreference(page, 'theme', SQUARE);
    await openScene(page);
    await expect(page.locator('html')).toHaveAttribute('data-font-bold', 'none');

    const pixels = await page.evaluate(async () => {
      await document.fonts.load('700 40px VT323');
      const draw = (weight: number) => {
        const canvas = document.createElement('canvas');
        canvas.width = 240;
        canvas.height = 60;
        const ctx = canvas.getContext('2d')!;
        ctx.font = `${weight} 40px VT323`;
        ctx.fillText('HHHH', 4, 44);
        return Array.from(ctx.getImageData(0, 0, 240, 60).data).join(',');
      };
      return { regular: draw(400), bold: draw(700) };
    });
    expect(pixels.bold === pixels.regular, 'bold paints the regular outlines').toBe(true);

    const colours = await page.evaluate(() => {
      // Any CSS colour, read back as a painted pixel's brightness.
      const brightness = (colour: string) => {
        const ctx = document.createElement('canvas').getContext('2d')!;
        ctx.fillStyle = colour;
        ctx.fillRect(0, 0, 1, 1);
        const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
        return r + g + b;
      };
      const strong = document.querySelector('#sq-strong')!;
      const prose = document.querySelector('#sq-prose')!;
      const probe = document.createElement('span');
      probe.style.color = 'var(--text-strong)';
      strong.appendChild(probe);
      const out = {
        strong: getComputedStyle(strong).color,
        textStrong: getComputedStyle(probe).color,
        strongBrightness: brightness(getComputedStyle(strong).color),
        proseBrightness: brightness(getComputedStyle(prose).color),
      };
      probe.remove();
      return out;
    });
    expect(colours.strong).toBe(colours.textStrong);
    // A dark page, so bold steps up toward white, in either theme.
    expect(colours.strongBrightness, JSON.stringify(colours)).toBeGreaterThan(colours.proseBrightness);
  });

  test('a font with a bold face keeps its bold, in the text colour', async ({ page }) => {
    await openScene(page);
    await expect(page.locator('html')).toHaveAttribute('data-font-bold', 'face');
    const same = await page.evaluate(() => {
      const probe = document.createElement('span');
      probe.style.color = 'var(--text-primary)';
      const strong = document.querySelector('#sq-strong')!;
      strong.appendChild(probe);
      const equal = getComputedStyle(strong).color === getComputedStyle(probe).color;
      probe.remove();
      return equal;
    });
    expect(same, 'strong keeps --text-primary').toBe(true);
  });
});
