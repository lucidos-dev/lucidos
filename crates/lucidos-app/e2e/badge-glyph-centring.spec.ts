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
 * is drawn at four sub-pixel offsets on both axes, pill black and glyph red.
 * The red is compared against the black, across and down. Runs on every
 * project, because WebKit rounds to device pixels and Chromium does not.
 */
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';
import { drawnInkShift, inkFont, MEASURE_PX } from '../src/utils/inkCentre';

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

/** Distance from the pill's middle to the ink's, in CSS px, by bounding box,
 *  across and down. The pill is every pixel that is black or red; the ink is
 *  the mostly-red ones. */
async function glyphOffset(
  page: import('@playwright/test').Page, png: Buffer, dsf = DSF,
): Promise<{ dx: number; dy: number }> {
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
    let pillLeft = Infinity, pillRight = -1, inkLeft = Infinity, inkRight = -1;
    for (let y = 0; y < img.height; y++) {
      for (let x = 0; x < img.width; x++) {
        const p = (y * img.width + x) * 4;
        if (d[p + 1] > 40 || d[p + 2] > 40) continue;
        pillTop = Math.min(pillTop, y);
        pillBottom = Math.max(pillBottom, y);
        pillLeft = Math.min(pillLeft, x);
        pillRight = Math.max(pillRight, x);
        if (d[p] > 128) {
          inkTop = Math.min(inkTop, y);
          inkBottom = Math.max(inkBottom, y);
          inkLeft = Math.min(inkLeft, x);
          inkRight = Math.max(inkRight, x);
        }
      }
    }
    return {
      dx: ((inkLeft + inkRight) / 2 - (pillLeft + pillRight) / 2) / dsf,
      dy: ((inkTop + inkBottom) / 2 - (pillTop + pillBottom) / 2) / dsf,
    };
  }, { b64: png.toString('base64'), dsf });
}

test.describe('Badge glyph centring', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('every badge draws its glyph in the middle, at every sub-pixel offset', async ({ page }) => {
    await navigateToApp(page);
    // A scale whose rem lands on fractions, which is where the bug lived.
    await page.evaluate(() => document.documentElement.style.setProperty('--user-ui-scale', '125%'));

    const found: Record<string, Array<{ dx: number; dy: number }>> = {};
    for (const [name, markup] of Object.entries(BADGES)) {
      for (const offset of OFFSETS) {
        const box = await page.evaluate(({ markup, offset, selector }) => {
          document.querySelector('[data-badge-probe]')?.remove();
          const host = document.createElement('div');
          host.dataset.badgeProbe = '';
          host.style.cssText = `position:fixed;left:${40 + offset}px;top:${40 + offset}px;`
            + 'z-index:2147483647;'
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

    for (const [name, samples] of Object.entries(found)) {
      for (const axis of ['dx', 'dy'] as const) {
        const offsets = samples.map(o => o[axis]);
        // Moving the badge must not move the glyph against its pill. One device
        // pixel of slack, for antialiasing on the ink's edge.
        expect(Math.max(...offsets) - Math.min(...offsets), `${name} ${axis} moved: ${offsets}`)
          .toBeLessThanOrEqual(0.25);
        // A quarter pixel is the design bound. Half a device pixel more is the
        // measurement's own resolution.
        for (const d of offsets) {
          expect(Math.abs(d), `${name} ${axis} off centre: ${offsets}`).toBeLessThanOrEqual(0.375);
        }
      }
    }
  });
});

/** Where each header badge rides, as its real host renders it. */
const HEADER_HOSTS: Record<string, { host: string; badge: string }> = {
  'unread count on the mark': { host: '.brand-mark-slot', badge: 'badge brand-unread-badge' },
  'bell count': { host: '.notifications-bell', badge: 'badge' },
};

/** Painted rows of the pill, and of the ink above and below the glyph. */
async function paintedRows(page: import('@playwright/test').Page, png: Buffer) {
  return page.evaluate(async (b64) => {
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
        if (d[p + 1] > 60 || d[p + 2] > 60) continue;
        pillTop = Math.min(pillTop, y);
        pillBottom = Math.max(pillBottom, y);
        if (d[p] > 128) {
          inkTop = Math.min(inkTop, y);
          inkBottom = Math.max(inkBottom, y);
        }
      }
    }
    return { pill: pillBottom - pillTop + 1, above: inkTop - pillTop, below: pillBottom - inkBottom };
  }, png.toString('base64'));
}

/**
 * The same badges inside the header, on a 1x screen.
 *
 * Each header region is centred with a `-50%` translate, which is -24.75px at
 * 137.5%. Unrounded, Chrome snaps the pill and the digit apart inside it: the
 * pill loses its bottom row and the count sits low. Only a 1x screen shows it,
 * so the probe above, at 4x on a plain host, cannot.
 */
test.describe('Header badge centring on a 1x screen', () => {
  test.use({ deviceScaleFactor: 1 });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a header badge paints its whole pill, with its glyph in the middle', async ({ page }) => {
    await navigateToApp(page);
    // The splash fades out over the header, and a screenshot would read it.
    await expect(page.locator('.boot-splash')).toHaveCount(0);
    const failures: string[] = [];
    for (let scale = 100; scale <= 200; scale += 12.5) {
      for (const [name, { host, badge }] of Object.entries(HEADER_HOSTS)) {
        await page.evaluate(({ scale, host, badge }) => {
          document.documentElement.style.setProperty('--user-ui-scale', `${scale}%`);
          document.querySelector('[data-badge-probe]')?.remove();
          const slot = [...document.querySelectorAll<HTMLElement>(host)]
            .find(e => e.getBoundingClientRect().width > 0)!;
          const probe = document.createElement('span');
          probe.className = badge;
          probe.dataset.badgeProbe = '';
          probe.textContent = '0';
          slot.appendChild(probe);
          // A white ring keeps the black pill apart from whatever it rides.
          probe.style.cssText = 'background:#000!important;color:#f00!important;'
            + 'box-shadow:0 0 0 3px #fff!important;border-radius:0!important';
        }, { scale, host, badge });
        // The app writes the scale itself, and a late preference load would
        // take it back and leave every pass measuring 100%.
        await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize))
          .toBe(`${(16 * scale) / 100}px`);
        // The workspace name folds away at some scales, which moves the mark.
        await page.waitForTimeout(200);
        const box = await page.locator('[data-badge-probe]').boundingBox();
        const png = await page.screenshot({
          clip: { x: box!.x - 2, y: box!.y - 2, width: box!.width + 4, height: box!.height + 4 },
        });
        const rows = await paintedRows(page, png);
        // styles/badges.css makes the height a whole pixel, so all of it paints.
        // The glyph's two gaps may differ by the one row an odd split leaves.
        if (rows.pill !== box!.height || Math.abs(rows.above - rows.below) > 1) {
          failures.push(`${name} at ${scale}%: box ${box!.height}px, painted `
            + `${rows.pill}px, ${rows.above} above and ${rows.below} below the glyph`);
        }
      }
    }
    expect(failures).toEqual([]);
  });
});

/** The badges on the phone's thread header, each in the host that wears it. */
const PHONE_HOSTS: Record<string, { host: string; badge: string }> = {
  'thread toggle count': { host: '.mobile-thread-header .thread-toggle', badge: 'badge' },
  'unread count on the mark': {
    host: '.mobile-thread-header .brand-mark-slot', badge: 'badge brand-unread-badge',
  },
  'menu count': { host: '.mobile-thread-header .hamburger-panel', badge: 'badge' },
};

/** Digits that Fira Code draws off their own advance ("3" most of all), one
 *  and two wide. A narrow pill and a content-sized one round differently. */
const PHONE_GLYPHS = ['1', '2', '3', '4', '5', '38'];

/** The probe is a bare span, so it gets the shift GlyphBadge would state:
 *  the app's own `drawnInkShift`, run in the page on the probe's own font. */
async function stateInkShift(page: import('@playwright/test').Page): Promise<void> {
  const { text, style } = await page.evaluate(() => {
    const probe = document.querySelector<HTMLElement>('[data-badge-probe]')!;
    const { fontStyle, fontWeight, fontFamily } = getComputedStyle(probe);
    return { text: probe.textContent ?? '', style: { fontStyle, fontWeight, fontFamily } };
  });
  const shift = (await page.evaluate(drawnInkShift, { text, font: inkFont(style), px: MEASURE_PX })) ?? 0;
  await page.evaluate((shift) => {
    document.querySelector<HTMLElement>('[data-badge-probe]')!
      .style.setProperty('--badge-ink-shift', `${shift.toFixed(4)}em`);
  }, shift);
}

/**
 * The phone header's badges at every UI scale, as a phone draws them.
 *
 * Each scale puts the pill and the glyph on a different fraction of a pixel.
 * The probe above holds one scale on a plain host; this walks them all, in the
 * real header, at the 3x an iPhone paints at. On an iPhone, the mark's "3" at
 * 200% sat two device pixels left of its pill's middle. The font draws it left
 * of its slot.
 */
test.describe('Phone header badges at every UI scale', () => {
  const dsf = 3;
  test.use({ viewport: { width: 390, height: 844 }, deviceScaleFactor: dsf, hasTouch: true });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('each badge draws its glyph in the middle of its pill', async ({ page, browserName }) => {
    await navigateToApp(page);
    await expect(page.locator('.boot-splash')).toHaveCount(0);
    // A quarter pixel is the design bound, plus the measurement's half a device pixel.
    const bound = 0.25 + 0.5 / dsf;
    // WebKit steps a glyph in whole device pixels from its own origin. So a
    // pill at a fractional device-pixel x can land its digit half a device
    // pixel further off. The header's rem geometry puts it there at some
    // scales (ADR 0361).
    const fractionalBound = bound + (browserName === 'webkit' ? 0.5 / dsf : 0);
    const failures: string[] = [];
    for (let scale = 100; scale <= 200; scale += 12.5) {
      await page.evaluate((scale) => {
        document.documentElement.style.setProperty('--user-ui-scale', `${scale}%`);
      }, scale);
      await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize))
        .toBe(`${(16 * scale) / 100}px`);
      await page.waitForTimeout(200);
      for (const [name, { host, badge }] of Object.entries(PHONE_HOSTS)) {
        for (const glyph of PHONE_GLYPHS) {
          await page.evaluate(({ host, badge, glyph }) => {
            document.querySelector('[data-badge-probe]')?.remove();
            const slot = [...document.querySelectorAll<HTMLElement>(host)]
              .find(e => e.getBoundingClientRect().width > 0)!;
            const probe = document.createElement('span');
            probe.className = badge;
            probe.dataset.badgeProbe = '';
            // GlyphBadge's markup: the text in the span its shift moves.
            const ink = document.createElement('span');
            ink.className = 'badge-ink';
            ink.textContent = glyph;
            probe.appendChild(ink);
            slot.appendChild(probe);
            // A white ring keeps the black pill apart from whatever it rides.
            probe.style.cssText = 'background:#000!important;color:#f00!important;'
              + 'box-shadow:0 0 0 3px #fff!important;border-radius:0!important';
          }, { host, badge, glyph });
          await stateInkShift(page);
          const box = (await page.locator('[data-badge-probe]').boundingBox())!;
          const png = await page.screenshot({
            clip: { x: box.x - 2, y: box.y - 2, width: box.width + 4, height: box.height + 4 },
          });
          const { dx, dy } = await glyphOffset(page, png, dsf);
          const fractional = Math.abs(box.x * dsf - Math.round(box.x * dsf)) > 0.01;
          if (Math.abs(dx) > (fractional ? fractionalBound : bound) || Math.abs(dy) > bound) {
            failures.push(`${name} "${glyph}" at ${scale}%: ${dx.toFixed(2)}px across, `
              + `${dy.toFixed(2)}px down, in a ${box.width.toFixed(2)}x${box.height.toFixed(2)} pill `
              + `at x ${box.x.toFixed(3)}, ${await page.locator('[data-badge-probe]').evaluate((e) => {
                const ink = e.firstElementChild!.getBoundingClientRect();
                const cs = getComputedStyle(e);
                return `shift ${e.style.getPropertyValue('--badge-ink-shift')}, ink ${ink.left.toFixed(3)}`
                  + `+${ink.width.toFixed(3)}, font ${cs.fontSize} ${cs.fontFamily.slice(0, 20)}, `
                  + `pad ${cs.paddingLeft}/${cs.paddingRight}, features ${cs.fontFeatureSettings}, layers `
                  + [...(function* up(n: Element | null) { while (n) { yield n; n = n.parentElement; } })(e.parentElement)]
                    .filter(n => { const s = getComputedStyle(n); return s.transform !== 'none' || s.willChange !== 'auto'; })
                    .map(n => `${n.className.toString().split(' ')[0]}@${n.getBoundingClientRect().left.toFixed(3)}`
                      + `[${getComputedStyle(n).transform}|${getComputedStyle(n).willChange}]`).join(' ');
              })}`);
          }
        }
      }
    }
    expect(failures).toEqual([]);
  });
});
