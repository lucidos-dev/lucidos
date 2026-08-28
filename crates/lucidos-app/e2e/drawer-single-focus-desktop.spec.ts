import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy, waitForEventStream } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** Open the drawer on a page that has finished connecting.
 *
 *  Every assertion below reads rows the SSE connect brings in. Open the drawer
 *  ahead of that and the list is still filling, so the highlight walk and the
 *  overflow check both race the render. */
async function openSettledDrawer(page: Page): Promise<void> {
  await navigateToApp(page);
  await waitForEventStream(page);
  await openThreadDrawer(page);
}

/** Walk the keyboard highlight down onto `id`.
 *
 *  A single press does not reach it, and no spec can arrange a drawer holding
 *  only its own row. `clearAllThreads` truncates the projection, and a thread an
 *  earlier spec left alive rewrites its own row seconds later from its next
 *  event. The row comes back UNTITLED, because the title went with the truncate,
 *  and it lands in Current, which the drawer draws above Archive.
 *
 *  So walk. The seeded row is the newest archived one, so it is the last section's
 *  first row and the walk always reaches it. Reads `aria-activedescendant`, the
 *  one place the highlight lives, so a walk that runs out says which node it
 *  stopped on.
 *
 *  One press per poll, and the interval is pinned flat because the default one
 *  backs off to a second. The walk would then be budgeted in presses rather than
 *  in time, and how many rows come back is not something it can know. */
async function highlightRow(page: Page, id: string): Promise<void> {
  await expect.poll(async () => {
    await page.keyboard.press('ArrowDown');
    return await page.evaluate(() => document
      .querySelector('.thread-drawer:not(.thread-drawer-collapsed)')
      ?.getAttribute('aria-activedescendant') ?? '<none>');
  }, {
    intervals: [50],
    message: 'the highlight never reached the seeded row',
  }).toBe(`drawer-nav-${id}`);
}

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

    await openSettledDrawer(page);

    const row = page.locator(`.thread-row[data-thread-nav="${id}"]`).first();
    await expect(row).toBeVisible();
    // Mouse-only: present in the DOM, but removed from the Tab order so the
    // drawer stays one tab stop.
    await expect(row.locator('.pin-thread-btn')).toHaveAttribute('tabindex', '-1');
    await expect(row.locator('button[aria-haspopup="menu"]')).toHaveAttribute('tabindex', '-1');
  });

  test('focusing the drawer sets aria-activedescendant; ↓ moves it (= the highlight); Tab exits', async ({ page }) => {
    // Enough rows to overflow the list, because the Tab assertion below only
    // bites on a list that scrolls. Chromium hands a scroll container its own
    // tab stop once it has somewhere to scroll to, and two rows never did. That
    // is why this read as flaky: it failed only after a neighbouring spec left
    // threads behind, and passed alone.
    const t = Date.now();
    const seeded = Array.from({ length: 30 }, (_, i) => ({
      id: randomUUID(),
      title: `row-${t}-${String(i).padStart(2, '0')}`,
      now: new Date(t - i * 1000).toISOString(),
    }));
    psql(seeded.map(seedThreadRow).join(';\n'));

    await openSettledDrawer(page);

    const drawer = page.locator('.thread-drawer:not(.thread-drawer-collapsed)').first();

    // The premise of the Tab assertion. A list that stopped overflowing would
    // pass it for the wrong reason and guard nothing.
    await expect
      .poll(async () => await page.evaluate(() => {
        const l = document.querySelector('.thread-drawer-list');
        return l ? l.scrollHeight - l.clientHeight : -1;
      }), { message: 'the drawer list never overflowed' })
      .toBeGreaterThan(0);

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
    //
    // Names the node it stopped on rather than answering yes or no. `contains`
    // is true of the drawer itself. A Tab that moved nothing would otherwise
    // read as a row button stealing focus, and the two have different causes.
    await page.keyboard.press('Tab');
    const stopped = await page.evaluate(() => {
      const d = document.querySelector('.thread-drawer:not(.thread-drawer-collapsed)');
      const a = document.activeElement;
      if (!d || !a || !d.contains(a)) return 'outside';
      if (a === d) return 'the drawer itself (Tab moved nothing)';
      const cls = typeof a.className === 'string' ? a.className : '';
      return `${a.tagName.toLowerCase()}${cls ? `.${cls.trim().split(/\s+/).join('.')}` : ''}`;
    });
    expect(stopped).toBe('outside');
  });

  test('the "Open thread actions" shortcut opens the highlighted row\'s ⋯ menu (with Pin)', async ({ page }) => {
    const id = randomUUID();
    psql(seedThreadRow({ id, title: `menu-${Date.now()}`, now: new Date().toISOString() }));

    await openSettledDrawer(page);

    await page.keyboard.press('Control+Shift+1');
    // Move off the section headers and any survivor row onto the seeded one.
    await highlightRow(page, id);
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
