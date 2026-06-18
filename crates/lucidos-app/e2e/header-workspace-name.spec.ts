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
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, ensureOnThreadPane } from './helpers';

interface RectBounds { left: number; right: number; top: number; bottom: number }
function rectsOverlap(a: RectBounds, b: RectBounds): boolean {
  return !(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top);
}

type HeaderScope = '.mobile-thread-header' | '.desktop-header';

async function setLongWorkspaceName(page: Page, scope: HeaderScope): Promise<void> {
  // workspaceName is populated async from /health.
  await page.waitForSelector(`${scope} .workspace-name-label`, { state: 'attached' });
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
        lucidosClient: lucidos.clientWidth,
        lucidosScroll: lucidos.scrollWidth,
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

    // Brand priority: while the workspace label is still on screen and able
    // to absorb shrink, the "lucidos" title must remain at its natural width.
    // Both truncating at once ("lucid... ● wo...") is the regression we test.
    expect(r.lucidosClient,
      `Lucidos truncated (${r.lucidosClient} < ${r.lucidosScroll}) while workspace was still visible`)
      .toBe(r.lucidosScroll);

    // Status dot must remain visible (its width > 0).
    expect(r.dot.right - r.dot.left, 'status dot disappeared').toBeGreaterThan(0);
  });

  test('priority order: workspace truncates → workspace hidden → brand truncates → only dot stays', async ({ page }) => {
    // Walk the brand-label width through every truncation tier and verify the
    // user-visible state at each one. Drives the bug fix so that every
    // intermediate state stays correct on every commit.
    await page.setViewportSize({ width: 1400, height: 700 });
    await navigateToApp(page);
    await setLongWorkspaceName(page, '.desktop-header');

    type State = {
      lucidosFull: boolean;
      lucidosWidth: number;
      lucidosNatural: number;
      workspaceVisible: boolean;
      workspaceTruncated: boolean;
      workspaceWidth: number;
      workspaceNatural: number;
      dotVisible: boolean;
    };

    // Measure the natural widths of the non-shrinkable parts (lucidos title +
    // dot block) once at a wide brand-label so we can pick brand-label widths
    // for each tier RELATIVE to the actual fonts/spacing instead of guessing
    // pixel constants that drift when CSS changes.
    const naturals = await page.evaluate(() => {
      const brandLabel = document.querySelector('.desktop-header .pane-header-brand-label') as HTMLElement;
      brandLabel.style.flex = 'none';
      brandLabel.style.width = '500px';
      const lucidos = brandLabel.querySelector('.pane-header-title') as HTMLElement;
      const conn = brandLabel.querySelector('.connection-status-inline') as HTMLElement;
      const ws = brandLabel.querySelector('.workspace-name-label') as HTMLElement;
      const margin = (el: HTMLElement) => {
        const cs = getComputedStyle(el);
        return (parseFloat(cs.marginLeft) || 0) + (parseFloat(cs.marginRight) || 0);
      };
      return {
        lucidosNatural: lucidos.scrollWidth + margin(lucidos),
        connNatural: conn.scrollWidth + margin(conn),
        wsMargin: margin(ws),
      };
    });

    const nonWs = naturals.lucidosNatural + naturals.connNatural;

    const measureAt = async (brandLabelWidthPx: number): Promise<State> => {
      return await page.evaluate((w) => {
        const brandLabel = document.querySelector('.desktop-header .pane-header-brand-label') as HTMLElement;
        brandLabel.style.flex = 'none';
        brandLabel.style.width = `${w}px`;
        return new Promise<State>((resolve) => {
          // Two RAFs so the ResizeObserver in ConnectionStatus runs and
          // updates is-hidden, then layout reflects it.
          requestAnimationFrame(() => requestAnimationFrame(() => {
            const lucidos = brandLabel.querySelector('.pane-header-title') as HTMLElement;
            const dot = brandLabel.querySelector('.status-dot') as HTMLElement;
            const workspace = brandLabel.querySelector('.workspace-name-label') as HTMLElement;
            const wsRect = workspace.getBoundingClientRect();
            resolve({
              lucidosFull: lucidos.clientWidth >= lucidos.scrollWidth,
              lucidosWidth: lucidos.clientWidth,
              lucidosNatural: lucidos.scrollWidth,
              workspaceVisible: getComputedStyle(workspace).visibility !== 'hidden' && wsRect.width > 0,
              workspaceTruncated: workspace.clientWidth < workspace.scrollWidth,
              workspaceWidth: workspace.clientWidth,
              workspaceNatural: workspace.scrollWidth,
              dotVisible: dot.getBoundingClientRect().width > 0,
            });
          }));
        });
      }, brandLabelWidthPx);
    };

    // Tier 1: brand-label wide enough for everything full. Sanity check.
    const wide = await measureAt(500);
    expect(wide.lucidosFull, 'tier 1 (wide): lucidos full').toBe(true);
    expect(wide.workspaceVisible, 'tier 1 (wide): workspace visible').toBe(true);
    expect(wide.workspaceTruncated, 'tier 1 (wide): workspace not truncated').toBe(false);

    // Tier 2: well inside the "workspace can absorb" range. Workspace truncates,
    // lucidos stays at natural width.
    const tier2 = await measureAt(Math.ceil(nonWs + naturals.wsMargin) + 60);
    expect(tier2.lucidosFull,
      `tier 2 (workspace truncates): lucidos full (${tier2.lucidosWidth}/${tier2.lucidosNatural})`).toBe(true);
    expect(tier2.workspaceVisible, 'tier 2: workspace visible').toBe(true);
    expect(tier2.workspaceTruncated, 'tier 2: workspace truncated').toBe(true);
    expect(tier2.dotVisible, 'tier 2: dot visible').toBe(true);

    // Tier 3: too narrow for workspace's margin gap, but lucidos+dot still fit.
    // Workspace must be hidden, lucidos still full.
    const tier3 = await measureAt(Math.ceil(nonWs) + 2);
    expect(tier3.workspaceVisible,
      `tier 3 (brand+dot only): workspace hidden (width=${tier3.workspaceWidth})`).toBe(false);
    expect(tier3.lucidosFull,
      `tier 3: lucidos still full (${tier3.lucidosWidth}/${tier3.lucidosNatural})`).toBe(true);
    expect(tier3.dotVisible, 'tier 3: dot visible').toBe(true);

    // Tier 4: too narrow even for full lucidos + dot. Workspace hidden,
    // lucidos truncates with ellipsis, dot still visible.
    const tier4 = await measureAt(Math.floor(nonWs) - 20);
    expect(tier4.workspaceVisible, 'tier 4: workspace hidden').toBe(false);
    expect(tier4.lucidosFull,
      `tier 4 (brand truncates): lucidos truncated (${tier4.lucidosWidth}/${tier4.lucidosNatural})`).toBe(false);
    expect(tier4.dotVisible, 'tier 4: dot still visible — last thing standing').toBe(true);
  });

  test('Lucidos itself truncates and stays clear of the search icon', async ({ page }) => {
    // Drawer open + narrow desktop = thread-nav-group inside the brand eats
    // most of the available width, forcing the title to truncate even with
    // no workspace name.
    await page.setViewportSize({ width: 800, height: 700 });
    await page.addInitScript(() => {
      localStorage.setItem('lucidos-thread-drawer-open', 'true');
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
