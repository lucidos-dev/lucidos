/**
 * The composer's bottom row holds what it has room for, and no more.
 *
 * Two promises, on the densest row the composer draws: a coding-agent thread
 * with a pending change, carrying its leading icons, the standalone Diff
 * button and the Apply split button.
 *
 * 1. **Diff shares the row while the row can hold it.** It lifts onto a
 *    sub-row only on a genuine overflow. It used to lift with room to spare.
 *    The row reserved a 2.25rem box for a clear-draft button with nothing to
 *    clear. The fit check also billed a gap between every pair, where only
 *    `.prompt-actions-right` declares one.
 * 2. **Nothing overflows and nothing jumps.** Every `[data-row-item]` stays
 *    inside the row's content box. The leading icons hold their positions when
 *    the first character lands. The clear button mounts at the END of their
 *    cluster, whose next sibling takes the free space.
 *
 * On a coding-agent destination the call toggle is absent whatever voice reads
 * (ADR 0165), so voice adds no box to this row. The spec still measures voice
 * off and on, to prove that. It arms the switch itself and puts it back, so
 * neither reading depends on what ran before it. The overflow that lifts Diff
 * is driven by ui-scale, which a user can raise as high as 200%.
 */
import { test, expect, Page } from './fixtures';
import {
  navigateToApp, uniqueMessage, assertHealthy, setVoiceEnabled, waitForVisibleInput,
} from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

interface RowMetrics {
  /** Inner edges of the row's content box, its own padding excluded. */
  contentLeft: number;
  contentRight: number;
  /** True while `.prompt-actions-right` is in its lifted two-sub-row layout. */
  stacked: boolean;
  /** Every measured `[data-row-item]`, in document order. */
  items: Array<{ left: number; right: number }>;
  /** Left edges of the leading cluster, which must not move. It is everything
   *  the row carries outside `.prompt-actions-right`. A control wrapped in an
   *  anchor div (the agent menu, the attach menu) therefore counts once, like
   *  the bare buttons beside it. */
  iconLefts: number[];
  /** Width of the leading cluster's first box, the settle probe below. */
  iconWidth: number;
  /** What the leading cluster holds, so a wrong count names itself. */
  leadingDesc: string[];
  /** Vertical middles, for the "same row" check. `null` when absent. */
  diffMiddle: number | null;
  applyMiddle: number | null;
  sendCount: number;
}

async function measureRow(page: Page): Promise<RowMetrics | null> {
  return page.evaluate(() => {
    const rows = Array.from(document.querySelectorAll<HTMLElement>('.prompt-actions-row'));
    const row = rows.find(r => r.getBoundingClientRect().width > 0);
    if (!row) return null;
    const r = row.getBoundingClientRect();
    const cs = getComputedStyle(row);
    const middleOf = (el: Element) => {
      const b = el.getBoundingClientRect();
      return b.top + b.height / 2;
    };
    const diff = Array.from(row.querySelectorAll('button'))
      .find(b => b.textContent?.trim() === 'Diff');
    const apply = row.querySelector('.split-button-primary');
    const rowItems = Array.from(row.querySelectorAll<HTMLElement>('[data-row-item]'));
    const items = rowItems.map((el) => {
      const b = el.getBoundingClientRect();
      return { left: b.left, right: b.right };
    });
    if (items.length === 0) return null;
    const leading = rowItems.filter(el => !el.closest('.prompt-actions-right'));
    return {
      contentLeft: r.left + (parseFloat(cs.paddingLeft) || 0),
      contentRight: r.right - (parseFloat(cs.paddingRight) || 0),
      stacked: !!row.querySelector('.prompt-actions-right.is-stacked'),
      items,
      iconLefts: leading.map(el => el.getBoundingClientRect().left),
      iconWidth: leading[0]?.getBoundingClientRect().width ?? 0,
      leadingDesc: leading.map(el =>
        el.getAttribute('aria-label')
        ?? el.querySelector('[aria-label]')?.getAttribute('aria-label')
        ?? el.className),
      diffMiddle: diff ? middleOf(diff) : null,
      applyMiddle: apply ? middleOf(apply) : null,
      sendCount: row.querySelectorAll('.send-cancel-morph').length,
    };
  });
}

/** The row is showing the change banner: Diff beside the Apply face. */
const bannerShowing = (m: RowMetrics) => m.diffMiddle !== null && m.applyMiddle !== null;

/** The row a typed draft produces. The banner yields to the send morph, so the
 *  Apply face is gone and the standalone Diff is what remains. */
const draftShowing = (m: RowMetrics) =>
  m.diffMiddle !== null && m.sendCount === 1 && m.leadingDesc.includes('Clear draft');

/** A reading of the row, taken only once two consecutive reads agree.
 *
 *  Every assertion here is about the row's RESTING layout, and every one of
 *  them can be read too early. The fit check re-runs on a resize and its answer
 *  lands a render later, so a measurement taken mid-settle reports the previous
 *  layout. This spec passed against the pre-fix build until it waited. */
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
        m.stacked,
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

/** No item may cross either inner edge of the row. */
function expectNoOverflow(m: RowMetrics, when: string): void {
  const widest = Math.max(...m.items.map(i => i.right));
  const leftmost = Math.min(...m.items.map(i => i.left));
  expect(
    widest,
    `${when}: an item reaches ${widest.toFixed(1)}, past the row's content edge `
    + `at ${m.contentRight.toFixed(1)}`,
  ).toBeLessThanOrEqual(m.contentRight + 0.5);
  expect(leftmost, `${when}: an item starts left of the row's content box`)
    .toBeGreaterThanOrEqual(m.contentLeft - 0.5);
}

test.describe('Composer action row - Diff keeps its seat while the row fits', () => {
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

  // Four configurations, and each names the row it measures.
  //
  // `voice` is the experimental switch. On a coding-agent destination the call
  // toggle is absent whatever it reads (ADR 0165), so voice adds no box here.
  // The row is three boxes either way: the agent menu, the follow toggle and
  // the attach button. Measuring voice off AND on is the guard that a call
  // toggle wrongly returning to this row fails the box count.
  //
  // ui-scale drives the lift, not voice. 100% has room to spare at this width.
  // 137.5% is the cell that overflows: the five items ask for more than the
  // content box holds, so the row lifts Diff onto a sub-row. That lift is the
  // row keeping promise 1, reachable because a user can raise the root to 200%.
  //
  // `boxes` and `liftsDiff` are measured, not chosen. A control added or
  // removed moves them. Re-measure and re-set the row rather than relaxing
  // an assertion, which is what would stop this spec noticing the next
  // control.
  for (const { voice, scale, boxes, liftsDiff } of [
    { voice: false, scale: 100, boxes: 3, liftsDiff: false },
    { voice: false, scale: 137.5, boxes: 3, liftsDiff: true },
    { voice: true, scale: 100, boxes: 3, liftsDiff: false },
    { voice: true, scale: 137.5, boxes: 3, liftsDiff: true },
  ]) {
    const where = `voice ${voice ? 'on' : 'off'}, ui-scale ${scale}`;
    test(`Diff keeps the seat the row can afford with ${where}, and typing moves no icon`, async ({ page }) => {
      const suffix = uniqueMessage('rowfit').replace(/[^a-z0-9-]/g, '');
      const { threadId, changeId, branch, file } = createCCThreadWithChange(
        'E2E Row Fit', suffix, { requiresRestart: true },
      );

      try {
        await page.addInitScript((tid: string) => {
          localStorage.setItem('lucidos-focused-thread', tid);
        }, threadId);
        // Set the state, never inherit it. The app reads its preferences at
        // boot, so this lands before the navigation.
        await setVoiceEnabled(page, voice);
        await navigateToApp(page);
        await expect(page.locator('.thread-action-buttons:visible .split-button-primary'))
          .toBeVisible({ timeout: 15_000 });

        const empty = await setScaleAndSettle(page, scale, where);

        // Promise 1: the row lifts Diff exactly when it cannot hold it, and
        // never with room to spare.
        // A stacked row still ends flush with the content edge, so the width
        // it ASKS for is the honest number to report here.
        const asked = empty.items.reduce((sum, i) => sum + (i.right - i.left), 0);
        const arithmetic = `Its ${empty.items.length} items ask for ${asked.toFixed(1)}px `
          + `plus one gap, against `
          + `${(empty.contentRight - empty.contentLeft).toFixed(1)}px of content box`;
        expect(
          empty.stacked,
          liftsDiff
            ? `${where}: the row kept Diff on one row. ${arithmetic}`
            : `${where}: the row lifted Diff. ${arithmetic}`,
        ).toBe(liftsDiff);
        // Same vertical middle means one row. On the cell that overflows, the
        // lift is the point, so they must NOT share one.
        const middles = Math.abs(empty.diffMiddle! - empty.applyMiddle!);
        if (liftsDiff) {
          expect(middles, `${where}: Diff did not leave Apply's row`)
            .toBeGreaterThan(2);
        } else {
          expect(middles, `${where}: Diff and Apply are on different rows`)
            .toBeLessThan(2);
        }
        // The half that holds in EVERY cell, and the one that matters most:
        // lifted or not, nothing may cross the row's content edge.
        expectNoOverflow(empty, `${where}, empty draft`);

        // The empty row carries no clear-draft box. A clear button would be
        // one more 2.25rem box on the end of the cluster. Reserving one with
        // nothing to clear is what used to push Diff off the row at both
        // scales.
        expect(
          empty.leadingDesc,
          `${where}: an empty draft still reserves a clear button`,
        ).not.toContain('Clear draft');
        expect(
          empty.leadingDesc.length,
          `${where}: leading cluster is [${empty.leadingDesc.join(' | ')}]`,
        ).toBe(boxes);
        expect(empty.sendCount, `${where}: the banner owns the send slot`).toBe(0);

        // Promise 2: the first character mounts the clear button and the send
        // morph, and takes the banner away. Nothing already drawn moves.
        await (await waitForVisibleInput(page)).fill('x');
        const typed = await settledMetrics(page, `${where}, typed`, draftShowing);

        expect(typed.leadingDesc, `${where}: the clear button never arrived`)
          .toContain('Clear draft');
        expect(typed.leadingDesc.slice(0, boxes), `${where}: the cluster reordered`)
          .toEqual(empty.leadingDesc);
        expect(
          typed.iconLefts.slice(0, boxes),
          `${where}: the leading icons moved when the draft opened`,
        ).toEqual(empty.iconLefts.map(x => expect.closeTo(x, 1)));
        expect(typed.sendCount, `${where}: the send morph replaces the banner`).toBe(1);
        expectNoOverflow(typed, `${where}, one character typed`);
      } finally {
        cleanupCCThread(threadId, changeId, branch, file);
      }
    });
  }
});
