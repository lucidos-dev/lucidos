import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

// The thread drawer used to carry TWO competing focuses: the ↑/↓ "highlight"
// (a signal) and native Tab focus on each row's pin/⋯ buttons — Enter could act
// on either. The fix collapses them into ONE: the drawer is a single tab stop
// (role="tree", tabindex=0) whose `aria-activedescendant` points at the
// highlighted row, the per-row buttons leave the Tab order (tabindex=-1), and
// the keyboard reaches row actions through the ⋯ menu via the customizable
// "Open thread actions" shortcut.
//
// Desktop-only (`-desktop.spec.ts`): the drawer's keyboard list-nav + focused
// pane model is desktop-only (mobile uses a dedicated threads pane and navigates,
// not focuses). The mobile Playwright projects exclude this file via testIgnore.
test.describe('Thread drawer — single keyboard focus (aria-activedescendant)', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.clearCookies();
    // Archive must be expanded (seeded rows land there) — a survivor collapse
    // from another test would hide them.
    await context.addInitScript(() => {
      localStorage.removeItem('lucidos-drawer-collapsed');
    });
    clearAllThreads();
  });

  test('per-row action buttons are not in the Tab order (single tab stop)', async ({ page }) => {
    const id = randomUUID();
    psql(seedThreadRow({ id, title: `solo-${Date.now()}`, now: new Date().toISOString() }));

    await navigateToApp(page);
    await openThreadDrawer(page);

    const row = page.locator(`.thread-row[data-thread-nav="${id}"]`).first();
    await expect(row).toBeVisible();
    // Mouse-only: present in the DOM, but removed from the Tab order so the
    // drawer stays one tab stop.
    await expect(row.locator('.pin-thread-btn')).toHaveAttribute('tabindex', '-1');
    await expect(row.locator('button[aria-haspopup="menu"]')).toHaveAttribute('tabindex', '-1');
  });

  test('focusing the drawer sets aria-activedescendant; ↓ moves it (= the highlight); Tab exits', async ({ page }) => {
    const t = Date.now();
    const idA = randomUUID();
    const idB = randomUUID();
    psql([
      seedThreadRow({ id: idA, title: `aaa-${t}`, now: new Date(t).toISOString() }),
      seedThreadRow({ id: idB, title: `bbb-${t}`, now: new Date(t - 1000).toISOString() }),
    ].join(';\n'));

    await navigateToApp(page);
    await openThreadDrawer(page);

    const drawer = page.locator('.thread-drawer:not(.thread-drawer-collapsed)').first();

    // ⌘⇧1 / Ctrl+Shift+1 — the focus-aware drawer toggle focuses the container
    // and seeds the highlight, so the container (not a row) holds DOM focus.
    await page.keyboard.press('Control+Shift+1');
    await expect(drawer).toBeFocused();
    await expect(drawer).toHaveAttribute('aria-activedescendant', /.+/);

    // ↓ lands on a thread row: the active-descendant id and the visually
    // highlighted row are the SAME element — one focus, not two.
    await page.keyboard.press('ArrowDown');
    const highlighted = page.locator('.thread-row.thread-row-highlighted').first();
    await expect(highlighted).toBeVisible();
    const activeDesc = await drawer.getAttribute('aria-activedescendant');
    const highlightedId = await highlighted.getAttribute('id');
    expect(activeDesc).toBe(highlightedId);
    // DOM focus never moved onto a row/button — it stays on the container.
    await expect(drawer).toBeFocused();

    // Tab leaves the drawer entirely (it is a single tab stop) — no row button
    // grabs focus.
    await page.keyboard.press('Tab');
    const focusInDrawer = await page.evaluate(() => {
      const d = document.querySelector('.thread-drawer:not(.thread-drawer-collapsed)');
      return !!d && !!document.activeElement && d.contains(document.activeElement);
    });
    expect(focusInDrawer).toBe(false);
  });

  test('the "Open thread actions" shortcut opens the highlighted row\'s ⋯ menu (with Pin)', async ({ page }) => {
    const id = randomUUID();
    psql(seedThreadRow({ id, title: `menu-${Date.now()}`, now: new Date().toISOString() }));

    await navigateToApp(page);
    await openThreadDrawer(page);

    await page.keyboard.press('Control+Shift+1');
    // Move off the Archive section header onto the (only) thread row.
    await page.keyboard.press('ArrowDown');
    await expect(
      page.locator(`.thread-row.thread-row-highlighted[data-thread-nav="${id}"]`),
    ).toBeVisible();

    // ⌘⇧M / Ctrl+Shift+M opens that row's overflow menu — the keyboard route to
    // every per-row action. Pin/Unpin lives in the menu, so it is the complete
    // action surface.
    await page.keyboard.press('Control+Shift+M');
    const menu = page.locator('.thread-overflow-menu:visible');
    await expect(menu).toBeVisible();
    await expect(menu.getByText(/Pin thread|Unpin thread/)).toBeVisible();
  });
});
