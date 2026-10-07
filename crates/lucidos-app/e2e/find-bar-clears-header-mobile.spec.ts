/**
 * The transcript's find bar stays reachable on a phone, wherever the header is.
 *
 * The header is fixed over the transcript, and the transcript's box starts at
 * viewport y=0. In flow, the bar sat behind the shown header, and under the
 * Dynamic Island once the header slid away. It now floats on the header's
 * visible bottom edge, floored at the safe area (styles/mobile.css).
 *
 * Playwright reports no safe area, so the island is stamped through the
 * safe-area floor, as header-icon-hit-target-mobile.spec.ts does.
 */
import { test, expect, type Page } from './fixtures';
import {
  assertHealthy, disableMobileDynamicBars, disarmFollowSeed, enableMobileDynamicBars, ensureMobileView,
  navigateToApp,
} from './helpers';
import { psql, seedChatThread } from './db-helpers';

test.use({ viewport: { width: 393, height: 852 } });

/** An iPhone's status bar band with the Dynamic Island, in px. */
const ISLAND_PX = 54;

interface Geometry {
  barTop: number;
  /** Where the header's visible part ends: 0 once it has slid away. */
  headerBottom: number;
  /** The bar's controls a tap on their centre does not reach, by name. */
  covered: string[];
}

async function geometry(page: Page): Promise<Geometry | null> {
  return page.evaluate(() => {
    const bar = document.querySelector('.mobile-swipe-pane .thread-view > .find-bar-slot');
    const header = document.querySelector('.app-header');
    if (!bar || !header) return null;
    const controls = [...bar.querySelectorAll<HTMLElement>('[data-role="find-input"], button')];
    const covered = controls.filter((el) => {
      const r = el.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return !hit || !el.contains(hit);
    }).map((el) => el.getAttribute('aria-label') ?? el.dataset.role ?? el.tagName);
    return {
      barTop: bar.getBoundingClientRect().top,
      headerBottom: Math.max(0, header.getBoundingClientRect().bottom),
      covered,
    };
  });
}

/** Wait until the bar rests on the header's visible bottom edge, floored at
 *  the island, with every control reachable. Polls out the bars' glide. */
async function expectBarClearsChrome(page: Page, where: string): Promise<Geometry> {
  let last: Geometry | null = null;
  await expect.poll(async () => {
    last = await geometry(page);
    if (!last) return 'no find bar';
    const rest = Math.max(ISLAND_PX, last.headerBottom);
    if (Math.abs(last.barTop - rest) > 1) return `bar at ${last.barTop}, chrome ends at ${rest}`;
    return last.covered.length === 0 ? 'clear' : `covered: ${last.covered.join(', ')}`;
  }, { message: where, timeout: 10_000 }).toBe('clear');
  return last!;
}

test.describe('The transcript find bar on a phone', () => {
  const seeded: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await enableMobileDynamicBars(page);
  });

  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
    if (seeded.length === 0) return;
    const ids = seeded.splice(0).map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('clears the header and the Dynamic Island as the header comes and goes', async ({ page }) => {
    const threadId = seedChatThread({ turns: 40, needles: [3], title: 'Find bar on a phone' });
    seeded.push(threadId);
    await disarmFollowSeed(page);
    await page.addInitScript((tid: string) => localStorage.setItem('lucidos-focused-thread', tid), threadId);
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');
    await expect(page.locator('.mobile-swipe-pane .thread-content')).toContainText('Answer 39.', { timeout: 15_000 });
    await page.evaluate((px) => {
      document.documentElement.style.setProperty('--safe-area-floor-top', `${px}px`);
    }, ISLAND_PX);

    await page.locator('.mobile-thread-title-row .thread-title-menu:visible').click();
    await page.locator('.thread-overflow-menu').getByRole('menuitem', { name: 'Find in thread' }).click();
    const field = page.locator('.mobile-swipe-pane [data-role="find-input"]');
    await expect(field).toBeFocused();

    // The field takes the keyboard, which sends the header away.
    const typing = await expectBarClearsChrome(page, 'while typing, the bar sits under the island');
    expect(typing.headerBottom, 'the keyboard did not send the header away').toBe(0);

    // The keyboard closes and the header returns. The bar rides below it.
    await field.evaluate((el) => (el as HTMLElement).blur());
    const shown = await expectBarClearsChrome(page, 'with the header shown, the bar sits below it');
    expect(shown.headerBottom, 'the header did not return').toBeGreaterThan(ISLAND_PX);

    // The reader scrolls down from mid-thread, and the header slides away
    // again. Spaced steps, the way a drag scrolls: one jump is one event, which
    // can land in the app's own navigation window and be ignored.
    await page.locator('.mobile-swipe-pane .thread-content').evaluate(async (el) => {
      el.scrollTop = (el.scrollHeight - el.clientHeight) / 2;
      for (let i = 0; i < 8; i++) {
        await new Promise((r) => setTimeout(r, 80));
        el.scrollTop += 80;
      }
    });
    const away = await expectBarClearsChrome(page, 'with the header scrolled away, the bar sits under the island');
    expect(away.headerBottom, 'the scroll did not send the header away').toBe(0);
  });
});
