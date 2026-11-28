import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  waitForVisibleInput,
  openThreadDrawer,
  clickVisibleElement,
} from './helpers';
import { clearAllThreads } from './db-helpers';


test.describe('Threads-header unified Filter control — desktop layout', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  const sizeAndOpen = async (page: import('@playwright/test').Page) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);
    await openThreadDrawer(page);
    await page.waitForFunction(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find((h) => h.getBoundingClientRect().width > 0);
      const title = header?.querySelector('.threads-header-title');
      return !!title && (title as HTMLElement).getBoundingClientRect().width > 0;
    }, undefined, { timeout: 10_000 });
    await page.waitForTimeout(400);
  };

  test('one Filter button, no separate view selector, holds the Threads title in place', async ({ page }) => {
    await sizeAndOpen(page);

    const measure = async () => page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement | undefined;
      if (!header) return null;
      const title = header.querySelector('.threads-header-title') as HTMLElement | null;
      const filter = header.querySelector('button[aria-label="Filter threads"]') as HTMLElement | null;
      const search = header.querySelector('button[aria-label="Search threads"]') as HTMLElement | null;
      const grouping = header.querySelector('.grouping-btn') as HTMLElement | null;
      const selector = header.querySelector('button[aria-label="Switch thread view"]');
      const rect = (el: HTMLElement | null) => el ? el.getBoundingClientRect() : null;
      return {
        titleTextAlign: title ? getComputedStyle(title).textAlign : '',
        titleLeft: rect(title)?.left ?? 0,
        titleRight: rect(title)?.right ?? 0,
        filterWidth: rect(filter)?.width ?? 0,
        filterLeft: rect(filter)?.left ?? 0,
        filterRight: rect(filter)?.right ?? 0,
        groupingRight: rect(grouping)?.right ?? 0,
        searchLeft: rect(search)?.left ?? 0,
        hasSeparateSelector: !!selector,
      };
    });

    const empty = await measure();
    expect(empty, 'visible threads-header').not.toBeNull();
    expect(empty!.hasSeparateSelector, 'no separate view-selector button').toBe(false);
    expect(empty!.filterWidth, 'single Filter button is visible').toBeGreaterThan(20);
    expect(empty!.filterRight, 'Filter button sits left of the pane title')
      .toBeLessThanOrEqual(empty!.titleLeft + 1);
    const groupingToSearch = empty!.searchLeft - empty!.groupingRight;
    expect(groupingToSearch, 'Grouping button sits just left of Search').toBeGreaterThanOrEqual(0);
    expect(groupingToSearch, 'Grouping button sits just left of Search').toBeLessThanOrEqual(8);
    expect(empty!.titleTextAlign, 'Threads title text centres in its box').toBe('center');

    const input = await waitForVisibleInput(page);
    await input.fill('an unsent draft that adds a drawer row');
    await page.waitForTimeout(100);
    const withDraft = await measure();
    expect(Math.abs(withDraft!.titleLeft - empty!.titleLeft), 'Threads title moved when a draft appeared')
      .toBeLessThan(1);
  });

  test('the title never runs under a header control, on either desktop build, at the drawer floor', async ({ page }) => {
    await sizeAndOpen(page);
    await page.locator('.threads-header button[aria-label="Filter threads"]').click();
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();

    const measure = () => page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement | undefined;
      if (!header) return null;
      const title = header.querySelector('.threads-header-title') as HTMLElement | null;
      if (!title) return null;
      const box = title.getBoundingClientRect();
      const overlapOf = (el: Element | null) => {
        if (!el) return -1;
        const r = el.getBoundingClientRect();
        return Math.max(0, Math.min(r.right, box.right) - Math.max(r.left, box.left));
      };
      const search = header.querySelector('button[aria-label="Search threads"]') as HTMLElement;
      return {
        drawerWidth: header.getBoundingClientRect().width,
        toggle: overlapOf(document.querySelector('.desktop-header .thread-toggle-slot .thread-toggle')),
        filter: overlapOf(header.querySelector('button[aria-label="Filter threads"]')),
        grouping: overlapOf(header.querySelector('.grouping-btn')),
        search: overlapOf(search),
        shown: getComputedStyle(title).visibility === 'visible',
        whole: title.scrollWidth <= title.clientWidth + 0.5,
        searchInside: search.getBoundingClientRect().right
          <= header.getBoundingClientRect().right + 1,
      };
    });

    const divider = page.locator('.drawer-divider');
    for (const scale of [100, 125]) for (const overlay of [false, true]) {
      const build = `${overlay ? 'packaged macOS' : 'web'} at ${scale}%`;
      await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);
      await page.evaluate((on) => {
        const root = document.documentElement;
        if (on) {
          root.setAttribute('data-titlebar-overlay', '');
          root.style.setProperty('--titlebar-inset', '28px');
        } else {
          root.removeAttribute('data-titlebar-overlay');
          root.style.removeProperty('--titlebar-inset');
        }
      }, overlay);
      await page.waitForTimeout(400);

      const box = await divider.boundingBox();
      expect(box, `${build}: drawer divider is visible`).not.toBeNull();
      await page.mouse.move(box!.x + box!.width / 2, box!.y + 100);
      await page.mouse.down();
      await page.mouse.move(150, box!.y + 100, { steps: 12 });
      await page.mouse.up();
      await page.waitForTimeout(1200);

      const g = await measure();
      expect(g, `${build}: visible threads-header`).not.toBeNull();
      expect(g!.drawerWidth, `${build}: the drawer stayed below its floor`)
        .toBeGreaterThan(300);
      expect(g!.searchInside, `${build}: the Search button is outside the drawer`).toBe(true);
      expect(g!.shown, `${build}: the title shows`).toBe(!overlay);
      if (!g!.shown) continue;
      expect(g!.whole, `${build}: the title is cut`).toBe(true);
      expect(g!.toggle, `${build}: title overlaps the drawer toggle`).toBe(0);
      expect(g!.filter, `${build}: title overlaps the Filter button`).toBe(0);
      expect(g!.grouping, `${build}: title overlaps the Grouping button`).toBe(0);
      expect(g!.search, `${build}: title overlaps the Search button`).toBe(0);
    }
  });

  test('opens the thread-type panel IN THE DRAWER PANE; a type narrows the list in place and keeps the panel up', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    await filterBtn.click();

    const panel = page.locator('.thread-drawer .thread-filter-panel');
    await expect(panel).toBeVisible();
    await expect(page.locator('.thread-filter-panel')).toHaveCount(1);
    await expect(page.locator('.threads-header .thread-filter-panel')).toHaveCount(0);

    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Filters');
    await expect(panel.locator('.thread-filter-panel-header')).toHaveCount(0);
    await expect(panel.locator('.thread-filter-panel-footer')).toHaveCount(0);
    await expect(panel.locator('.thread-filter-close')).toHaveCount(0);

    await expect(panel.locator('.thread-filter-title')).toHaveCount(0);
    expect(await panel.evaluate((p) => Array.from(p.children).map(c => c.classList[0]))).toEqual([
      'thread-filter-types', 'thread-filter-option',
    ]);
    await expect(panel.locator('.thread-filter-divider')).toHaveCount(0);

    const types = panel.locator('div.thread-filter-types');
    await expect(types).toHaveAttribute('role', 'group');
    await expect(types).toHaveAttribute('aria-label', 'Thread types');
    await expect(types.locator('input[type="checkbox"]').first()).toBeEnabled();

    const lucidos = panel.locator(
      '.thread-filter-option:not(.thread-filter-option-child)', { hasText: 'Lucidos' },
    );
    const includeDeleted = panel.locator(
      'label.thread-filter-option', { hasText: 'Include deleted' },
    );
    await expect(types.locator('label', { hasText: 'Include deleted' })).toHaveCount(0);
    const includeDeletedBox = includeDeleted.locator('input[type="checkbox"]');
    await includeDeletedBox.click();
    await expect(includeDeletedBox).toBeChecked();
    await includeDeletedBox.click();
    await expect(includeDeletedBox).not.toBeChecked();

    await lucidos.click();
    await expect(panel).toBeVisible();
    await expect(lucidos.locator('input[type="checkbox"]')).not.toBeChecked();

    await filterBtn.click();
    await expect(panel).toBeHidden();
    await filterBtn.click();
    await expect(panel).toBeVisible();
    await expect(lucidos.locator('input[type="checkbox"]')).not.toBeChecked();

    await lucidos.click();
    await expect(lucidos.locator('input[type="checkbox"]')).toBeChecked();

    await page.keyboard.press('Escape');
    await expect(panel).toBeHidden();
  });

  test('the Filter button is held down while the panel is up, and pressing it again is the way out', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const glyph = filterBtn.locator('.crossfade-layer[data-current] svg');
    const panel = page.locator('.thread-drawer .thread-filter-panel');

    await expect(glyph.locator('path')).toHaveCount(1);
    await expect(glyph.locator('line, circle')).toHaveCount(0);
    await expect(glyph).toHaveAttribute('fill', 'none');
    const funnel = await glyph.innerHTML();
    await expect(filterBtn).toHaveAttribute('aria-pressed', 'false');
    await expect(filterBtn).toHaveAttribute('aria-expanded', 'false');

    await filterBtn.click();
    await expect(panel).toBeVisible();

    expect(await glyph.innerHTML(), 'opening swapped the funnel').toBe(funnel);
    await expect(filterBtn).toHaveAttribute('aria-pressed', 'true');
    await expect(filterBtn).toHaveAttribute('aria-expanded', 'true');

    await filterBtn.click();
    await expect(panel).toBeHidden();
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Folders');
    await expect(page.locator('.thread-drawer .thread-drawer-list')).toBeVisible();
    expect(await glyph.innerHTML()).toBe(funnel);
    await expect(filterBtn).toHaveAttribute('aria-pressed', 'false');
  });

  test('pressed means the panel is open, and the glyph says whether types narrow the list', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const glyph = filterBtn.locator('.crossfade-layer[data-current] svg');
    const panel = page.locator('.thread-drawer .thread-filter-panel');
    await expect(glyph).toHaveAttribute('fill', 'none');
    const funnel = await glyph.innerHTML();

    await filterBtn.click();
    await expect(filterBtn).toHaveAttribute('aria-pressed', 'true');
    const lucidos = panel.locator(
      '.thread-filter-option:not(.thread-filter-option-child)', { hasText: 'Lucidos' },
    );
    await lucidos.click();
    await expect(glyph).toHaveAttribute('fill', 'currentColor');
    await filterBtn.click();
    await expect(panel).toBeHidden();
    await expect(filterBtn).toHaveAttribute('aria-pressed', 'false');
    await expect(glyph).toHaveAttribute('fill', 'currentColor');

    const slot = page.locator('.threads-header .filter-slot');
    await page.locator('.threads-header .grouping-btn').click();
    await expect(slot).not.toHaveAttribute('data-shown');
    await expect(filterBtn).toBeHidden();
    await page.locator('.threads-header .grouping-btn').click();
    await expect(slot).toHaveAttribute('data-shown');
    await expect(filterBtn).toBeVisible();
    await expect(glyph).toHaveAttribute('fill', 'currentColor');

    await filterBtn.click();
    await lucidos.click();
    await expect(glyph).toHaveAttribute('fill', 'none');
    await filterBtn.click();
    await expect(panel).toBeHidden();
    expect(await glyph.innerHTML()).toBe(funnel);
  });

  test('the panel sits on the thread list own column: same left inset, same edges', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    await filterBtn.click();
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();

    const geometry = await page.evaluate(() => {
      const px = (el: Element, prop: string) => parseFloat(getComputedStyle(el).getPropertyValue(prop));
      const list = document.querySelector('.thread-drawer .thread-drawer-list')!;
      const cover = document.querySelector('.thread-drawer .thread-filter-cover')!;
      const panel = cover.querySelector('.thread-filter-panel')!;
      const typeRow = panel.querySelector('.thread-filter-types .thread-filter-option')!;
      const deletedRow = panel.querySelector(':scope > .thread-filter-option')!;
      const row = document.createElement('div');
      row.className = 'thread-row';
      list.appendChild(row);
      const listColumn = px(row, 'padding-left');
      row.remove();
      return {
        listColumn,
        panelTop: px(cover, 'padding-top'),
        listTop: px(list, 'padding-top'),
        panelLeft: px(cover, 'padding-left'),
        typeLeft: px(typeRow, 'padding-left'),
        deletedLeft: px(deletedRow, 'padding-left'),
      };
    });

    expect(geometry.panelTop).toBe(geometry.listTop);
    expect(geometry.panelLeft).toBe(0);
    expect(geometry.typeLeft).toBe(geometry.listColumn);
    expect(geometry.deletedLeft).toBe(geometry.listColumn);
  });

  test('the Filter button opens reliably and toggles closed (Chrome open-bug regression)', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const panel = page.locator('.thread-drawer .thread-filter-panel');

    await filterBtn.click();
    await expect(panel).toBeVisible();

    await filterBtn.click();
    await expect(panel).toBeHidden();
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Folders');

    await filterBtn.click();
    await expect(panel).toBeVisible();
  });

  test('closing the drawer keeps the panel open, but nothing invisible holds Escape', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const panel = page.locator('.thread-drawer .thread-filter-panel');
    const openCover = page.locator('.thread-filter-cover[data-open]');
    await filterBtn.click();
    await expect(panel).toBeVisible();

    const toggled = await clickVisibleElement(page, 'button[aria-label^="Show or hide thread drawer"]');
    expect(toggled, 'drawer toggle was visible').toBe(true);

    await expect(page.locator('.thread-filter-cover[data-open][inert]')).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(openCover).toHaveCount(1);

    await openThreadDrawer(page);
    await expect(panel).toBeVisible();
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Filters');
    await page.keyboard.press('Escape');
    await expect(openCover).toHaveCount(0);
  });

  test('is a pane view, not an overlay: a click elsewhere acts normally and leaves it open', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const panel = page.locator('.thread-drawer .thread-filter-panel');
    await filterBtn.click();
    await expect(panel).toBeVisible();

    await expect(page.locator('html[data-overlay-open]')).toHaveCount(0);

    const input = await waitForVisibleInput(page);
    await input.click();
    await expect(input).toBeFocused();

    await expect(panel).toBeVisible();

    await page.locator('.threads-header button[aria-label="Search threads"]').click();
    await expect(panel).toBeHidden();
  });
});
