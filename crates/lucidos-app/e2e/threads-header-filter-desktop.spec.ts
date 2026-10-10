import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  waitForVisibleInput,
  openThreadDrawer,
  clickVisibleElement,
} from './helpers';
import { clearAllThreads } from './db-helpers';

// Desktop-only layout test for the threads-header unified Filter control. The
// `.threads-header` (drawer header) only renders on desktop and depends on
// `page.setViewportSize()` actually changing the layout — which mobile-emulated
// projects ignore (they pin the iPhone viewport via `isMobile: true`). Living in
// a `-desktop.spec.ts` file excludes it from those projects
// (`testIgnore: /-desktop\.spec\.ts$/`).

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
    // The drawer/header width animates for var(--duration-slow) (300ms). Settle
    // before measuring so geometry isn't mixed with the drawer-open transition.
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
    // The view selector has been merged into the Filter control: there is no
    // separate "Switch thread view" button anymore.
    expect(empty!.hasSeparateSelector, 'no separate view-selector button').toBe(false);
    expect(empty!.filterWidth, 'single Filter button is visible').toBeGreaterThan(20);
    // Filter leads the row, just past the drawer toggle, left of the title.
    // Grouping sits just left of Search at the trailing end.
    expect(empty!.filterRight, 'Filter button sits left of the pane title')
      .toBeLessThanOrEqual(empty!.titleLeft + 1);
    const groupingToSearch = empty!.searchLeft - empty!.groupingRight;
    expect(groupingToSearch, 'Grouping button sits just left of Search').toBeGreaterThanOrEqual(0);
    expect(groupingToSearch, 'Grouping button sits just left of Search').toBeLessThanOrEqual(8);
    // The title is centred on the drawer pane (split-resize-desktop.spec.ts
    // measures that); its text centres in its own box.
    expect(empty!.titleTextAlign, 'Threads title text centres in its box').toBe('center');

    // A draft changes the drawer's rows and counts, never the header row, so
    // the title must not move.
    const input = await waitForVisibleInput(page);
    await input.fill('an unsent draft that adds a drawer row');
    await page.waitForTimeout(100);
    const withDraft = await measure();
    expect(Math.abs(withDraft!.titleLeft - empty!.titleLeft), 'Threads title moved when a draft appeared')
      .toBeLessThan(1);
  });

  test('the title never runs under a header control, on either desktop build, at the drawer floor', async ({ page }) => {
    // The packaged macOS build rests the drawer toggle past the traffic lights.
    // The row keeps that whole lead free for it, so the title gets the least
    // room there. The title is centred on the pane and clamped to clear the
    // wider of the row's two ends: the toggle's room and Filter, or Grouping
    // and Search. A word that outgrows that room hides, never ellipsizes.
    // This checks it at the drawer floor, on both builds.
    // Simulated by stamping what `titlebar_inset_script` stamps: nothing in the
    // CSS keys off Tauri itself, so this is the geometry the webview lays out.
    await sizeAndOpen(page);
    await page.locator('.threads-header button[aria-label="Filter threads"]').click();
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();

    // The TITLE's BOX, not its text run: the box is the structural property,
    // since the clamp is what keeps it clear. A cramped title is hidden, so it
    // overlaps nothing on screen whatever its box does.
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
        // The toggle is not a member of the row: it rests over the row's
        // leading end from the header, so it is found from the header.
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
    // 100% and 125% UI scale: the floor and the clamp both scale with the root.
    for (const scale of [100, 125]) for (const overlay of [false, true]) {
      const build = `${overlay ? 'packaged macOS' : 'web'} at ${scale}%`;
      await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);
      // Stamp the build FIRST: the two rows lay out differently (the packaged
      // one leads past the traffic lights), so a drag run before the attribute
      // would measure the other build's row. The FLOOR is the same on both
      // (ADR 0058), which is why one `toBeGreaterThan` covers the pair.
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

      // Drag the drawer hard past its floor. The clamp refuses to follow the
      // pointer there (ADR 0056), so the drawer is left AT the narrowest width
      // it can rest at, which is what the measurement below wants.
      const box = await divider.boundingBox();
      expect(box, `${build}: drawer divider is visible`).not.toBeNull();
      await page.mouse.move(box!.x + box!.width / 2, box!.y + 100);
      await page.mouse.down();
      await page.mouse.move(150, box!.y + 100, { steps: 12 });
      await page.mouse.up();
      // The geometry transition, with room to spare. Nothing moves after
      // release under the clamp (ADR 0056); the wait is for the drag itself.
      await page.waitForTimeout(1200);

      const g = await measure();
      expect(g, `${build}: visible threads-header`).not.toBeNull();
      // The clamp refused to follow the pointer, which is the floor doing its
      // job. 300 rather than the exact 320, so a UI-scale default or a row
      // tweak does not re-tune this test: what it guards is the overlap below.
      expect(g!.drawerWidth, `${build}: the drawer stayed below its floor`)
        .toBeGreaterThan(300);
      expect(g!.searchInside, `${build}: the Search button is outside the drawer`).toBe(true);
      // Whole or hidden, never an ellipsis. The web row has room for "Filters"
      // at the floor; the packaged row, past the lights, has none.
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

    // The panel is a view inside the drawer pane, NOT a popout in the header.
    const panel = page.locator('.thread-drawer .thread-filter-panel');
    await expect(panel).toBeVisible();
    await expect(page.locator('.thread-filter-panel')).toHaveCount(1);
    await expect(page.locator('.threads-header .thread-filter-panel')).toHaveCount(0);

    // The pane header names what the pane is showing, so the panel needs no
    // title row of its own. It needs no footer either: the header's Filter
    // button is the way out, held down while the panel is up (asserted in its
    // own test below).
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Filters');
    await expect(panel.locator('.thread-filter-panel-header')).toHaveCount(0);
    await expect(panel.locator('.thread-filter-panel-footer')).toHaveCount(0);
    await expect(panel.locator('.thread-filter-close')).toHaveCount(0);

    // The type rows, then "Include deleted", and nothing else. No heading: the
    // pane title already says Filters. No hairline either.
    await expect(panel.locator('.thread-filter-title')).toHaveCount(0);
    expect(await panel.evaluate((p) => Array.from(p.children).map(c => c.classList[0]))).toEqual([
      'thread-filter-types', 'thread-filter-option',
    ]);
    await expect(panel.locator('.thread-filter-divider')).toHaveCount(0);

    // The channel section is a named group, and its knobs are live.
    const types = panel.locator('div.thread-filter-types');
    await expect(types).toHaveAttribute('role', 'group');
    await expect(types).toHaveAttribute('aria-label', 'Thread types');
    await expect(types.locator('input[type="checkbox"]').first()).toBeEnabled();

    // Excluding the child rows keeps the Lucidos locator off a repo or app that
    // happens to share the channel's name.
    const lucidos = panel.locator(
      '.thread-filter-option:not(.thread-filter-option-child)', { hasText: 'Lucidos' },
    );
    const includeDeleted = panel.locator(
      'label.thread-filter-option', { hasText: 'Include deleted' },
    );
    // "Include deleted" sits after the types group, outside it.
    await expect(types.locator('label', { hasText: 'Include deleted' })).toHaveCount(0);
    // The checkbox itself: the row's centre can land on its explainer icon,
    // which opens the explainer instead.
    const includeDeletedBox = includeDeleted.locator('input[type="checkbox"]');
    await includeDeletedBox.click();
    await expect(includeDeletedBox).toBeChecked();
    await includeDeletedBox.click();
    await expect(includeDeletedBox).not.toBeChecked();

    // Dropping a channel narrows the list, and the panel stays up: a type is
    // not a terminal pick.
    await lucidos.click();
    await expect(panel).toBeVisible();
    await expect(lucidos.locator('input[type="checkbox"]')).not.toBeChecked();

    // Closing keeps the narrowing: reopening finds it as it was left.
    await filterBtn.click();
    await expect(panel).toBeHidden();
    await filterBtn.click();
    await expect(panel).toBeVisible();
    await expect(lucidos.locator('input[type="checkbox"]')).not.toBeChecked();

    // Tick the channel on again so the panel is left as this test found it.
    await lucidos.click();
    await expect(lucidos.locator('input[type="checkbox"]')).toBeChecked();

    await page.keyboard.press('Escape');
    await expect(panel).toBeHidden();
  });

  test('the Filter button is held down while the panel is up, and pressing it again is the way out', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    // The glyph on show: the button keeps every glyph mounted for its crossfade.
    const glyph = filterBtn.locator('.crossfade-layer[data-current] svg');
    const panel = page.locator('.thread-drawer .thread-filter-panel');

    // Closed, the button wears the plain funnel (one outline, not filled), the
    // glyph for an unfiltered `all` view, and it is not pressed.
    await expect(glyph.locator('path')).toHaveCount(1);
    await expect(glyph.locator('line, circle')).toHaveCount(0);
    await expect(glyph).toHaveAttribute('fill', 'none');
    const funnel = await glyph.innerHTML();
    await expect(filterBtn).not.toHaveClass(/view-selector-active/);
    await expect(filterBtn).toHaveAttribute('aria-expanded', 'false');

    await filterBtn.click();
    await expect(panel).toBeVisible();

    // Open, it is the same funnel, pressed, and no X. At the far end of the
    // header an X reads as "close this pane".
    expect(await glyph.innerHTML(), 'opening swapped the funnel').toBe(funnel);
    await expect(filterBtn).toHaveClass(/view-selector-active/);
    // The accessible NAME does not change with it: this is a disclosure, and
    // aria-expanded is what carries the state.
    await expect(filterBtn).toHaveAttribute('aria-expanded', 'true');

    // Pressing it again closes the panel, and closing is not a commit: the list
    // is back, the pane title says so, and the button is released.
    await filterBtn.click();
    await expect(panel).toBeHidden();
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Folders');
    await expect(page.locator('.thread-drawer .thread-drawer-list')).toBeVisible();
    expect(await glyph.innerHTML()).toBe(funnel);
    await expect(filterBtn).not.toHaveClass(/view-selector-active/);
  });

  test('pressed means the panel is open, and the glyph says whether types narrow the list', async ({ page }) => {
    // A filtered list must not look like a panel left open. So the highlight
    // follows the panel alone, and the glyph carries the filter, open or closed.
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    // The glyph on show: the button keeps every glyph mounted for its crossfade.
    const glyph = filterBtn.locator('.crossfade-layer[data-current] svg');
    const panel = page.locator('.thread-drawer .thread-filter-panel');
    await expect(glyph).toHaveAttribute('fill', 'none');
    const funnel = await glyph.innerHTML();

    // Narrowing by thread type fills the funnel, the same glyph pressed or
    // released.
    await filterBtn.click();
    await expect(filterBtn).toHaveClass(/view-selector-active/);
    const lucidos = panel.locator(
      '.thread-filter-option:not(.thread-filter-option-child)', { hasText: 'Lucidos' },
    );
    await lucidos.click();
    await expect(glyph).toHaveAttribute('fill', 'currentColor');
    await filterBtn.click();
    await expect(panel).toBeHidden();
    await expect(filterBtn).not.toHaveClass(/view-selector-active/);
    await expect(glyph).toHaveAttribute('fill', 'currentColor');

    // The filter shapes Folders only. Under Ongoing the button fades out in
    // its slot. Back under Folders, the glyph still reports the narrowing.
    const slot = page.locator('.threads-header .filter-slot');
    await page.locator('.threads-header .grouping-btn').click();
    await expect(slot).not.toHaveAttribute('data-shown');
    await expect(filterBtn).toBeHidden();
    await page.locator('.threads-header .grouping-btn').click();
    await expect(slot).toHaveAttribute('data-shown');
    await expect(filterBtn).toBeVisible();
    await expect(glyph).toHaveAttribute('fill', 'currentColor');

    // Tick it back on, so the test leaves the list as it found it.
    await filterBtn.click();
    await lucidos.click();
    await expect(glyph).toHaveAttribute('fill', 'none');
    await filterBtn.click();
    await expect(panel).toBeHidden();
    expect(await glyph.innerHTML()).toBe(funnel);
  });

  test('the panel sits on the thread list own column: same left inset, same edges', async ({ page }) => {
    // The panel covers the list inside one pane, so a filter row that started on
    // a different x than the thread names under it read as a different surface
    // rather than as this pane showing something else.
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    await filterBtn.click();
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();

    const geometry = await page.evaluate(() => {
      const px = (el: Element, prop: string) => parseFloat(getComputedStyle(el).getPropertyValue(prop));
      const list = document.querySelector('.thread-drawer .thread-drawer-list')!;
      // The cover is the box the panel shows in, so it carries the padding.
      const cover = document.querySelector('.thread-drawer .thread-filter-cover')!;
      const panel = cover.querySelector('.thread-filter-panel')!;
      const typeRow = panel.querySelector('.thread-filter-types .thread-filter-option')!;
      // "Include deleted", the one row outside the types group.
      const deletedRow = panel.querySelector(':scope > .thread-filter-option')!;
      // The list's own column: the x a thread name starts at (`.thread-row`'s
      // padding-left at depth 0), read off the rule rather than hardcoded.
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

    // The panel takes the list's vertical padding and adds no gutter of its own,
    // so every row's inset is the row's own padding, as in the list.
    expect(geometry.panelTop).toBe(geometry.listTop);
    expect(geometry.panelLeft).toBe(0);
    // The type rows and "Include deleted" start where a thread name does.
    expect(geometry.typeLeft).toBe(geometry.listColumn);
    expect(geometry.deletedLeft).toBe(geometry.listColumn);
  });

  test('the Filter button opens reliably and toggles closed (Chrome open-bug regression)', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const panel = page.locator('.thread-drawer .thread-filter-panel');

    // Fresh click opens it (the old separate view selector failed to open here).
    await filterBtn.click();
    await expect(panel).toBeVisible();

    // Re-clicking the toggle closes it, and the pane title goes back.
    await filterBtn.click();
    await expect(panel).toBeHidden();
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Folders');

    // And it opens again on the next click.
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

    // Hide the whole pane the panel is a view of. The mobile header's toggle
    // stays mounted under a desktop viewport, so click whichever copy is on
    // screen.
    const toggled = await clickVisibleElement(page, 'button[aria-label^="Show or hide thread drawer"]');
    expect(toggled, 'drawer toggle was visible').toBe(true);

    // The panel stays open behind the hidden drawer, but an Escape must not
    // close a panel nobody can see.
    await expect(openCover).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(openCover).toHaveCount(1);

    // Reopening lands back on the filters, and Escape reaches them again.
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

    // Nothing floats over the rest of the app, so nothing behind is inert and no
    // click is swallowed to dismiss.
    await expect(page.locator('html[data-overlay-open]')).toHaveCount(0);

    // Click the composer: it must take focus. Focus is the exact property an
    // overlay would have destroyed, since the dismiss path preventDefaults the
    // paired click and focusing is that click's default action. (Asserting on
    // typed TEXT instead would race the composer's own first-keystroke
    // re-render, which drops a character.)
    const input = await waitForVisibleInput(page);
    await input.click();
    await expect(input).toBeFocused();

    // A pane view stays put until it is dismissed.
    await expect(panel).toBeVisible();

    // Opening search does put it away: they compete for the same pane body.
    await page.locator('.threads-header button[aria-label="Search threads"]').click();
    await expect(panel).toBeHidden();
  });
});
