/**
 * The follow toggle floats in the down chevron's old slot, over the composer's
 * top edge. The chevron stands one slot over it while the reader is scrolled
 * up, and the magnet never moves.
 *
 * A browser test, because each claim is a measurement. The two circles share a
 * right edge and a size. The chevron sits one stack gap over the magnet. At the
 * live edge the last turn ends above the magnet.
 *
 * Plan: `docs/plans/2026-10-28-the-follow-toggle-floats-with-the-down-chevron.md`.
 */
import { test, expect, type Page } from './fixtures';
import {
  assertHealthy,
  disableMobileDynamicBars,
  disarmFollowSeed,
  enableMobileDynamicBars,
  ensureMobileView,
  hideDashboardRail,
  isMobileViewport,
  navigateToApp,
  waitForScrollSettled,
} from './helpers';
import { psql, seedChatThread } from './db-helpers';

interface Box { left: number; right: number; top: number; bottom: number }

interface Geometry {
  magnet: Box;
  chevron: Box;
  chevronShown: boolean;
  /** The bottom edge of the feed's last child. */
  lastTurnBottom: number;
}

/** The visible transcript's geometry, read in one frame. */
function geometry(page: Page): Promise<Geometry | null> {
  return page.evaluate(() => {
    const shown = (el: Element) => el.getBoundingClientRect().height > 0;
    const scroller = [...document.querySelectorAll('.thread-content.visible')].find(shown);
    const magnet = [...document.querySelectorAll('.prompt-area > [data-role="follow-live-edge"]')].find(shown);
    const chevron = scroller?.parentElement?.querySelector(':scope > .scroll-to-bottom');
    const last = scroller?.querySelector('.thread-feed')?.lastElementChild;
    if (!scroller || !magnet || !chevron || !last) return null;
    const box = (el: Element) => {
      const r = el.getBoundingClientRect();
      return { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
    };
    return {
      magnet: box(magnet),
      chevron: box(chevron),
      chevronShown: chevron.classList.contains('visible'),
      lastTurnBottom: last.getBoundingClientRect().bottom,
    };
  });
}

/** A CSS length in px, resolved at the root. */
function lengthPx(page: Page, value: string): Promise<number> {
  return page.evaluate((value) => {
    const probe = document.createElement('div');
    probe.style.position = 'absolute';
    probe.style.width = value;
    document.body.appendChild(probe);
    const w = probe.getBoundingClientRect().width;
    probe.remove();
    return w;
  }, value);
}

/** The bottom is one jump. The middle is reached in spaced steps, the way a
 *  drag scrolls, so dynamic bars bring the prompt back on the way up. One jump
 *  can land in the app's navigation window and be ignored. */
async function scrollTranscript(page: Page, to: 'bottom' | 'middle'): Promise<void> {
  await page.evaluate(async (to) => {
    const el = [...document.querySelectorAll<HTMLElement>('.thread-content.visible')]
      .find((c) => c.getBoundingClientRect().height > 0)!;
    const end = el.scrollHeight - el.clientHeight;
    if (to === 'bottom') {
      el.scrollTop = end;
      return;
    }
    const middle = Math.round(end / 2);
    for (let step = 0; step < 100 && el.scrollTop > middle + 1; step++) {
      el.scrollTop = Math.max(middle, el.scrollTop - 80);
      await new Promise((r) => setTimeout(r, 80));
    }
  }, to);
  await waitForScrollSettled(page);
}

const fmt = (b: Box) => `${(b.right - b.left).toFixed(1)}x${(b.bottom - b.top).toFixed(1)} at (${b.left.toFixed(1)}, ${b.top.toFixed(1)})`;

test.describe('the follow toggle floats with the down chevron', () => {
  const seeded: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await disarmFollowSeed(page);
    await hideDashboardRail(page);
  });

  test.afterEach(async ({ page }) => {
    if (isMobileViewport(page)) await disableMobileDynamicBars(page);
    if (seeded.length === 0) return;
    const ids = seeded.splice(0).map((id) => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  async function openThread(page: Page): Promise<void> {
    const threadId = seedChatThread({ turns: 30, needles: [], title: 'E2E follow toggle slot' });
    seeded.push(threadId);
    await page.addInitScript((id) => {
      localStorage.setItem('lucidos-focused-thread', id);
      localStorage.setItem('lucidos-split-ratio', '1');
    }, threadId);
    await navigateToApp(page);
    await page.evaluate((tid) => { location.hash = `#thread=${tid}`; }, threadId);
    await ensureMobileView(page, 'thread');
    await expect.poll(() => geometry(page).then((g) => g !== null), { timeout: 15_000 }).toBe(true);
  }

  async function checkSlots(page: Page, where: string): Promise<void> {
    await scrollTranscript(page, 'bottom');
    await expect.poll(() => geometry(page).then((g) => g?.chevronShown), { message: `${where}: chevron at the live edge` })
      .toBe(false);
    const edge = (await geometry(page))!;
    console.log(`${where} live edge: magnet ${fmt(edge.magnet)}, last turn ends at ${edge.lastTurnBottom.toFixed(1)}`);
    expect(edge.lastTurnBottom, `${where}: the last turn runs under the magnet`).toBeLessThanOrEqual(edge.magnet.top + 0.5);

    await scrollTranscript(page, 'middle');
    await expect.poll(() => geometry(page).then((g) => g?.chevronShown), { message: `${where}: chevron scrolled up` })
      .toBe(true);
    const up = (await geometry(page))!;
    const gap = await lengthPx(page, 'var(--scroll-chevron-stack-gap)');
    console.log(`${where} scrolled up: chevron ${fmt(up.chevron)}, magnet ${fmt(up.magnet)}, gap ${gap}`);
    expect(up.magnet, `${where}: the magnet moved when the chevron came`).toEqual(edge.magnet);
    expect(up.chevron.right, `${where}: right edges differ`).toBeCloseTo(up.magnet.right, 0);
    expect(up.chevron.right - up.chevron.left, `${where}: sizes differ`).toBeCloseTo(up.magnet.right - up.magnet.left, 0);
    expect(up.magnet.top - up.chevron.bottom, `${where}: the stack gap`).toBeCloseTo(gap, 0);
  }

  test('stands under the chevron, and the last turn clears it at the live edge', async ({ page }) => {
    await openThread(page);
    await checkSlots(page, isMobileViewport(page) ? 'phone' : 'desktop');
  });

  test('keeps both claims under the phone\'s dynamic bars', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'dynamic bars are a phone layout');
    await enableMobileDynamicBars(page);
    await openThread(page);
    await checkSlots(page, 'dynamic bars');
  });

  test('stays one slot under the chevron when the prompt slides away over a home indicator', async ({ page }) => {
    test.skip(!isMobileViewport(page), 'dynamic bars are a phone layout');
    await enableMobileDynamicBars(page);
    await openThread(page);
    // Stand in for an iPhone's home indicator, which the emulators lack.
    await page.evaluate(() => document.documentElement.style.setProperty('--safe-area-floor-bottom', '34px'));
    await page.evaluate(async () => {
      const el = [...document.querySelectorAll<HTMLElement>('.thread-content.visible')]
        .find((c) => c.getBoundingClientRect().height > 0)!;
      el.scrollTop = 0;
      await new Promise((r) => setTimeout(r, 300));
      // Spaced steps, the way a drag scrolls (see dynamic-bars-prompt-mobile.spec.ts).
      for (let i = 0; i < 10; i++) {
        el.scrollTop += 80;
        await new Promise((r) => setTimeout(r, 80));
      }
    });
    const promptAway = () => page.evaluate(() => {
      const prompt = document.querySelector('.mobile-swipe-pane .prompt-area')!;
      return prompt.getBoundingClientRect().bottom - prompt.closest('.thread-pane')!.getBoundingClientRect().bottom;
    });
    await expect.poll(promptAway, { message: 'the prompt slid away' }).toBeGreaterThan(34);
    await waitForScrollSettled(page);
    await expect.poll(() => geometry(page).then((g) => g?.chevronShown)).toBe(true);
    const g = (await geometry(page))!;
    const gap = await lengthPx(page, 'var(--scroll-chevron-stack-gap)');
    console.log(`home indicator, prompt away: chevron ${fmt(g.chevron)}, magnet ${fmt(g.magnet)}, gap ${gap}`);
    expect(g.magnet.top - g.chevron.bottom, 'the stack gap, with the prompt away').toBeCloseTo(gap, 0);
  });

  test('stands over the composer in the compose view, where no chevron is', async ({ page }) => {
    await navigateToApp(page);
    const magnet = page.locator('.prompt-area > [data-role="follow-live-edge"]:visible');
    await expect(magnet).toHaveCount(1);
    await expect(magnet).toHaveAttribute('aria-pressed', 'false');
    const [m, box] = await Promise.all([
      magnet.boundingBox(),
      page.locator('.prompt-area:visible .prompt-box').first().boundingBox(),
    ]);
    expect(m!.y + m!.height, 'the magnet sits on the prompt box').toBeLessThan(box!.y);
    await expect(page.locator('.scroll-to-bottom.visible')).toHaveCount(0);
  });
});
