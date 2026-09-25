/**
 * A badge's glyph sits in the middle of its pill, wherever the badge lands.
 *
 * Chrome paints a baseline on a whole CSS pixel, and a pill's edges too, each
 * rounding on its own. So a badge moved by a fraction of a pixel could jump a
 * pixel against its pill, and the mark's unread count sat visibly low.
 * `styles/badges.css` puts both on whole pixels so they round together.
 *
 * `styles/__tests__/badge-glyph-centring.test.ts` pins the shape of that rule.
 * Only a browser can say what it paints, so this measures the ink. Each badge
 * is drawn at four sub-pixel offsets, pill black and glyph red, and the red is
 * compared against the black. Runs on every project, because WebKit rounds to
 * device pixels and Chromium does not.
 */
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

// Fine enough to see a quarter of a CSS pixel.
test.use({ viewport: { width: 1280, height: 900 }, deviceScaleFactor: 4 });

/** The markup each badge renders with, minus everything that is not its box.
 *  A symmetric glyph, so the ink's own middle is its geometric one. */
const BADGES: Record<string, string> = {
  'header count': '<span class="badge" style="position:static">0</span>',
  'unread count on the mark':
    '<span class="brand-mark-slot">'
    + '<span style="display:inline-block;width:2.1rem;height:2.1rem"></span>'
    + '<span class="badge brand-unread-badge">0</span></span>',
  'drawer attention count': '<span class="badge drawer-view-count">0</span>',
  'section count': '<span class="section-count-badge">0</span>',
  'menu drawer count': '<span class="drawer-badge">0</span>',
  'workspace menu count':
    '<span style="display:flex;line-height:1.4"><span class="brand-menu-ws-badge">0</span></span>',
  'picker count': '<span class="ws-picker-badge">0</span>',
  'question mark': '<span class="thread-status-question-badge"></span>',
};

const BADGE_SELECTOR = '.badge, .section-count-badge, .drawer-badge, .brand-menu-ws-badge, '
  + '.ws-picker-badge, .thread-status-question-badge';

const OFFSETS = [0, 0.25, 0.5, 0.75];

const DSF = 4;

/** Distance from the pill's middle to the ink's, in CSS px, by bounding box.
 *  The pill is every pixel that is black or red; the ink is the mostly-red ones. */
async function glyphOffset(page: import('@playwright/test').Page, png: Buffer): Promise<number> {
  return page.evaluate(async ({ b64, dsf }) => {
    const img = new Image();
    img.src = `data:image/png;base64,${b64}`;
    await img.decode();
    const canvas = document.createElement('canvas');
    canvas.width = img.width;
    canvas.height = img.height;
    const ctx = canvas.getContext('2d')!;
    ctx.drawImage(img, 0, 0);
    const d = ctx.getImageData(0, 0, img.width, img.height).data;
    let pillTop = Infinity, pillBottom = -1, inkTop = Infinity, inkBottom = -1;
    for (let y = 0; y < img.height; y++) {
      for (let x = 0; x < img.width; x++) {
        const p = (y * img.width + x) * 4;
        if (d[p + 1] > 40 || d[p + 2] > 40) continue;
        pillTop = Math.min(pillTop, y);
        pillBottom = Math.max(pillBottom, y);
        if (d[p] > 128) {
          inkTop = Math.min(inkTop, y);
          inkBottom = Math.max(inkBottom, y);
        }
      }
    }
    return ((inkTop + inkBottom) / 2 - (pillTop + pillBottom) / 2) / dsf;
  }, { b64: png.toString('base64'), dsf: DSF });
}

test.describe('Badge glyph centring', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('every badge draws its glyph in the middle, at every sub-pixel offset', async ({ page }) => {
    await navigateToApp(page);
    // A scale whose rem lands on fractions, which is where the bug lived.
    await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '125%'));

    const found: Record<string, number[]> = {};
    for (const [name, markup] of Object.entries(BADGES)) {
      for (const offset of OFFSETS) {
        const box = await page.evaluate(({ markup, offset, selector }) => {
          document.querySelector('[data-badge-probe]')?.remove();
          const host = document.createElement('div');
          host.dataset.badgeProbe = '';
          host.style.cssText = `position:fixed;left:40px;top:${40 + offset}px;z-index:2147483647;`
            + 'display:inline-flex;padding:4px;background:#fff';
          host.innerHTML = markup
            // The "?" is drawn by a pseudo-element, which inline style cannot reach.
            + '<style>[data-badge-probe] .thread-status-question-badge::after{color:#f00}</style>';
          document.body.appendChild(host);
          const badge = host.querySelector<HTMLElement>(selector)!;
          badge.style.cssText += ';background:#000;color:#f00;border-radius:0;box-shadow:none';
          const r = badge.getBoundingClientRect();
          return { x: r.x, y: r.y, width: r.width, height: r.height };
        }, { markup, offset, selector: BADGE_SELECTOR });
        const png = await page.screenshot({
          clip: { x: box.x - 2, y: box.y - 2, width: box.width + 4, height: box.height + 4 },
        });
        (found[name] ??= []).push(await glyphOffset(page, png));
      }
    }

    for (const [name, offsets] of Object.entries(found)) {
      // Moving the badge must not move the glyph against its pill. One device
      // pixel of slack, for antialiasing on the ink's edge.
      expect(Math.max(...offsets) - Math.min(...offsets), `${name} moved: ${offsets}`)
        .toBeLessThanOrEqual(0.25);
      // A quarter pixel is the design bound. Half a device pixel more is the
      // measurement's own resolution.
      for (const dy of offsets) {
        expect(Math.abs(dy), `${name} off centre: ${offsets}`).toBeLessThanOrEqual(0.375);
      }
    }
  });
});
