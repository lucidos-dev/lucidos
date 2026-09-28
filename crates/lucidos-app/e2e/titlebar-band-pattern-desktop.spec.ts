import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type Page } from './fixtures';
import { apiRequest, gotoWithRetry, waitForVisibleInput } from './helpers';

/**
 * A patterned header fill carries on up through the macOS title-bar band.
 *
 * Setting `--header-gradient` derives `--titlebar-strip-continues`, and the
 * header's fill then paints as one box from the band's top edge.
 *
 * Pinned here: the striped rows keep their period across the join, and the
 * default theme and two built-ins paint exactly as the header's own background
 * did. `mono` sets a flat `--header-gradient`, so it takes the continuing path
 * and must still not move. This spec stamps the build rather than running it
 * (ADR 0016).
 */

test.use({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });

type Rgb = [number, number, number];

const STRIPED = 'e2e-striped-header';
const here = dirname(fileURLToPath(import.meta.url));
const fixture = readFileSync(resolve(here, 'themes', 'striped-header.json'), 'utf-8');

const BLACK: Rgb = [0, 0, 0];
const GREEN: Rgb = [0, 255, 0];
/** The fixture's period: two black rows, then one green. */
const stripeAt = (y: number): Rgb => (y % 3 < 2 ? BLACK : GREEN);
/** Per-channel slack for gradient rasterisation. */
const EPS = 8;
/** The themes whose own `--header-gradient` derives the continuing band. */
const CONTINUES = new Set([STRIPED, 'mono']);

/** The header painting its own fill, with nothing from the band. Injected
 *  over the live sheet, it is the control. */
const RULES_BEFORE = `
  .titlebar-strip::before { content: none !important; }
  .app-header { background: var(--header-gradient) !important; }
`;

async function deviceId(page: Page): Promise<string> {
  let id: string | null = null;
  await expect.poll(async () => {
    id = await page.evaluate(() => localStorage.getItem('lucidos-device-id')).catch(() => null);
    return id;
  }, { message: 'the shell registered a device' }).toBeTruthy();
  return id as unknown as string;
}

async function setTheme(page: Page, theme: string): Promise<void> {
  const device = await deviceId(page);
  const res = await apiRequest(page).put('/api/v1/preferences?key=theme', { data: { value: theme, device_id: device } });
  expect(res.ok(), `theme=${theme}`).toBe(true);
}

/** Set the theme and wait until its band switch is on the document. */
async function applyTheme(page: Page, theme: string): Promise<void> {
  await setTheme(page, theme);
  await expect.poll(
    () => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--titlebar-strip-continues').trim()),
    { message: `${theme} reached the document` },
  ).toBe(CONTINUES.has(theme) ? '1' : '0');
}

async function stampOverlayBuild(page: Page): Promise<void> {
  await page.evaluate(() => {
    document.documentElement.style.setProperty('--titlebar-inset', '28px');
    document.documentElement.setAttribute('data-titlebar-overlay', '');
  });
}

/** Hide everything drawn ON the bar, so only its fill is left to read. */
async function bareBar(page: Page): Promise<void> {
  await page.addStyleTag({ content: '.app-header * { visibility: hidden !important; }' });
}

/** The bar's bottom edge: the band plus the header. */
async function barHeight(page: Page): Promise<number> {
  return page.evaluate(() => Math.round(document.querySelector('.app-header')!.getBoundingClientRect().bottom));
}

/** One column of the window's top, decoded in the page, since the suite ships
 *  no PNG decoder. */
async function pixelColumn(page: Page, x: number, height: number): Promise<Rgb[]> {
  const png = await settledShot(page, { x, y: 0, width: 1, height });
  return page.evaluate(async (b64) => {
    const img = new Image();
    img.src = `data:image/png;base64,${b64}`;
    await img.decode();
    const canvas = document.createElement('canvas');
    canvas.width = img.width;
    canvas.height = img.height;
    const ctx = canvas.getContext('2d')!;
    ctx.drawImage(img, 0, 0);
    const data = ctx.getImageData(0, 0, img.width, img.height).data;
    const rows: [number, number, number][] = [];
    for (let y = 0; y < img.height; y++) rows.push([data[y * 4], data[y * 4 + 1], data[y * 4 + 2]]);
    return rows;
  }, png.toString('base64'));
}

const close = (a: Rgb, b: Rgb) => a.every((c, i) => Math.abs(c - b[i]) <= EPS);

type Clip = { x: number; y: number; width: number; height: number };

/** A screenshot once two in a row agree, so a theme's colour transitions have
 *  finished before anything is compared. */
async function settledShot(page: Page, clip: Clip): Promise<Buffer> {
  let last = await page.screenshot({ clip });
  for (let i = 0; i < 20; i++) {
    await page.waitForTimeout(100);
    const next = await page.screenshot({ clip });
    if (next.equals(last)) return next;
    last = next;
  }
  throw new Error('the bar never stopped changing');
}

/** 'none' for identical pixels, else how many differ, where, and by how much. */
async function pixelDiff(page: Page, a: Buffer, b: Buffer): Promise<string> {
  return page.evaluate(async ([pa, pb]) => {
    const decode = async (b64: string) => {
      const img = new Image();
      img.src = `data:image/png;base64,${b64}`;
      await img.decode();
      const canvas = document.createElement('canvas');
      canvas.width = img.width;
      canvas.height = img.height;
      const ctx = canvas.getContext('2d')!;
      ctx.drawImage(img, 0, 0);
      return { w: img.width, data: ctx.getImageData(0, 0, img.width, img.height).data };
    };
    const [x, y] = [await decode(pa), await decode(pb)];
    let count = 0;
    let delta = 0;
    const rows = new Set<number>();
    for (let i = 0; i < x.data.length; i += 4) {
      const d = Math.max(...[0, 1, 2].map(c => Math.abs(x.data[i + c] - y.data[i + c])));
      if (d === 0) continue;
      count++;
      delta = Math.max(delta, d);
      rows.add(Math.floor(i / 4 / x.w));
    }
    return count === 0 ? 'none' : `${count} pixels differ, by up to ${delta}, on rows ${[...rows].slice(0, 12)}`;
  }, [a.toString('base64'), b.toString('base64')]);
}

test.describe('the title-bar band continues the header fill', () => {
  test.beforeEach(async ({ page }) => {
    await gotoWithRetry(page, '/');
    const res = await apiRequest(page).put(`/api/v1/data/themes/${STRIPED}.json`, { data: fixture });
    expect(res.ok(), `saving ${STRIPED}: ${await res.text()}`).toBe(true);
    await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
    await waitForVisibleInput(page);
  });

  test.afterEach(async ({ page }) => {
    await setTheme(page, 'lucidos');
    await apiRequest(page).delete(`/api/v1/data/themes/${STRIPED}.json`);
  });

  test('a striped header keeps its period from the window top, across the join', async ({ page }) => {
    await applyTheme(page, STRIPED);
    await stampOverlayBuild(page);
    await bareBar(page);
    const bar = await barHeight(page);
    expect(bar, 'the bar is the 28px band plus the header').toBeGreaterThan(28);

    for (const x of [3, 640, 1270]) {
      const wrong = (await pixelColumn(page, x, bar))
        .map((rgb, y) => ({ y, rgb, want: stripeAt(y) }))
        .filter(r => !close(r.rgb, r.want))
        .map(r => `y=${r.y} ${r.y < 28 ? 'band' : 'header'}: ${r.rgb} want ${r.want}`);
      expect(wrong, `x=${x}: rows off the stripe period`).toEqual([]);
    }
  });

  test('a header fill with its own tile size and a colour layer keeps both', async ({ page }) => {
    await applyTheme(page, 'lucidos');
    await page.evaluate(() => document.documentElement.style.setProperty(
      '--header-gradient', 'radial-gradient(#ffffff 1px, transparent 1px) 0 0 / 8px 8px, #0b3a66',
    ));
    await bareBar(page);
    const clip = { x: 0, y: 0, width: 1280, height: await barHeight(page) };
    const now = await settledShot(page, clip);
    await page.addStyleTag({ content: RULES_BEFORE });
    expect(await pixelDiff(page, now, await settledShot(page, clip)), 'the dotted bar moved').toBe('none');
  });

  for (const theme of ['lucidos', 'gruvbox', 'mono']) {
    for (const build of ['macOS', 'web'] as const) {
      test(`${theme} on the ${build} build paints the band and header exactly as before`, async ({ page }) => {
        await applyTheme(page, theme);
        if (build === 'macOS') await stampOverlayBuild(page);
        await bareBar(page);
        const clip = { x: 0, y: 0, width: 1280, height: await barHeight(page) };
        const now = await settledShot(page, clip);
        await page.addStyleTag({ content: RULES_BEFORE });
        const before = await settledShot(page, clip);
        expect(await pixelDiff(page, now, before), `${theme}: the bar moved`).toBe('none');
      });
    }
  }
});
