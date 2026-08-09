/**
 * Where the workspace name is shown, on each viewport.
 *
 * DESKTOP shows it in the bar, beside the Lucidos mark, for as long as the pane
 * can hold it WHOLE. There are only two states: the full name, or no name. It
 * never ellipsises, because half a workspace name identifies nothing, and it
 * never overlaps a header icon.
 *
 * MOBILE has no room for it in any header row, so the name lives inside the
 * Lucidos menu as the Workspaces row's trailing selected-value chip. That
 * surface is desktop's fallback too, which is why the menu case below runs on
 * both viewports: it is the only place the name is guaranteed to be, and the
 * menu is a fixed-width centred panel, so "does not widen" is the whole of the
 * layout contract there.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, ensureOnThreadPane } from './helpers';

interface RectBounds { left: number; right: number; top: number; bottom: number }
function rectsOverlap(a: RectBounds, b: RectBounds): boolean {
  return !(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top);
}

async function setLongWorkspaceName(page: Page): Promise<void> {
  // workspaceName is populated async from /health.
  await page.waitForSelector('.desktop-header .workspace-name-label', { state: 'attached' });
  await page.evaluate(() => {
    const label = document.querySelector('.desktop-header .workspace-name-label') as HTMLElement | null;
    if (label) label.textContent = 'a-very-long-workspace-name';
  });
}

/** The name in the menu truncates rather than widening the fixed-width panel.
 *  One body, two viewports: each opens the menu from the mark its own layout
 *  renders. */
async function assertMenuNameTruncates(page: Page, markSelector: string): Promise<void> {
  await page.locator(markSelector).first().click();
  await expect(page.locator('.brand-menu')).toBeVisible();

  const result = await page.evaluate(() => {
    const menu = document.querySelector('.brand-menu') as HTMLElement | null;
    const name = document.querySelector('.brand-menu-value-name') as HTMLElement | null;
    const chip = document.querySelector('.brand-menu-value') as HTMLElement | null;
    if (!menu || !name || !chip) return { error: 'missing elements' };

    const widthBefore = menu.getBoundingClientRect().width;
    name.textContent = 'a-very-long-workspace-name-that-keeps-going';
    const menuRect = menu.getBoundingClientRect();

    return {
      widthBefore,
      widthAfter: menuRect.width,
      chipRight: chip.getBoundingClientRect().right,
      menuRight: menuRect.right,
      nameTruncates: name.clientWidth < name.scrollWidth,
      nameOverflow: getComputedStyle(name).textOverflow,
    };
  });

  expect(result).not.toHaveProperty('error');
  const r = result as Exclude<typeof result, { error: string }>;

  // The menu is a fixed-width panel: a long name must not stretch it.
  expect(r.widthAfter, 'menu widened to fit the name').toBeCloseTo(r.widthBefore, 1);
  // The name ellipsises rather than spilling past the panel edge.
  expect(r.nameOverflow).toBe('ellipsis');
  expect(r.nameTruncates, 'the selected-workspace chip did not truncate').toBe(true);
  expect(r.chipRight).toBeLessThanOrEqual(r.menuRight + 0.5);
}

test.describe('Mobile Lucidos menu, workspace name truncation', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a long workspace name truncates inside the menu rather than widening it', async ({ page }) => {
    await navigateToApp(page);
    await ensureOnThreadPane(page);
    await assertMenuNameTruncates(page, '.mobile-thread-header [data-role="brand-menu-toggle"]');
  });
});

test.describe('Desktop chat header: the mark, and the name while it fits', () => {
  // 900px is the smallest desktop viewport (>768 mobile breakpoint) where the
  // brand area is tight enough to force workspace truncation.
  test.use({ viewport: { width: 900, height: 700 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the name sits after the left actions, clear of the trailing cluster, and is whole or gone', async ({ page }) => {
    await navigateToApp(page);
    await setLongWorkspaceName(page);

    const result = await page.evaluate(() => {
      const header = document.querySelector('.desktop-header');
      if (!header) return { error: 'no desktop header' };

      const brandLabel = header.querySelector('.pane-header-brand-label') as HTMLElement | null;
      // Left action group: collapsed-thread-actions when drawer is closed,
      // thread-nav-group inside brand when drawer is open.
      const leftActions = (header.querySelector('.collapsed-thread-actions') ||
                           header.querySelector('.pane-header-brand .thread-nav-group')) as HTMLElement | null;
      // The whole trailing cluster, not one button in it: at this width the
      // actions have folded into the ⋯ menu (ThreadHeaderActions), so naming
      // Search alone would find nothing. What must not overlap is the cluster,
      // whatever it currently holds.
      const actions = header.querySelector('.pane-header-brand-actions') as HTMLElement | null;
      const mark = header.querySelector('.pane-header-brand-label .brand-mark') as HTMLElement | null;
      const workspace = header.querySelector('.pane-header-brand-label .workspace-name-label') as HTMLElement | null;

      if (!brandLabel || !leftActions || !actions || !mark || !workspace) {
        return { error: 'missing elements' };
      }

      return {
        brandLabel: brandLabel.getBoundingClientRect(),
        leftActions: leftActions.getBoundingClientRect(),
        actions: actions.getBoundingClientRect(),
        mark: mark.getBoundingClientRect(),
        workspaceVisible: getComputedStyle(workspace).visibility !== 'hidden'
          && workspace.getBoundingClientRect().width > 0,
        workspaceClient: workspace.clientWidth,
        workspaceScroll: workspace.scrollWidth,
      };
    });

    expect(result).not.toHaveProperty('error');
    const r = result as Exclude<typeof result, { error: string }>;

    // The mark must start at or after the right edge of the left action group.
    expect(r.mark.left,
      `the mark starts at ${r.mark.left} but left actions end at ${r.leftActions.right}`)
      .toBeGreaterThanOrEqual(r.leftActions.right);

    // Brand label must not overlap the trailing actions.
    expect(rectsOverlap(r.brandLabel, r.actions),
      `Brand label ${JSON.stringify(r.brandLabel)} overlaps the actions ${JSON.stringify(r.actions)}`).toBe(false);

    // Whole or gone: a rendered name is never a clipped one.
    if (r.workspaceVisible) {
      expect(r.workspaceClient, 'the name was truncated instead of hidden')
        .toBeGreaterThanOrEqual(r.workspaceScroll);
    }

    // The mark is the one thing that never gives way.
    expect(r.mark.width, 'the mark was squeezed by the name beside it').toBeGreaterThan(0);
  });

  test('the name is whole until it does not fit, then gone, and the mark holds the centre', async ({ page }) => {
    // Walk the brand-label width through every tier and verify the user-visible
    // state at each one, so every intermediate state stays correct on every
    // commit.
    await page.setViewportSize({ width: 1400, height: 700 });
    await navigateToApp(page);
    await setLongWorkspaceName(page);

    type State = {
      markVisible: boolean;
      markCentred: boolean;
      workspaceVisible: boolean;
      workspaceTruncated: boolean;
      workspaceWidth: number;
      workspaceNatural: number;
    };

    // Measure the natural width of the part that cannot shrink (the mark's slot)
    // once at a wide brand-label, so the tier widths below are picked RELATIVE
    // to the live geometry instead of pixel constants that drift when CSS moves.
    // The measuring box is the cluster's flex MIDDLE (the chevrons take its
    // ends), so that is what the tiers below are expressed against.
    const naturals = await page.evaluate(() => {
      const box = document.querySelector('.desktop-header .pane-header-brand-center') as HTMLElement;
      box.style.flex = 'none';
      box.style.width = '500px';
      const slot = box.querySelector('.brand-mark-slot') as HTMLElement;
      const ws = box.querySelector('.workspace-name-label') as HTMLElement;
      const margin = (el: HTMLElement) => {
        const cs = getComputedStyle(el);
        return (parseFloat(cs.marginLeft) || 0) + (parseFloat(cs.marginRight) || 0);
      };
      return {
        markNatural: slot.scrollWidth + margin(slot),
        wsMargin: margin(ws),
        wsNatural: ws.scrollWidth,
      };
    });

    const measureAt = async (boxWidthPx: number): Promise<State> => {
      return await page.evaluate((w) => {
        const brandLabel = document.querySelector('.desktop-header .pane-header-brand-center') as HTMLElement;
        brandLabel.style.flex = 'none';
        brandLabel.style.width = `${w}px`;
        return new Promise<State>((resolve) => {
          // Two RAFs so the ResizeObserver in WorkspaceNameLabel runs and
          // updates is-hidden, then layout reflects it.
          requestAnimationFrame(() => requestAnimationFrame(() => {
            const mark = brandLabel.querySelector('.brand-mark') as HTMLElement;
            const workspace = brandLabel.querySelector('.workspace-name-label') as HTMLElement;
            const markRect = mark.getBoundingClientRect();
            const boxRect = brandLabel.getBoundingClientRect();
            const wsRect = workspace.getBoundingClientRect();
            resolve({
              markVisible: markRect.width > 0,
              // With the name gone the mark is the box's only content, so it
              // takes the box's own centre.
              markCentred: Math.abs(
                (markRect.left + markRect.right) / 2 - (boxRect.left + boxRect.right) / 2,
              ) < 1,
              workspaceVisible: getComputedStyle(workspace).visibility !== 'hidden' && wsRect.width > 0,
              workspaceTruncated: workspace.clientWidth < workspace.scrollWidth,
              workspaceWidth: workspace.clientWidth,
              workspaceNatural: workspace.scrollWidth,
            });
          }));
        });
      }, boxWidthPx);
    };

    // The width that exactly hosts the mark, the gap and the whole name.
    const exact = naturals.markNatural + naturals.wsMargin + naturals.wsNatural;

    // Tier 1: room for the whole name. It is shown, and shown WHOLE.
    const wide = await measureAt(Math.ceil(exact) + 40);
    expect(wide.markVisible, 'tier 1 (wide): mark visible').toBe(true);
    expect(wide.workspaceVisible, 'tier 1 (wide): workspace visible').toBe(true);
    expect(wide.workspaceTruncated, 'tier 1 (wide): workspace not truncated').toBe(false);

    // Tier 2: one step too narrow for the whole name. It goes, rather than
    // ellipsising, and the mark takes the box's centre.
    const tier2 = await measureAt(Math.floor(exact) - 20);
    expect(tier2.workspaceVisible,
      `tier 2 (mark alone): workspace hidden (width=${tier2.workspaceWidth})`).toBe(false);
    expect(tier2.markVisible, 'tier 2: mark visible').toBe(true);
    expect(tier2.markCentred, 'tier 2: the mark holds the centre once the name is gone').toBe(true);
  });

  test('the desktop menu is the fallback: the name is in it even when the bar cannot show it', async ({ page }) => {
    await navigateToApp(page);
    await assertMenuNameTruncates(page, '.desktop-header [data-role="brand-menu-toggle"]');
  });
});
