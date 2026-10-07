/**
 * A step row's context counter takes a tap that lands near it, on a phone.
 *
 * The counter is a short percentage pinned to the row's right end, about 20px
 * wide. Without a reach, the space on both sides takes no tap: the row gap and
 * margin on the left, the turn inset and pane gutter on the right. A tap there
 * lands on bare transcript and opens nothing, and on the right it reads as an
 * edge gesture.
 *
 * On touch the counter carries a transparent reach over that dead space
 * (`.step-context::before` in steps.css). It stays inside its own row, so a
 * near miss can never open a neighbouring row's context.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, disarmFollowSeed, ensureMobileView, navigateToApp, revealSteps } from './helpers';
import { psql, seedThreadOfCounters } from './db-helpers';

test.use({ viewport: { width: 393, height: 852 } });

const CONTEXT_MODAL = '[data-role="context-captured-modal"]';
const SCALES = [['100%', 16], ['112.5%', 18]] as const;

/** Name what a tap at (x, y) reaches. A counter is named by its row's step,
 *  so a probe can tell its own row from a neighbour's. */
const DESCRIBE = `(x, y) => {
  const el = document.elementFromPoint(x, y);
  if (!el) return 'nothing';
  const counter = el.closest('[data-role="step-context"]');
  if (counter) {
    const row = counter.closest('[data-role="inline-step"]');
    return 'counter of ' + row.querySelector('.step-description').textContent;
  }
  if (el.closest('[data-role="step-main"]')) return 'step-main';
  const control = el.closest('button, a, [role="button"]');
  if (control) return control.getAttribute('aria-label') || control.className;
  return 'bare ' + el.tagName.toLowerCase() + '.' + String(el.className).split(' ')[0];
}`;

interface Box { left: number; right: number; top: number; bottom: number }

interface Probe {
  step: string;
  row: Box;
  main: Box;
  /** The drawn percentage, which must not move. */
  text: Box;
  /** The span around the counter's centre where a tap still reaches it. */
  hit: Box;
  /** The pane's right edge, which the reach must stop short of. */
  paneRight: number;
  /** What a tap at each probed point reaches, keyed by where it is. */
  at: Record<string, string>;
}

/** Scroll the counter nearest the pane's middle to the middle, clear of both
 *  scroll chevrons, and probe around it. */
async function probeMiddleCounter(page: Page, offsets: number[]): Promise<Probe> {
  return page.evaluate(({ offsets, describeSrc }) => {
    const describe = new Function(`return ${describeSrc}`)() as (x: number, y: number) => string;
    const scroller = document.querySelector('.mobile-swipe-pane .thread-content') as HTMLElement;
    const counters = [...document.querySelectorAll('.mobile-swipe-pane [data-role="step-context"]')] as HTMLElement[];
    const centre = (el: Element) => { const r = el.getBoundingClientRect(); return (r.top + r.bottom) / 2; };
    const mid = innerHeight / 2;
    const counter = counters.reduce((a, c) => (Math.abs(centre(c) - mid) < Math.abs(centre(a) - mid) ? c : a));
    scroller.scrollTop += centre(counter) - mid;

    const rect = (el: Element) => {
      const r = el.getBoundingClientRect();
      return { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
    };
    const row = counter.closest('[data-role="inline-step"]') as HTMLElement;
    const text = [...counter.querySelectorAll('span')].find(s => s.getBoundingClientRect().width > 0)!;
    const box = rect(text);
    const cx = (box.left + box.right) / 2;
    const cy = (box.top + box.bottom) / 2;
    const reaches = (x: number, y: number) => document.elementFromPoint(x, y)?.closest('[data-role="step-context"]') === counter;
    // How far the reach runs from the centre, bisected to well under a
    // hundredth of a pixel. A coarse step would undercount every edge.
    const walk = (dx: number, dy: number) => {
      let inside = 0;
      let outside = 120;
      if (reaches(cx + dx * outside, cy + dy * outside)) return outside;
      while (outside - inside > 1 / 128) {
        const d = (inside + outside) / 2;
        if (reaches(cx + dx * d, cy + dy * d)) inside = d;
        else outside = d;
      }
      return inside;
    };
    const main = rect(row.querySelector('[data-role="step-main"]')!);
    const rowBox = rect(row);
    const eighthRem = parseFloat(getComputedStyle(document.documentElement).fontSize) / 8;
    const at: Record<string, string> = {
      // The reach stops an eighth of a rem short of the step's own target,
      // the margin WebKit's wide left-edge hit test needs.
      'gap start': describe(main.right + eighthRem + 0.5, cy),
      'gap middle': describe((main.right + box.left) / 2, cy),
      // Above and below the drawn text, inside the row's line. The reach
      // sits a fraction of a pixel off centre, so these keep a pixel of slack.
      'row top': describe(cx, rowBox.top + 3),
      'row bottom': describe(cx, rowBox.bottom - 3),
      'main end': describe(main.right - 1, cy),
    };
    for (const d of offsets) {
      at[`left+${d}`] = describe(box.left - d, cy);
      at[`right+${d}`] = describe(box.right + d, cy);
    }
    return {
      step: row.querySelector('.step-description')!.textContent!,
      row: rowBox,
      main,
      text: box,
      hit: { left: cx - walk(-1, 0), right: cx + walk(1, 0), top: cy - walk(0, -1), bottom: cy + walk(0, 1) },
      paneRight: scroller.getBoundingClientRect().right,
      at,
    };
  }, { offsets, describeSrc: DESCRIBE });
}

async function setScale(page: Page, scale: string, rootPx: number): Promise<void> {
  await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', s), scale);
  await expect
    .poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
      { timeout: 5_000, message: `the root never settled at ui-scale ${scale}` })
    .toBe(`${rootPx}px`);
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await disarmFollowSeed(page);
  await page.addInitScript((tid: string) => localStorage.setItem('lucidos-focused-thread', tid), threadId);
  await navigateToApp(page);
  await page.waitForFunction(() => localStorage.getItem('lucidos-ui-scale') !== null, undefined, { timeout: 10_000 });
  await ensureMobileView(page, 'thread');
  await revealSteps(page);
  await expect(page.locator('.mobile-swipe-pane [data-role="step-context"]').first()).toBeVisible({ timeout: 15_000 });
}

const fmt = (b: Box) => `${(b.right - b.left).toFixed(1)}x${(b.bottom - b.top).toFixed(1)} at (${b.left.toFixed(1)}, ${b.top.toFixed(1)})`;

test.describe('The step context counter takes a near miss on a phone', () => {
  const seeded: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test.afterEach(() => {
    if (seeded.length === 0) return;
    const ids = seeded.splice(0).map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('the hit box covers the dead space beside the counter, and only its own row', async ({ page }, info) => {
    const threadId = seedThreadOfCounters({ turns: 3, stepsPerTurn: 20, title: 'E2E step counter reach' });
    seeded.push(threadId);
    await openThread(page, threadId);

    for (const [scale, rootPx] of SCALES) {
      await setScale(page, scale, rootPx);
      const offsets = [2, 4, 8].map(d => (d * rootPx) / 16);
      const p = await probeMiddleCounter(page, offsets);
      const where = `at ${scale}`;
      const own = `counter of ${p.step}`;
      console.log(`[${info.project.name}] ${where}: text ${fmt(p.text)}, row ${fmt(p.row)}, `
        + `hit ${fmt(p.hit)}, pane right ${p.paneRight}\n  ${JSON.stringify(p.at)}`);

      // The drawn percentage did not move: it still ends at the row's right
      // edge, 0.625rem after the step's own target (the row gap plus the
      // counter's margin).
      expect(p.text.right, `${where}: the percentage left the row's right end`).toBeCloseTo(p.row.right, 0);
      expect(p.text.left - p.main.right, `${where}: the gap before the counter changed`)
        .toBeCloseTo(0.625 * rootPx, 0);

      // At least 44px wide at the 16px root, scaled with it.
      expect(p.hit.right - p.hit.left, `${where}: hit width`).toBeGreaterThanOrEqual((44 * rootPx) / 16);
      // Never past the pane, where it would cover the edge-swipe strip's job.
      expect(p.hit.right, `${where}: the reach left the pane`).toBeLessThanOrEqual(p.paneRight);

      for (const key of ['gap start', 'gap middle', 'row top', 'row bottom']) {
        expect(p.at[key], `${where}: a tap at the ${key}`).toBe(own);
      }
      for (const d of offsets) {
        expect(p.at[`left+${d}`], `${where}: a tap ${d}px left of the percentage`).toBe(own);
        expect(p.at[`right+${d}`], `${where}: a tap ${d}px right of the percentage`).toBe(own);
      }
      // The step's own target keeps every pixel it had.
      expect(p.at['main end'], `${where}: the step's own target lost its end`).toBe('step-main');
      // The reach stays inside its row, so a neighbour's tap stays the
      // neighbour's: one row up and one row down each reach their own counter.
      expect(p.hit.top, `${where}: the reach rose into the row above`).toBeGreaterThanOrEqual(p.row.top - 0.5);
      expect(p.hit.bottom, `${where}: the reach fell into the row below`).toBeLessThanOrEqual(p.row.bottom + 0.5);
    }
  });

  test('a real tap beside the percentage opens that row\'s context', async ({ page }) => {
    const threadId = seedThreadOfCounters({ turns: 3, stepsPerTurn: 20, title: 'E2E step counter tap' });
    seeded.push(threadId);
    await openThread(page, threadId);

    for (const [scale, rootPx] of SCALES) {
      await setScale(page, scale, rootPx);
      for (const side of ['gap', 'right'] as const) {
        const p = await probeMiddleCounter(page, []);
        const cy = (p.text.top + p.text.bottom) / 2;
        const x = side === 'gap' ? (p.main.right + p.text.left) / 2 : p.text.right + (6 * rootPx) / 16;
        const where = `at ${scale}, tapped ${side === 'gap' ? 'in the gap before' : 'just right of'} the percentage`;

        await page.touchscreen.tap(x, cy);
        const modal = page.locator(CONTEXT_MODAL);
        await expect(modal, `${where}: the context viewer did not open`).toHaveCount(1, { timeout: 5_000 });
        await expect(modal.locator('.step-detail-description'), `${where}: it opened another row's context`)
          .toHaveText(p.step);
        await modal.locator('[data-role="surface-close"]').click();
        await expect(modal).toHaveCount(0);
      }
    }
  });
});
