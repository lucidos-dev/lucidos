/**
 * The composer's bottom row is ONE row, and nothing leaves its box.
 *
 * Measured on the densest row the composer draws: a coding-agent thread with a
 * pending change, carrying its leading icons, the Diff button and the Apply
 * split button. Swept across widths and UI scales, because both promises are
 * about room and a single cell proves neither.
 *
 * 1. **Nothing crosses the row's content edge.** Every `[data-row-item]` sits
 *    inside it, at every cell. An Archive button clipped by the screen edge is
 *    what this file exists to catch.
 * 2. **One row.** One horizontal line crosses every member. The row used to
 *    give way three ways that disagreed: a fold, an `is-stacked` sub-row lift
 *    and a `flex-wrap` on the right-hand cluster. The fold is the only one now.
 * 3. **Nothing jumps.** The leading icons hold their positions when the first
 *    character lands.
 *
 * On a coding-agent destination the call toggle is absent whatever voice reads
 * (ADR 0165), so voice adds no box to this row. The spec still measures voice
 * off and on, to prove that. It arms the switch itself and puts it back.
 *
 * Plan: `docs/plans/2026-09-19-the-composer-row-is-one-row.md`.
 */
import { test, expect, Page } from './fixtures';
import {
  navigateToApp, uniqueMessage, assertHealthy, setVoiceEnabled, waitForVisibleInput,
  composerControl,
} from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

interface RowMetrics {
  /** The row's own box. Nothing may leave it, ever: this is the composer's
   *  visible edge, and a control past it is the reported defect. */
  boxLeft: number;
  boxRight: number;
  /** Inner edges of the row's content box, its own padding excluded. */
  contentLeft: number;
  contentRight: number;
  /** Every measured `[data-row-item]`, in document order. */
  items: Array<{ left: number; right: number; top: number; bottom: number; label: string }>;
  /** Left edges of the leading cluster, which must not move when a draft opens.
   *  It is everything the row carries outside `.prompt-actions-right`. */
  iconLefts: number[];
  /** Width of the leading cluster's first box, the scale probe below. */
  iconWidth: number;
  /** What the leading cluster holds, so a wrong set names itself. */
  leadingDesc: string[];
  /** Is anything folded? The ⋯ carries the fold marker with an empty name. */
  folded: boolean;
  /** Is the change action in the row? Read from the split button's own face
   *  rather than from a label: it carries no `aria-label`, so the item's name
   *  falls through to its class. */
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
      changeActionUp: !!row.querySelector('.split-button-primary'),
      sendCount: row.querySelectorAll('.send-cancel-morph').length,
    };
  });
}

/** The row is showing the change banner: the send morph has yielded to it. */
const bannerShowing = (m: RowMetrics) => m.sendCount === 0;

/** The row a typed draft produces: the banner has yielded to the send morph. */
const draftShowing = (m: RowMetrics) => m.sendCount === 1;

/** A reading of the row, taken only once two consecutive reads agree.
 *
 *  Every assertion here is about the row's RESTING layout, and every one of
 *  them can be read too early. The fold re-runs on a resize and its answer
 *  lands a render later, so a measurement taken mid-settle reports the previous
 *  layout. */
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

/** Apply a ui-scale, then take a settled reading. Everything in this row is
 *  rem-sized, so the scale change relays the whole row out. */
async function setScaleAndSettle(page: Page, scale: number, where: string): Promise<RowMetrics> {
  await page.evaluate(
    (s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`),
    scale,
  );
  const m = await settledMetrics(page, where, bannerShowing);
  // `.icon-btn.header-icon` is 2.25rem, against a root of `--user-ui-scale`
  // percent of the browser's own 16px default. Proves the scale really applied.
  expect(m.iconWidth, `${where}: the root font size never took`)
    .toBeCloseTo(2.25 * 16 * (scale / 100), 0);
  return m;
}

/** Promise 1, in two strengths.
 *
 *  **Nothing may leave the row's BOX, ever.** That is the composer's visible
 *  edge, and a control past it is the defect this file was written for.
 *
 *  **Where the row still has something to fold, nothing may enter its PADDING
 *  either.** Once the fold has bottomed out the row has done all it can, and
 *  what is left is the floor: the control menu, the ⋯ and the send button. At
 *  300pt with a 200% root those three are wider than the content box, so they
 *  spend some of the padding. They still do not leave the row. */
function expectInsideTheBox(m: RowMetrics, when: string): void {
  // The floor: the anchor and the ⋯ are all the leading cluster has left.
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
  // The hard promise holds at every cell, floor or not.
  for (const item of m.items) {
    expect(item.right, `${when}: "${item.label}" left the composer box`)
      .toBeLessThanOrEqual(m.boxRight + 0.5);
    expect(item.left, `${when}: "${item.label}" left the composer box`)
      .toBeGreaterThanOrEqual(m.boxLeft - 0.5);
  }
}

/** Promise 2. One horizontal line crosses every member, so there is one row.
 *
 *  Overlap, not a shared centre. The row sits its children on `flex-end` and
 *  they are not one height. So a 2.25rem icon box and a shorter action button
 *  have centres a few px apart while plainly sharing a line. Two ROWS is the
 *  thing to catch, and two rows means disjoint vertical spans. */
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
  // iPhone 15 Pro portrait points, the device the report came from.
  test.use({ viewport: { width: 393, height: 852 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  // Voice is global and ships off, so put it back for every later spec.
  test.afterAll(async ({ browser }) => {
    const page = await browser.newPage();
    await setVoiceEnabled(page, false);
    await page.close();
  });

  // The sweep. Width and ui-scale decide how much room the row has, and the
  // fold answers for every combination of the two.
  //
  // `voice` is the experimental switch. On a coding-agent destination the call
  // toggle is absent whatever it reads (ADR 0165), so voice adds no box here.
  // Measuring both is the guard that a call toggle wrongly returning to this
  // row fails the sweep.
  //
  // 200% is the top of the ui-scale range a user can reach, and 300pt is
  // narrower than any phone ships. Neither is a corner the row may give up in:
  // the pinned floor is the control menu and the send button.
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
          // Set the state, never inherit it. The app reads its preferences at
          // boot, so this lands before the navigation.
          await setVoiceEnabled(page, voice);
          await navigateToApp(page);
          await expect(page.locator('.thread-action-buttons:visible'))
            .toBeVisible({ timeout: 15_000 });

          const empty = await setScaleAndSettle(page, scale, where);
          expectInsideTheBox(empty, `${where}, empty draft`);
          expectOneRow(empty, `${where}, empty draft`);

          // The two pinned members are there at every cell. They are what makes
          // the floor always fit, so a sweep that loses one has lost the floor.
          expect(empty.leadingDesc[0], `${where}: the control menu is not first`)
            .toBeTruthy();
          expect(
            empty.changeActionUp || empty.folded,
            `${where}: the change action is neither in the row nor in the ⋯`,
          ).toBe(true);

          // The empty row carries no clear-draft box. A reserved one with
          // nothing to clear is a box the row spends on nothing.
          expect(empty.leadingDesc, `${where}: an empty draft reserves a clear button`)
            .not.toContain('Clear draft');
          expect(empty.sendCount, `${where}: the banner owns the send slot`).toBe(0);

          // Promise 3: the first character mounts the clear action and the send
          // morph, and takes the banner away. The anchor does not move.
          await (await waitForVisibleInput(page)).fill('x');
          const typed = await settledMetrics(page, `${where}, typed`, draftShowing);
          expectInsideTheBox(typed, `${where}, one character typed`);
          expectOneRow(typed, `${where}, one character typed`);
          expect(typed.iconLefts[0], `${where}: the anchor moved when the draft opened`)
            .toBeCloseTo(empty.iconLefts[0], 1);
          expect(typed.sendCount, `${where}: the send morph replaces the banner`).toBe(1);

          // The clear action arrived, standing or folded. Where it landed is the
          // fold's business; this asks only that it is reachable.
          await expect(await composerControl(page, 'button.prompt-clear')).toBeVisible();
        } finally {
          cleanupCCThread(threadId, changeId, branch, file);
        }
      });
    }
  }
});
