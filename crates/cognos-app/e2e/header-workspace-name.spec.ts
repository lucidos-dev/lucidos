/**
 * Header layout regression tests: the brand label `[Lucidos • workspace]`
 * must be centered in its available space (between left and right actions),
 * truncate the workspace name first, then "Lucidos", and never overlap
 * any header icon. The status dot is the minimum — it must stay visible
 * even when both text labels are fully truncated.
 *
 * Both the desktop and mobile chat headers share the same brand-label
 * internals (Lucidos title + connection status + workspace name), so the
 * priority truncation behavior is verified for both.
 */
import { test, expect, Page } from '@playwright/test';
import { assertHealthy, navigateToApp, ensureOnThreadPane } from './helpers';

interface RectBounds { left: number; right: number; top: number; bottom: number }
function rectsOverlap(a: RectBounds, b: RectBounds): boolean {
  return !(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top);
}

type HeaderScope = '.mobile-thread-header' | '.desktop-header';

async function setLongWorkspaceName(page: Page, scope: HeaderScope): Promise<void> {
  await page.evaluate((s) => {
    const label = document.querySelector(`${s} .workspace-name-label`) as HTMLElement | null;
    if (label) label.textContent = 'a-very-long-workspace-name';
  }, scope);
}

test.describe('Mobile chat header — workspace name truncation', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('long workspace name truncates and does not overlap compose icon', async ({ page }) => {
    await navigateToApp(page);
    await ensureOnThreadPane(page);
    await setLongWorkspaceName(page, '.mobile-thread-header');

    const result = await page.evaluate(() => {
      const label = document.querySelector('.mobile-thread-header .workspace-name-label') as HTMLElement | null;
      const compose = document.querySelector('.mobile-thread-header .brand-compose-btn') as HTMLElement | null;
      if (!label || !compose) return { error: 'missing elements' };

      const labelRect = label.getBoundingClientRect();
      const composeRect = compose.getBoundingClientRect();
      const styles = getComputedStyle(label);
      return {
        labelRect: { left: labelRect.left, right: labelRect.right, top: labelRect.top, bottom: labelRect.bottom },
        composeRect: { left: composeRect.left, right: composeRect.right, top: composeRect.top, bottom: composeRect.bottom },
        scrollWidth: label.scrollWidth,
        clientWidth: label.clientWidth,
        whiteSpace: styles.whiteSpace,
        overflow: styles.overflow,
        textOverflow: styles.textOverflow,
      };
    });

    expect(result).not.toHaveProperty('error');
    const r = result as Exclude<typeof result, { error: string }>;

    expect(r.whiteSpace).toBe('nowrap');
    expect(r.overflow).toBe('hidden');
    expect(r.textOverflow).toBe('ellipsis');

    expect(rectsOverlap(r.labelRect, r.composeRect),
      `Label rect ${JSON.stringify(r.labelRect)} overlaps compose ${JSON.stringify(r.composeRect)}`).toBe(false);

    expect(r.clientWidth, 'label did not truncate (clientWidth >= scrollWidth)').toBeLessThan(r.scrollWidth);
  });
});

test.describe('Desktop chat header — brand label centered between actions', () => {
  // 900px is the smallest desktop viewport (>768 mobile breakpoint) where the
  // brand area is tight enough to force workspace truncation.
  test.use({ viewport: { width: 900, height: 700 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Lucidos sits after left actions, brand does not overlap search, workspace truncates first', async ({ page }) => {
    await navigateToApp(page);
    await setLongWorkspaceName(page, '.desktop-header');

    const result = await page.evaluate(() => {
      const header = document.querySelector('.desktop-header');
      if (!header) return { error: 'no desktop header' };

      const brandLabel = header.querySelector('.pane-header-brand-label') as HTMLElement | null;
      // Left action group: collapsed-thread-actions when drawer is closed,
      // thread-nav-group inside brand when drawer is open.
      const leftActions = (header.querySelector('.collapsed-thread-actions') ||
                           header.querySelector('.pane-header-brand .thread-nav-group')) as HTMLElement | null;
      const search = header.querySelector('.pane-header-brand [data-role="search-everywhere-toggle"]') as HTMLElement | null;
      const lucidos = header.querySelector('.pane-header-brand-label .pane-header-title') as HTMLElement | null;
      const dot = header.querySelector('.pane-header-brand-label .status-dot') as HTMLElement | null;
      const workspace = header.querySelector('.pane-header-brand-label .workspace-name-label') as HTMLElement | null;

      if (!brandLabel || !leftActions || !search || !lucidos || !dot || !workspace) {
        return { error: 'missing elements' };
      }

      return {
        brandLabel: brandLabel.getBoundingClientRect(),
        leftActions: leftActions.getBoundingClientRect(),
        search: search.getBoundingClientRect(),
        lucidos: lucidos.getBoundingClientRect(),
        dot: dot.getBoundingClientRect(),
        workspaceClient: workspace.clientWidth,
        workspaceScroll: workspace.scrollWidth,
      };
    });

    expect(result).not.toHaveProperty('error');
    const r = result as Exclude<typeof result, { error: string }>;

    // Lucidos must start at or after the right edge of the left action group.
    expect(r.lucidos.left,
      `Lucidos starts at ${r.lucidos.left} but left actions end at ${r.leftActions.right}`)
      .toBeGreaterThanOrEqual(r.leftActions.right);

    // Brand label must not overlap the search button.
    expect(rectsOverlap(r.brandLabel, r.search),
      `Brand label ${JSON.stringify(r.brandLabel)} overlaps search ${JSON.stringify(r.search)}`).toBe(false);

    // Workspace must truncate (clientWidth < scrollWidth) at this viewport.
    expect(r.workspaceClient,
      'workspace did not truncate at narrow desktop width').toBeLessThan(r.workspaceScroll);

    // Status dot must remain visible (its width > 0).
    expect(r.dot.right - r.dot.left, 'status dot disappeared').toBeGreaterThan(0);
  });

  test('Lucidos itself truncates and stays clear of the search icon', async ({ page }) => {
    // Drawer open + narrow desktop = thread-nav-group inside the brand eats
    // most of the available width, forcing the title to truncate even with
    // no workspace name.
    await page.setViewportSize({ width: 800, height: 700 });
    await page.addInitScript(() => {
      localStorage.setItem('cognos-thread-drawer-open', 'true');
    });
    await navigateToApp(page);
    await page.evaluate(() => {
      const label = document.querySelector('.desktop-header .workspace-name-label') as HTMLElement | null;
      if (label) label.textContent = '';
    });

    const result = await page.evaluate(() => {
      const lucidos = document.querySelector('.desktop-header .pane-header-brand-label .pane-header-title') as HTMLElement | null;
      const search = document.querySelector('.desktop-header .pane-header-brand [data-role="search-everywhere-toggle"]') as HTMLElement | null;
      if (!lucidos || !search) return { error: 'missing elements' };
      return {
        lucidos: lucidos.getBoundingClientRect(),
        lucidosClient: lucidos.clientWidth,
        lucidosScroll: lucidos.scrollWidth,
        overflowX: getComputedStyle(lucidos).overflowX,
        search: search.getBoundingClientRect(),
      };
    });

    expect(result).not.toHaveProperty('error');
    const r = result as Exclude<typeof result, { error: string }>;

    // Lucidos must clip horizontally so text-overflow:ellipsis works.
    expect(r.overflowX).toBe('hidden');

    // At this viewport Lucidos must actually truncate.
    expect(r.lucidosClient, 'Lucidos did not truncate (clientWidth >= scrollWidth)')
      .toBeLessThan(r.lucidosScroll);

    // Lucidos's rendered box must not extend over the search icon.
    expect(r.lucidos.right,
      `Lucidos right (${r.lucidos.right}) overlaps search left (${r.search.left})`)
      .toBeLessThanOrEqual(r.search.left);
  });
});
