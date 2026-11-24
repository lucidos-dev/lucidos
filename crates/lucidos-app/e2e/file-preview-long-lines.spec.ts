import { test, expect, type Locator, type Page } from './fixtures';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import {
  clickHeaderAction, clickVisibleElement, ensureOnThreadPane, gotoWithRetry,
  openFilesPanel, waitForVisibleElement, waitForVisibleInput,
} from './helpers';


const APP_ID = 'e2e-long-line-preview';
const APP_DIR = resolve(WORKSPACE, 'data/apps', APP_ID);
const SAMPLE_NAME = 'e2e-long-line-sample.txt';
const SAMPLE_PATH = `artifacts/${SAMPLE_NAME}`;
const SAMPLE_FILE = resolve(WORKSPACE, 'data', SAMPLE_PATH);

const LONG_LINE = 3;
const TAIL = 'THE-TAIL-THE-CITATION-MEANT';
const LONG = `const wide = ${Array.from({ length: 90 }, (_, i) => `segment_${i}`).join('_')}_${TAIL};`;

const LONG_FILE_NAME = 'e2e-long-file-sample.json';
const LONG_FILE = resolve(WORKSPACE, 'data/artifacts', LONG_FILE_NAME);
const LONG_FILE_LINES = 3000;

function gutterOf(row: Element): Element {
  const inRow = row.querySelector('.line-number');
  if (inRow) return inRow;
  const index = Array.prototype.indexOf.call(row.parentElement!.children, row);
  return row.closest('pre')!.querySelector('.line-numbered-gutter')!.children[index];
}

const backgroundOf = (el: Element) => getComputedStyle(el).backgroundColor;

interface RowProbe {
  scrollWidth: number;
  clientWidth: number;
  bandWidth: number;
  blockWidth: number;
  landedScrollLeft: number;
  rowHeight: number;
  gutters: number;
  gutterShift: number;
  tailInViewUnscrolled: boolean;
  tailInViewScrolled: boolean;
  scrolledBy: number;
}

async function probeRow(row: Locator): Promise<RowProbe> {
  const gutterCell = await row.evaluateHandle(gutterOf);
  return row.evaluate((el: HTMLElement, gutter: Element) => {
    const scroller = el.closest('.file-preview-content, .repo-file-content') as HTMLElement;
    const content = el.querySelector('.line-content') as HTMLElement;

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

    const landedScrollLeft = scroller.scrollLeft;
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
      bandWidth: rect.right - gutter.getBoundingClientRect().left,
      landedScrollLeft,
      blockWidth: (el.closest('.file-preview-code') as HTMLElement).getBoundingClientRect().width,
      rowHeight: rect.height,
      gutters: el.querySelectorAll('.line-number').length,
      gutterShift,
      tailInViewUnscrolled,
      tailInViewScrolled,
      scrolledBy,
    };
  }, gutterCell);
}

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
    const longFile = Array.from({ length: LONG_FILE_LINES }, (_, i) =>
      `  "row_${i + 1}": "${'a value too wide for any screen '.repeat(9)}${i + 1}",`);
    writeFileSync(LONG_FILE, longFile.join('\n'));
  });

  test.afterAll(() => {
    rmSync(APP_DIR, { recursive: true, force: true });
    rmSync(SAMPLE_FILE, { force: true });
    rmSync(LONG_FILE, { force: true });
  });

  async function bootWithWrap(page: Page, wrap: boolean) {
    await page.addInitScript((on) => {
      localStorage.setItem('lucidos-file-preview-wrap', String(on));
    }, wrap);
  }


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

    expect(cited.tailInViewUnscrolled).toBe(true);
    expect(cited.scrollWidth).toBeLessThanOrEqual(cited.clientWidth + 1);

    expect(cited.gutters).toBe(1);
    expect(cited.rowHeight).toBeGreaterThan(short.rowHeight * 1.5);
    await expect(row(modal, LONG_LINE)).toHaveClass(/line-selected/);

    const box = await modal.boundingBox();
    const viewport = page.viewportSize();
    expect(box!.width).toBeLessThanOrEqual(viewport!.width);
    expect(await pageDoesNotPan(page)).toBe(true);
  });

  test('pans the modal under a pinned gutter once wrapping is off', async ({ page }) => {
    const modal = await openModal(page, false);
    const before = await modal.boundingBox();
    const cited = await probeRow(row(modal, LONG_LINE));

    expect(cited.tailInViewUnscrolled).toBe(false);
    expect(cited.scrolledBy).toBeGreaterThan(0);
    expect(cited.tailInViewScrolled).toBe(true);

    expect(cited.landedScrollLeft).toBeLessThanOrEqual(1);

    expect(cited.gutterShift).toBeLessThanOrEqual(1);
    expect(cited.bandWidth).toBeCloseTo(cited.blockWidth, 0);
    expect(cited.blockWidth).toBeGreaterThan(cited.clientWidth);

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


  async function openInFilesPanel(page: Page, wrap: boolean, name = SAMPLE_NAME) {
    await bootWithWrap(page, wrap);
    await gotoWithRetry(page, '/');
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
    await openFilesPanel(page);
    await waitForVisibleElement(page, '.file-item', 15_000);
    expect(await clickVisibleElement(page, '.file-item', name)).toBe(true);
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


  test('pans a long file under one pinned gutter, every row level with its number', async ({ page }) => {
    await openInFilesPanel(page, false, LONG_FILE_NAME);
    const pre = page.locator('.line-numbered-pan:visible').first();
    await expect(pre.locator(`.code-line[data-line="${LONG_FILE_LINES}"]`)).toHaveCount(1);

    const probe = await pre.evaluate((el: HTMLElement, last: number) => {
      const scroller = el.closest('.file-preview-content') as HTMLElement;
      const gutter = el.querySelector('.line-numbered-gutter') as HTMLElement;
      const cells = gutter.children;
      const rowOf = (n: number) => el.querySelector(`.code-line[data-line="${n}"]`) as HTMLElement;
      const skew = (n: number) => Math.abs(
        cells[n - 1].getBoundingClientRect().top - rowOf(n).getBoundingClientRect().top);
      let sticky = 0;
      for (const node of el.querySelectorAll('*')) if (getComputedStyle(node).position === 'sticky') sticky++;

      scroller.scrollLeft = 0;
      const before = gutter.getBoundingClientRect().left;
      scroller.scrollLeft = scroller.scrollWidth;
      const scrolledBy = scroller.scrollLeft;
      const shift = Math.abs(gutter.getBoundingClientRect().left - before);
      const box = gutter.getBoundingClientRect();
      const stack = document.elementsFromPoint(box.left + box.width / 2, innerHeight / 2);
      const gutterAt = stack.findIndex(n => n.closest('.line-numbered-gutter'));
      const codeAt = stack.findIndex(n => n.closest('.line-numbered-code'));
      const onTop = gutterAt >= 0 && (codeAt < 0 || gutterAt < codeAt);
      scroller.scrollLeft = 0;
      return {
        sticky, cellCount: cells.length, scrolledBy, shift, onTop,
        firstSkew: skew(1), lastSkew: skew(last),
        lastNumber: cells[last - 1].getAttribute('data-line-number'),
      };
    }, LONG_FILE_LINES);

    expect(probe.sticky).toBe(1);
    expect(probe.cellCount).toBe(LONG_FILE_LINES);
    expect(probe.lastNumber).toBe(String(LONG_FILE_LINES));
    expect(probe.scrolledBy).toBeGreaterThan(0);
    expect(probe.shift).toBeLessThanOrEqual(1);
    expect(probe.onTop).toBe(true);
    expect(probe.firstSkew).toBeLessThanOrEqual(1);
    expect(probe.lastSkew).toBeLessThanOrEqual(1);
    expect(await pageDoesNotPan(page)).toBe(true);
  });


  async function paintOf(scope: Locator, surface: string) {
    const of = async (line: number) => {
      const el = scope.locator(`.code-line[data-line="${line}"]`).first();
      return {
        row: await el.evaluate(backgroundOf),
        gutter: await (await el.evaluateHandle(gutterOf)).evaluate(backgroundOf),
      };
    };
    return {
      theme: await scope.page().evaluate(() => document.documentElement.getAttribute('data-theme-mode')),
      surface: await scope.page().locator(surface).evaluate(backgroundOf),
      column: await scope.locator('.line-numbered-gutter').evaluate(backgroundOf),
      cited: await of(LONG_LINE),
      plain: await of(1),
    };
  }

  for (const theme of ['dark', 'light'] as const) {
    test(`the pinned gutter wears its row and its surface in ${theme} theme`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: theme });
      const modal = await openModal(page, false);
      const paint = await paintOf(modal, '[data-role="file-preview-modal"]');

      expect(paint.theme, 'the emulated scheme did not reach the document').toBe(theme);
      expect(paint.column).toBe(paint.surface);
      expect(paint.plain.row).toBe(paint.plain.gutter);
      expect(paint.cited.gutter).toBe(paint.cited.row);
      expect(paint.cited.row).not.toBe(paint.plain.row);
    });
  }


  test.describe('shots', () => {
    const DIR = 'test-results/preview-shots';

    test.skip(process.env.PREVIEW_SHOTS !== '1', 'set PREVIEW_SHOTS=1 to take screenshots');

    async function frame(page: Page, theme: 'dark' | 'light', width: number) {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
    }

    async function splashGone(page: Page) {
      await expect
        .poll(() => page.evaluate(() => document.querySelectorAll('.boot-splash').length),
          { timeout: 30_000 })
        .toBe(0);
    }

    for (const theme of ['dark', 'light'] as const) {
      for (const [label, width] of [['wide', 1280], ['narrow', 430]] as const) {
        test(`modal ${theme} ${label}`, async ({ page }) => {
          await frame(page, theme, width);
          const modal = await openModal(page, true);
          await splashGone(page);
          await modal.screenshot({ path: `${DIR}/modal-wrap-${theme}-${label}.png` });

          await modal.locator('.file-preview-wrap-toggle').click();
          await expect(modal.locator('.file-preview-code')).toHaveClass(/line-numbered-pan/);
          await modal.screenshot({ path: `${DIR}/modal-pan-${theme}-${label}.png` });
        });

        test(`panel ${theme} ${label}`, async ({ page }) => {
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
