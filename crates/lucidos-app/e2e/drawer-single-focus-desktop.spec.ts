import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy, waitForEventStream } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

async function openSettledDrawer(page: Page): Promise<void> {
  await navigateToApp(page);
  await waitForEventStream(page);
  await openThreadDrawer(page);
}

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

test.describe('Thread drawer — single keyboard focus (aria-activedescendant)', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    await context.clearCookies();
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
    await expect(row.locator('.pin-btn')).toHaveAttribute('tabindex', '-1');
    await expect(row.locator('button[aria-haspopup="menu"]')).toHaveAttribute('tabindex', '-1');
  });

  test('focusing the drawer sets aria-activedescendant; ↓ moves it (= the highlight); Tab stays on the tree', async ({ page }) => {
    const t = Date.now();
    const seeded = Array.from({ length: 30 }, (_, i) => ({
      id: randomUUID(),
      title: `row-${t}-${String(i).padStart(2, '0')}`,
      now: new Date(t - i * 1000).toISOString(),
    }));
    psql(seeded.map(seedThreadRow).join(';\n'));

    await openSettledDrawer(page);

    const tree = page.locator('.thread-drawer:not(.thread-drawer-collapsed) .thread-drawer-list > .thread-drawer-tree').first();

    await expect
      .poll(async () => await page.evaluate(() => {
        const l = document.querySelector('.thread-drawer-list');
        return l ? l.scrollHeight - l.clientHeight : -1;
      }), { message: 'the drawer list never overflowed' })
      .toBeGreaterThan(0);

    await page.keyboard.press('Control+Shift+1');
    await expect(tree).toBeFocused();
    await expect(tree).toHaveAttribute('aria-activedescendant', /.+/);

    await page.keyboard.press('ArrowDown');
    const highlighted = page.locator('.thread-row.thread-row-highlighted').first();
    await expect(highlighted).toBeVisible();
    const activeDesc = await tree.getAttribute('aria-activedescendant');
    const highlightedId = await highlighted.getAttribute('id');
    expect(activeDesc).toBe(highlightedId);
    await expect(tree).toBeFocused();

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
    await highlightRow(page, id);
    await expect(
      page.locator(`.thread-row.thread-row-highlighted[data-thread-nav="${id}"]`),
    ).toBeVisible();

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
    await expect(page.getByRole('menuitem', { name: 'Show in Folders' })).toBeVisible();
  });
});
