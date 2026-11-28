import { test, expect, Page } from './fixtures';
import {
  navigateToApp, uniqueMessage, assertHealthy, setVoiceEnabled, waitForVisibleInput, watchPreferenceReads,
  composerControl,
} from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

interface RowMetrics {
  boxLeft: number;
  boxRight: number;
  contentLeft: number;
  contentRight: number;
  items: Array<{ left: number; right: number; top: number; bottom: number; label: string }>;
  iconLefts: number[];
  iconWidth: number;
  leadingDesc: string[];
  folded: boolean;
  changeActionUp: boolean;
  sendCount: number;
}

async function measureRow(page: Page): Promise<RowMetrics | null> {
  return page.evaluate(() => {
    const rows = Array.from(document.querySelectorAll<HTMLElement>('.prompt-actions-row'));
    const row = rows.find(r => r.getBoundingClientRect().width > 0);
    if (!row) return null;
    const r = row.getBoundingClientRect();
    const cs = getComputedStyle(row);
    const rowItems = Array.from(row.querySelectorAll<HTMLElement>('[data-row-item]'));
    const nameOf = (el: Element) =>
      el.getAttribute('aria-label')
      ?? el.querySelector('[aria-label]')?.getAttribute('aria-label')
      ?? el.className;
    const items = rowItems.map((el) => {
      const b = el.getBoundingClientRect();
      return { left: b.left, right: b.right, top: b.top, bottom: b.bottom, label: nameOf(el) };
    });
    if (items.length === 0) return null;
    const leading = rowItems.filter(el => !el.closest('.prompt-actions-right'));
    return {
      boxLeft: r.left,
      boxRight: r.right,
      contentLeft: r.left + (parseFloat(cs.paddingLeft) || 0),
      contentRight: r.right - (parseFloat(cs.paddingRight) || 0),
      items,
      iconLefts: leading.map(el => el.getBoundingClientRect().left),
      iconWidth: leading[0]?.getBoundingClientRect().width ?? 0,
      leadingDesc: leading.map(nameOf),
      folded: !!row.querySelector('[data-fold-key=""]'),
      changeActionUp: !!row.querySelector('.prompt-actions-right > [data-thread-action]'),
      sendCount: row.querySelectorAll('.send-cancel-morph').length,
    };
  });
}

const bannerShowing = (m: RowMetrics) => m.sendCount === 0;

const draftShowing = (m: RowMetrics) => m.sendCount === 1;

async function settledMetrics(
  page: Page,
  label: string,
  ready: (m: RowMetrics) => boolean,
): Promise<RowMetrics> {
  let previous: string | null = null;
  await expect
    .poll(async () => {
      const m = await measureRow(page);
      if (!m || !ready(m)) return false;
      const signature = JSON.stringify([
        m.folded,
        m.leadingDesc,
        m.items.map(i => [Math.round(i.left), Math.round(i.right)]),
      ]);
      const settled = signature === previous;
      previous = signature;
      return settled;
    }, { timeout: 15_000, message: `${label}: the action row never settled` })
    .toBe(true);
  return (await measureRow(page))!;
}

async function setScaleAndSettle(page: Page, scale: number, where: string): Promise<RowMetrics> {
  await page.evaluate(
    (s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`),
    scale,
  );
  const m = await settledMetrics(page, where, bannerShowing);
  expect(m.iconWidth, `${where}: the root font size never took`)
    .toBeCloseTo(2.25 * 16 * (scale / 100), 0);
  return m;
}

function expectInsideTheBox(m: RowMetrics, when: string): void {
  const atTheFloor = m.folded && m.leadingDesc.length <= 2;
  const [left, right, edge] = atTheFloor
    ? [m.boxLeft, m.boxRight, 'box']
    : [m.contentLeft, m.contentRight, 'content'];
  for (const item of m.items) {
    expect(
      item.right,
      `${when}: "${item.label}" reaches ${item.right.toFixed(1)}, past the row's `
      + `${edge} edge at ${right.toFixed(1)}`,
    ).toBeLessThanOrEqual(right + 0.5);
    expect(item.left, `${when}: "${item.label}" starts left of the row's ${edge} box`)
      .toBeGreaterThanOrEqual(left - 0.5);
  }
  for (const item of m.items) {
    expect(item.right, `${when}: "${item.label}" left the composer box`)
      .toBeLessThanOrEqual(m.boxRight + 0.5);
    expect(item.left, `${when}: "${item.label}" left the composer box`)
      .toBeGreaterThanOrEqual(m.boxLeft - 0.5);
  }
}

function expectOneRow(m: RowMetrics, when: string): void {
  const lowestTop = Math.max(...m.items.map(i => i.top));
  const highestBottom = Math.min(...m.items.map(i => i.bottom));
  expect(
    lowestTop,
    `${when}: no line crosses every member, so the row drew two `
    + `(${m.items.map(i => `${i.label}@${Math.round(i.top)}-${Math.round(i.bottom)}`).join(', ')})`,
  ).toBeLessThan(highestBottom);
}

test.describe('Composer action row - one row, nothing outside the box', () => {
  test.use({ viewport: { width: 393, height: 852 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test.afterAll(async ({ browser }) => {
    const page = await browser.newPage();
    await setVoiceEnabled(page, false);
    await page.close();
  });

  for (const voice of [false, true]) {
    for (const { width, scale } of [
      { width: 393, scale: 100 },
      { width: 393, scale: 137.5 },
      { width: 393, scale: 200 },
      { width: 340, scale: 137.5 },
      { width: 300, scale: 200 },
    ]) {
      const where = `voice ${voice ? 'on' : 'off'}, ${width}pt, ui-scale ${scale}`;
      test(`stays one row inside its box with ${where}`, async ({ page }) => {
        const suffix = uniqueMessage('rowfit').replace(/[^a-z0-9-]/g, '');
        const { threadId, changeId, branch, file } = createCCThreadWithChange(
          'E2E Row Fit', suffix, { requiresRestart: true },
        );

        try {
          await page.setViewportSize({ width, height: 852 });
          await page.addInitScript((tid: string) => {
            localStorage.setItem('lucidos-focused-thread', tid);
          }, threadId);
          await setVoiceEnabled(page, voice);
          const preferencesSettled = watchPreferenceReads(page);
          await navigateToApp(page);
          await preferencesSettled();
          await expect(page.locator('.thread-action-buttons:visible'))
            .toBeVisible({ timeout: 15_000 });

          const empty = await setScaleAndSettle(page, scale, where);
          expectInsideTheBox(empty, `${where}, empty draft`);
          expectOneRow(empty, `${where}, empty draft`);

          expect(empty.leadingDesc[0], `${where}: the control menu is not first`)
            .toBeTruthy();
          expect(empty.changeActionUp, `${where}: the change action left the action slot`).toBe(true);

          expect(empty.leadingDesc, `${where}: an empty draft reserves a clear button`)
            .not.toContain('Clear draft');
          expect(empty.sendCount, `${where}: the banner owns the send slot`).toBe(0);

          await (await waitForVisibleInput(page)).fill('x');
          const typed = await settledMetrics(page, `${where}, typed`, draftShowing);
          expectInsideTheBox(typed, `${where}, one character typed`);
          expectOneRow(typed, `${where}, one character typed`);
          expect(typed.iconLefts[0], `${where}: the anchor moved when the draft opened`)
            .toBeCloseTo(empty.iconLefts[0], 1);
          expect(typed.sendCount, `${where}: the send morph replaces the banner`).toBe(1);

          await expect(await composerControl(page, 'button.prompt-clear')).toBeVisible();
        } finally {
          cleanupCCThread(threadId, changeId, branch, file);
        }
      });
    }
  }
});
