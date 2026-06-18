/**
 * End-to-end verification of the back/forward chevron HISTORY menu — the
 * long-press / right-click popover that lets the user jump several steps at once
 * instead of clicking Back N times (NavChevron + the nav-history-menu popover).
 *
 * Driven on the CONTENT-pane chevron because its history is buildable without
 * the LLM: each `/api/v1/ui/navigate` to a menu target routes through
 * switchMenuItem → pushNavState, so two navigations leave a real back-stack.
 * The thread-pane chevron renders the SAME NavChevron, so this exercises the
 * shared open → list → jump → close mechanism.
 *
 * Right-click (not a timed long-press) is used to open the menu: it dispatches
 * the same `contextmenu` trigger deterministically, with no hold-timer race.
 * The long-press timing path is covered by the unit test
 * (src/hooks/useLongPress.test.ts).
 *
 * Desktop-only (`-desktop.spec.ts`, chromium project): the content chevron lives
 * in the desktop PanelNav header group, and right-click is a desktop affordance.
 */
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, waitForEventStream, clickVisibleElement } from './helpers';

const CONTENT_TITLE = '.pane-header-content-title';
const HISTORY_OPTION = '.nav-history-menu .dropdown-option';

async function navigateMenu(page: import('@playwright/test').Page, target: string): Promise<void> {
  const res = await page.request.post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate {${target}} -> ${res.status()}`).toBeTruthy();
}

async function waitForContentTitle(page: import('@playwright/test').Page, text: string): Promise<void> {
  await expect(page.locator(`${CONTENT_TITLE}:visible`).first()).toContainText(text, { timeout: 10_000 });
}

test.describe('back chevron history menu', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('right-click on Back lists the back-history and jumps multiple steps at once', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);

    // Build a back-stack: Apps → Triggers → Notifications. Each navigate routes
    // through switchMenuItem → pushNavState, so Back now has two prior entries.
    await navigateMenu(page, 'apps');
    await waitForContentTitle(page, 'Apps');
    await navigateMenu(page, 'triggers');
    await waitForContentTitle(page, 'Triggers');
    await navigateMenu(page, 'notifications');
    await waitForContentTitle(page, 'Notifications');

    // Right-click the Back chevron → the history popover opens.
    const backBtn = page.locator('.content-back-btn:visible').first();
    await expect(backBtn).toBeEnabled();
    await backBtn.click({ button: 'right' });

    // The menu lists the destinations Back walks toward, nearest-first:
    // Triggers (one step back) above Apps (two steps back).
    await page.waitForFunction((sel) => {
      const opts = Array.from(document.querySelectorAll(sel));
      return opts.some(el => (el.textContent ?? '').includes('Triggers'))
        && opts.some(el => (el.textContent ?? '').includes('Apps'));
    }, HISTORY_OPTION, { timeout: 10_000 });

    // `useAnchoredPosition` computes the popover's fixed offset in a post-mount
    // requestAnimationFrame; until it lands, NavChevron renders the menu
    // `visibility:hidden` at its natural flow position (the bottom of the <body>
    // portal — off-screen at the viewport's bottom edge). The option-text wait
    // above resolves at MOUNT — before that rAF — so measuring the geometry
    // immediately raced the position effect and, under full-suite host load,
    // intermittently captured the still-unpositioned menu. Wait for the menu to
    // actually settle (anchored under the button AND inside the viewport) before
    // measuring. A genuinely trapped/off-screen menu (the transformed-ancestor
    // bug below) never settles, so this times out and the guard still bites.
    await page.waitForFunction((sel) => {
      const btn = Array.from(document.querySelectorAll('.content-back-btn'))
        .find(el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }) as HTMLElement | undefined;
      const menu = document.querySelector(sel) as HTMLElement | null;
      if (!btn || !menu) return false;
      const b = btn.getBoundingClientRect();
      const m = menu.getBoundingClientRect();
      const inViewport = m.left >= -1 && m.top >= -1
        && m.right <= window.innerWidth + 1 && m.bottom <= window.innerHeight + 1;
      return inViewport
        && Math.abs(m.left - b.left) <= 8
        && Math.abs(m.top - b.bottom) <= 12;
    }, '.nav-history-menu', { timeout: 10_000 });

    // Regression guard for the transformed-ancestor / position:fixed containing
    // block bug: the chevrons live in `.app-header` regions that carry a
    // `transform`, which (without the <body> portal) would trap the fixed-
    // positioned popover and render it far from the button / off-screen. Assert
    // the menu is in the viewport AND anchored under the Back button. A plain
    // "is visible" check would NOT catch this — an off-screen element still
    // reports width/height > 0.
    const geom = await page.evaluate((sel) => {
      // Dual-layout: there's a hidden mobile-header copy of the chevron at
      // (0,0). Measure the VISIBLE one — the same element the menu anchored to.
      const btn = Array.from(document.querySelectorAll('.content-back-btn'))
        .find(el => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }) as HTMLElement | undefined;
      const menu = document.querySelector(sel) as HTMLElement | null;
      if (!btn || !menu) return null;
      const b = btn.getBoundingClientRect();
      const m = menu.getBoundingClientRect();
      return {
        b, m,
        inViewport: m.left >= -1 && m.top >= -1
          && m.right <= window.innerWidth + 1 && m.bottom <= window.innerHeight + 1,
      };
    }, '.nav-history-menu');
    expect(geom, 'found Back button + menu rects').not.toBeNull();
    expect(geom!.inViewport, `menu rect ${JSON.stringify(geom!.m)} within viewport`).toBeTruthy();
    // computeAnchorPosition left-aligns the menu to the anchor (clamped) — so the
    // menu's left tracks the button's left to within a few px. With the
    // transform bug the menu's left was offset by the ancestor's origin.
    expect(Math.abs(geom!.m.left - geom!.b.left)).toBeLessThanOrEqual(8);
    // And the menu sits just below the button (or flips above if no room) — its
    // top edge is near the button's bottom, not hundreds of px adrift.
    expect(Math.abs(geom!.m.top - geom!.b.bottom)).toBeLessThanOrEqual(12);

    // Jump two steps back in one click — straight to Apps.
    const jumped = await clickVisibleElement(page, HISTORY_OPTION, 'Apps');
    expect(jumped, 'history option "Apps" was clickable').toBeTruthy();

    // The content pane lands on Apps...
    await waitForContentTitle(page, 'Apps');

    // ...and the popover closes (no visible history options remain).
    await page.waitForFunction((sel) => {
      return !Array.from(document.querySelectorAll(sel)).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, HISTORY_OPTION, { timeout: 5_000 });

    // Forward is now available (we stepped back), and the Forward chevron's
    // history lists the entries ahead — Triggers then Notifications.
    const fwdBtn = page.locator('.content-forward-btn:visible').first();
    await expect(fwdBtn).toBeEnabled();
    await fwdBtn.click({ button: 'right' });
    await page.waitForFunction((sel) => {
      const opts = Array.from(document.querySelectorAll(sel));
      return opts.some(el => (el.textContent ?? '').includes('Triggers'))
        && opts.some(el => (el.textContent ?? '').includes('Notifications'));
    }, HISTORY_OPTION, { timeout: 10_000 });

    // Jump forward two steps to Notifications.
    const jumpedFwd = await clickVisibleElement(page, HISTORY_OPTION, 'Notifications');
    expect(jumpedFwd, 'forward history option "Notifications" was clickable').toBeTruthy();
    await waitForContentTitle(page, 'Notifications');
  });
});
