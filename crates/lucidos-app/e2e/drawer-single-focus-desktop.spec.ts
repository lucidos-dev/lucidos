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
      .querySelector('.thread-drawer:not(.thread-drawer-collapsed) .thread-drawer-list > .thread-drawer-tree')
      ?.getAttribute('aria-activedescendant') ?? '<none>');
  }, {
    intervals: [50],
    message: 'the highlight never reached the seeded row',
  }).toBe(`drawer-nav-${id}`);
}

// The thread drawer has ONE keyboard focus. Its tree is a single tab stop
// (role="tree", tabindex=0) whose `aria-activedescendant` points at the
// highlighted row. The per-row buttons leave the Tab order (tabindex=-1). The
// keyboard reaches row actions through the ⋯ menu, via the customizable "Open
// thread actions" shortcut.
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

  test('focusing the drawer sets aria-activedescendant; ↓ moves it (= the highlight); Tab stays on the tree', async ({ page }) => {
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

    const tree = page.locator('.thread-drawer:not(.thread-drawer-collapsed) .thread-drawer-list > .thread-drawer-tree').first();

    // The premise of the Tab assertion. A list that stopped overflowing would
    // pass it for the wrong reason and guard nothing.
    await expect
      .poll(async () => await page.evaluate(() => {
        const l = document.querySelector('.thread-drawer-list');
        return l ? l.scrollHeight - l.clientHeight : -1;
      }), { message: 'the drawer list never overflowed' })
      .toBeGreaterThan(0);

    // ⌘⇧1 / Ctrl+Shift+1 — the focus-aware drawer toggle focuses the tree
    // and seeds the highlight, so the tree (not a row) holds DOM focus.
    await page.keyboard.press('Control+Shift+1');
    await expect(tree).toBeFocused();
    await expect(tree).toHaveAttribute('aria-activedescendant', /.+/);

    // ↓ lands on a thread row: the active-descendant id and the visually
    // highlighted row are the SAME element — one focus, not two.
    await page.keyboard.press('ArrowDown');
    const highlighted = page.locator('.thread-row.thread-row-highlighted').first();
    await expect(highlighted).toBeVisible();
    const activeDesc = await tree.getAttribute('aria-activedescendant');
    const highlightedId = await highlighted.getAttribute('id');
    expect(activeDesc).toBe(highlightedId);
    // DOM focus never moved onto a row/button — it stays on the tree.
    await expect(tree).toBeFocused();

    // The tree is the list's single tab stop, and the per-pane Tab trap cycles
    // the focused pane. The grouping is picked in the header, so the tree is
    // the pane's only stop and Tab stays on it. No row button grabs focus, and
    // the scroller never becomes a stop of its own.
    //
    // Names the node it stopped on rather than answering yes or no, so a row
    // button stealing focus and focus escaping the pane read differently.
    const stoppedOn = () => page.evaluate(() => {
      const d = document.querySelector('.thread-drawer:not(.thread-drawer-collapsed)');
      const a = document.activeElement;
      if (!d || !a || !d.contains(a)) return 'outside';
      if (a.matches('.thread-drawer-tree')) return 'the tree';
      const cls = typeof a.className === 'string' ? a.className : '';
      return `${a.tagName.toLowerCase()}${cls ? `.${cls.trim().split(/\s+/).join('.')}` : ''}`;
    });
    await page.keyboard.press('Tab');
    expect(await stoppedOn()).toBe('the tree');
    await page.keyboard.press('Shift+Tab');
    expect(await stoppedOn()).toBe('the tree');
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

  test('with no drawer row focused, the shortcut opens the open thread\'s menu', async ({ page, context }) => {
    const id = randomUUID();
    const title = `open-${Date.now()}`;
    psql(seedThreadRow({ id, title, now: new Date().toISOString() }));
    await context.addInitScript((id) => {
      localStorage.setItem('lucidos-focused-thread', id);
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
    }, id);

    await navigateToApp(page);
    await waitForEventStream(page);
    await expect(page.locator('.thread-view-header')).toContainText(title);

    await page.keyboard.press('Control+Shift+M');
    // Only the title's menu offers this item; a drawer row's menu never does.
    await expect(page.getByRole('menuitem', { name: 'Show in Folders' })).toBeVisible();
  });
});
