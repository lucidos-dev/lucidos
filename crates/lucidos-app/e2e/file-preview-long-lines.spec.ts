import { test, expect, type Locator, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import {
  clickHeaderAction, clickVisibleElement, ensureOnThreadPane, gotoWithRetry,
  openFilesPanel, waitForVisibleElement, waitForVisibleInput,
} from './helpers';

/** A source line wider than the viewer used to be clipped, with no wrap and no
 *  visible way to reach its tail. It bit hardest exactly where the viewer
 *  matters most: an app deep-links into a cited line, and the part the citation
 *  was about is off the right edge with nothing saying it exists.
 *
 *  So every case here asks the same question of the rendered box: can the
 *  reader SEE the end of the line. It is measured with a Range over the last
 *  character, never inferred from a scrollWidth. "There is a scroll region
 *  somewhere" is what the bug already had. */

const APP_ID = 'e2e-long-line-preview';
const APP_DIR = resolve(WORKSPACE, 'data/apps', APP_ID);
const SAMPLE_NAME = 'e2e-long-line-sample.txt';
const SAMPLE_PATH = `artifacts/${SAMPLE_NAME}`;
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);

/** The line the citation points at, and the marker at its far end. */
const LONG_LINE = 3;
const TAIL = 'THE-TAIL-THE-CITATION-MEANT';
const LONG = `const wide = ${Array.from({ length: 90 }, (_, i) => `segment_${i}`).join('_')}_${TAIL};`;

/** Geometry of one rendered row, read after panning its scroll container as far
 *  right as it goes. `tailInView` is the whole question: is the last character
 *  of the line inside the box the reader is looking at. */
interface RowProbe {
  scrollWidth: number;
  clientWidth: number;
  rowWidth: number;
  /** The `<pre>`'s own width, which in pan mode is its widest row. Compared
   *  against `rowWidth` rather than against `scrollWidth`, whose extra is the
   *  scroll container's padding. */
  blockWidth: number;
  rowHeight: number;
  gutters: number;
  gutterShift: number;
  tailInViewUnscrolled: boolean;
  tailInViewScrolled: boolean;
  scrolledBy: number;
}

function probeRow(row: Locator): Promise<RowProbe> {
  return row.evaluate((el: HTMLElement) => {
    const scroller = el.closest('.file-preview-content, .repo-file-content') as HTMLElement;
    const gutter = el.querySelector('.line-number') as HTMLElement;
    const content = el.querySelector('.line-content') as HTMLElement;

    // The last character's own box. A Range is the only thing that answers
    // where the END of a wrapped or panned line actually landed.
    const walk = document.createTreeWalker(content, NodeFilter.SHOW_TEXT);
    let last: Text | null = null;
    for (let n = walk.nextNode(); n; n = walk.nextNode()) last = n as Text;
    const tailRect = () => {
      const r = document.createRange();
      r.setStart(last!, Math.max(0, last!.length - 1));
      r.setEnd(last!, last!.length);
      return r.getBoundingClientRect();
    };
    const within = (r: DOMRect) => {
      const box = scroller.getBoundingClientRect();
      return r.right <= box.right + 1 && r.left >= box.left - 1;
    };

    scroller.scrollLeft = 0;
    const tailInViewUnscrolled = within(tailRect());
    const rect = el.getBoundingClientRect();
    const gutterBefore = gutter.getBoundingClientRect().left;

    scroller.scrollLeft = scroller.scrollWidth;
    const scrolledBy = scroller.scrollLeft;
    const tailInViewScrolled = within(tailRect());
    const gutterShift = Math.abs(gutter.getBoundingClientRect().left - gutterBefore);

    scroller.scrollLeft = 0;
    return {
      scrollWidth: scroller.scrollWidth,
      clientWidth: scroller.clientWidth,
      rowWidth: rect.width,
      blockWidth: (el.closest('.file-preview-code') as HTMLElement).getBoundingClientRect().width,
      rowHeight: rect.height,
      gutters: el.querySelectorAll('.line-number').length,
      gutterShift,
      tailInViewUnscrolled,
      tailInViewScrolled,
      scrolledBy,
    };
  });
}

/** Nothing outside the code region absorbed the long line: no page-level
 *  horizontal scrollbar, whatever the mode. */
async function pageDoesNotPan(page: Page): Promise<boolean> {
  return page.evaluate(() =>
    document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1);
}

const row = (scope: Locator | Page, line: number) =>
  scope.locator(`.code-line[data-line="${line}"]:visible`).first();

test.describe('a source line wider than the file preview', () => {
  test.beforeAll(() => {
    mkdirSync(APP_DIR, { recursive: true });
    writeFileSync(resolve(APP_DIR, 'index.html'), `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>long line preview</title>
<script src="/api/v1/sdk.js"></script></head>
<body>
<div id="ready">ready</div>
<script>
  window.runPreview = function(params) { return lucidos.ui.previewFile(params); };
</script>
</body>
</html>
`);
    writeFileSync(resolve(APP_DIR, 'manifest.json'), JSON.stringify({
      id: APP_ID,
      name: 'Long line preview test',
      description: 'e2e fixture',
    }));

    mkdirSync(resolve(WORKSPACE, 'data/artifacts'), { recursive: true });
    const lines = Array.from({ length: 12 }, (_, i) =>
      i + 1 === LONG_LINE ? LONG : `short line ${i + 1}`);
    writeFileSync(SAMPLE_FILE, `${lines.join('\n')}\n`);
  });

  test.afterAll(() => {
    rmSync(APP_DIR, { recursive: true, force: true });
    rmSync(SAMPLE_FILE, { force: true });
  });

  /** Seed the remembered mode before the app boots, so a test names the mode it
   *  is about instead of inheriting the previous one. */
  async function bootWithWrap(page: Page, wrap: boolean) {
    await page.addInitScript((on) => {
      localStorage.setItem('lucidos-file-preview-wrap', String(on));
    }, wrap);
  }

  // ── The modal an app opens over itself ──

  async function openModal(page: Page, wrap: boolean) {
    await bootWithWrap(page, wrap);
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');
    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 15_000 });
    const handle = await iframeLoc.elementHandle();
    const frame = handle && await handle.contentFrame();
    if (!frame) throw new Error('app iframe never mounted');
    await frame.waitForSelector('#ready');
    void frame.evaluate(
      (p) => (window as unknown as { runPreview: (o: unknown) => Promise<void> }).runPreview(p),
      { file_path: SAMPLE_PATH, line: LONG_LINE },
    );
    const modal = page.locator('[data-role="file-preview-modal"]');
    await expect(modal).toBeVisible({ timeout: 15_000 });
    await expect(row(modal, LONG_LINE)).toBeVisible();
    return modal;
  }

  test('wraps the cited line in the modal, gutter and highlight intact', async ({ page }) => {
    const modal = await openModal(page, true);
    const cited = await probeRow(row(modal, LONG_LINE));
    const short = await probeRow(row(modal, 1));

    // Reachable with no panning at all, which is the fix.
    expect(cited.tailInViewUnscrolled).toBe(true);
    expect(cited.scrollWidth).toBeLessThanOrEqual(cited.clientWidth + 1);

    // One number for the whole logical line, and the highlight covers the whole
    // wrapped block rather than only its first visual row.
    expect(cited.gutters).toBe(1);
    expect(cited.rowHeight).toBeGreaterThan(short.rowHeight * 1.5);
    await expect(row(modal, LONG_LINE)).toHaveClass(/line-selected/);

    // The modal's own width is a min() against the viewport, and it holds.
    const box = await modal.boundingBox();
    const viewport = page.viewportSize();
    expect(box!.width).toBeLessThanOrEqual(viewport!.width);
    expect(await pageDoesNotPan(page)).toBe(true);
  });

  test('pans the modal under a pinned gutter once wrapping is off', async ({ page }) => {
    const modal = await openModal(page, false);
    const before = await modal.boundingBox();
    const cited = await probeRow(row(modal, LONG_LINE));

    // The tail is off the right edge until the reader pans, and panning gets
    // there. That is the other acceptable answer to the same requirement.
    expect(cited.tailInViewUnscrolled).toBe(false);
    expect(cited.scrolledBy).toBeGreaterThan(0);
    expect(cited.tailInViewScrolled).toBe(true);

    // The gutter does not go with it, and the highlight runs the whole width.
    expect(cited.gutterShift).toBeLessThanOrEqual(1);
    expect(cited.rowWidth).toBeCloseTo(cited.blockWidth, 0);
    expect(cited.blockWidth).toBeGreaterThan(cited.clientWidth);

    // A long line must not widen the modal, in this mode least of all.
    const after = await modal.boundingBox();
    expect(after!.width).toBeCloseTo(before!.width, 0);
    expect(after!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    expect(await pageDoesNotPan(page)).toBe(true);
  });

  test('remembers the mode across a close and a reopen', async ({ page }) => {
    const modal = await openModal(page, true);
    const pre = modal.locator('.file-preview-code');
    await expect(pre).toHaveClass(/line-numbered-wrap/);

    await modal.locator('.file-preview-wrap-toggle').click();
    await expect(pre).toHaveClass(/line-numbered-pan/);

    await modal.locator('button[aria-label="Close preview"]').click();
    await expect(modal).toHaveCount(0);

    const reopened = await openModal(page, false);
    await expect(reopened.locator('.file-preview-code')).toHaveClass(/line-numbered-pan/);
  });

  // ── The same renderer, behind the Files panel ──

  async function openInFilesPanel(page: Page, wrap: boolean) {
    await bootWithWrap(page, wrap);
    await gotoWithRetry(page, '/');
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
    await openFilesPanel(page);
    await waitForVisibleElement(page, '.file-item', 15_000);
    expect(await clickVisibleElement(page, '.file-item', SAMPLE_NAME)).toBe(true);
    await expect(row(page, LONG_LINE)).toBeVisible({ timeout: 10_000 });
  }

  test('wraps the same line in the Files panel', async ({ page }) => {
    await openInFilesPanel(page, true);
    const cited = await probeRow(row(page, LONG_LINE));

    expect(cited.tailInViewUnscrolled).toBe(true);
    expect(cited.scrollWidth).toBeLessThanOrEqual(cited.clientWidth + 1);
    expect(cited.gutters).toBe(1);
    expect(await pageDoesNotPan(page)).toBe(true);
  });

  test('offers the Wrap toggle in the Files header and pans when it is off', async ({ page }) => {
    await openInFilesPanel(page, true);
    await clickHeaderAction(page, '.file-preview-wrap-toggle');

    const pre = page.locator('.file-preview-code:visible').first();
    await expect(pre).toHaveClass(/line-numbered-pan/);

    const cited = await probeRow(row(page, LONG_LINE));
    expect(cited.tailInViewUnscrolled).toBe(false);
    expect(cited.tailInViewScrolled).toBe(true);
    expect(cited.gutterShift).toBeLessThanOrEqual(1);
    expect(await pageDoesNotPan(page)).toBe(true);
  });

  // ── The pinned gutter's colour, in both themes ──

  /** What the gutter and its row actually paint, and what they sit on.
   *
   *  A sticky gutter paints over the code passing beneath it, so its colour is
   *  load-bearing rather than decorative. It has to be the row's own, or the
   *  cited line's tint breaks at the gutter. The row's has to be the
   *  container's, or the strip reads as a band in the wrong grey. Neither is
   *  visible to the other assertions here, and both change with the theme. */
  function paintOf(scope: Locator, surface: string) {
    return scope.evaluate((panel: HTMLElement, opts: { sel: string; line: number }) => {
      const bg = (el: Element) => getComputedStyle(el).backgroundColor;
      const of = (line: number) => {
        const el = panel.querySelector(`.code-line[data-line="${line}"]`)!;
        return { row: bg(el), gutter: bg(el.querySelector('.line-number')!) };
      };
      return {
        theme: document.documentElement.getAttribute('data-theme'),
        surface: bg(document.querySelector(opts.sel)!),
        cited: of(opts.line),
        plain: of(1),
      };
    }, { sel: surface, line: LONG_LINE });
  }

  for (const theme of ['dark', 'light'] as const) {
    test(`the pinned gutter wears its row and its surface in ${theme} theme`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: theme });
      const modal = await openModal(page, false);
      const paint = await paintOf(modal, '[data-role="file-preview-modal"]');

      expect(paint.theme, 'the emulated scheme did not reach the document').toBe(theme);
      // An ordinary row is the panel's own surface, so the pinned strip reads
      // as the page rather than as a band.
      expect(paint.plain.row).toBe(paint.surface);
      // And the gutter is whatever its row is wearing, tinted or not, so the
      // cited line's highlight runs unbroken across it.
      expect(paint.plain.gutter).toBe(paint.plain.row);
      expect(paint.cited.gutter).toBe(paint.cited.row);
      expect(paint.cited.row).not.toBe(paint.plain.row);
    });
  }

  // ── The review loop ──

  /** Colour is the one thing an assertion cannot judge, and the pinned gutter
   *  paints a surface that has to match its container in both themes. So this
   *  writes PNGs instead of asserting, the same shape as `header-shots-*`:
   *
   *    PREVIEW_SHOTS=1 ./scripts/e2e-browser.sh -f file-preview-long-lines.spec.ts
   *
   *  They land in `test-results/preview-shots/`. Chromium only, since the point
   *  is the stylesheet rather than the engine. */
  test.describe('shots', () => {
    const DIR = 'test-results/preview-shots';

    test.skip(process.env.PREVIEW_SHOTS !== '1', 'set PREVIEW_SHOTS=1 to take screenshots');

    /** One project is enough: the subject is the stylesheet, not the engine,
     *  and the narrow width below covers what a phone would add.
     *
     *  Per test rather than on the describe, because a describe-level modifier
     *  callback is handed the FIXTURES alone. Reaching for `testInfo` there
     *  throws before any shot is taken. */
    const chromiumOnly = (testInfo: { project: { name: string } }) =>
      test.skip(testInfo.project.name !== 'chromium', 'chromium only');

    async function frame(page: Page, theme: 'dark' | 'light', width: number) {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
    }

    /** The boot splash is an opaque `inset: 0` overlay that fades once the app
     *  has painted. A shot taken under it is a picture of the splash. */
    async function splashGone(page: Page) {
      await expect
        .poll(() => page.evaluate(() => document.querySelectorAll('.boot-splash').length),
          { timeout: 30_000 })
        .toBe(0);
    }

    for (const theme of ['dark', 'light'] as const) {
      for (const [label, width] of [['wide', 1280], ['narrow', 430]] as const) {
        // The two surfaces are separate tests, and not for tidiness. `openModal`
        // seeds `app-window-open` in an init script, and that script re-runs on
        // every navigation. A Files-panel visit afterwards therefore lands with
        // the app reopened over the pane, and never reaches the preview.
        test(`modal ${theme} ${label}`, async ({ page }, testInfo) => {
          chromiumOnly(testInfo);
          await frame(page, theme, width);
          const modal = await openModal(page, true);
          await splashGone(page);
          await modal.screenshot({ path: `${DIR}/modal-wrap-${theme}-${label}.png` });

          await modal.locator('.file-preview-wrap-toggle').click();
          await expect(modal.locator('.file-preview-code')).toHaveClass(/line-numbered-pan/);
          await modal.screenshot({ path: `${DIR}/modal-pan-${theme}-${label}.png` });
        });

        test(`panel ${theme} ${label}`, async ({ page }, testInfo) => {
          chromiumOnly(testInfo);
          await frame(page, theme, width);
          await openInFilesPanel(page, true);
          await splashGone(page);
          const pane = page.locator('.file-preview-inline:visible').first();
          await pane.screenshot({ path: `${DIR}/panel-wrap-${theme}-${label}.png` });

          await clickHeaderAction(page, '.file-preview-wrap-toggle');
          await expect(page.locator('.file-preview-code:visible').first())
            .toHaveClass(/line-numbered-pan/);
          await pane.screenshot({ path: `${DIR}/panel-pan-${theme}-${label}.png` });
        });
      }
    }
  });
});
