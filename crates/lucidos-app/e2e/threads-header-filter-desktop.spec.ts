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
      const selector = header.querySelector('button[aria-label="Switch thread view"]');
      const rect = (el: HTMLElement | null) => el ? el.getBoundingClientRect() : null;
      return {
        titleTextAlign: title ? getComputedStyle(title).textAlign : '',
        titleLeft: rect(title)?.left ?? 0,
        filterWidth: rect(filter)?.width ?? 0,
        filterRight: rect(filter)?.right ?? 0,
        hasSeparateSelector: !!selector,
      };
    });

    const empty = await measure();
    expect(empty, 'visible threads-header').not.toBeNull();
    // The view selector has been merged into the Filter control — there is no
    // separate "Switch thread view" button anymore.
    expect(empty!.hasSeparateSelector, 'no separate view-selector button').toBe(false);
    expect(empty!.filterWidth, 'single Filter button is visible').toBeGreaterThan(20);
    // The Filter button sits left of the Threads title box (the title is flex:1,
    // so its box starts right after the button).
    expect(empty!.filterRight, 'Filter button sits left of the Threads title')
      .toBeLessThanOrEqual(empty!.titleLeft + 1);
    // The title centres in the gap between the Filter button and the Search icon
    // (079672700 — "center Threads title between Filter and Search icons").
    expect(empty!.titleTextAlign, 'Threads title text centres between Filter and Search').toBe('center');

    // The needs-attention badge is absolutely positioned, so even a draft that
    // surfaces per-view counts in the menu must not move the title.
    const input = await waitForVisibleInput(page);
    await input.fill('an unsent draft to surface the drafts count');
    await page.waitForTimeout(100);
    const withDraft = await measure();
    expect(Math.abs(withDraft!.titleLeft - empty!.titleLeft), 'Threads title moved when a draft appeared')
      .toBeLessThan(1);
  });

  test('the title never runs under a header control, on either desktop build, at the drawer floor', async ({ page }) => {
    // The packaged macOS build indents the whole row so its leading control
    // clears the traffic lights, which leaves the least room the title ever
    // gets. It used to lift the Filter button OUT of the row instead
    // (`position: absolute` beside the lights, with the row reserving its
    // footprint back), and the flex-centred title took that space anyway,
    // printing the funnel glyph through the word. Flex siblings cannot overlap,
    // so this now guards the arrangement rather than a reserve, on the build
    // where the row is tightest. Simulated by stamping what
    // `titlebar_inset_script` stamps: nothing in the CSS keys off Tauri itself,
    // so this is the same geometry the packaged webview lays out.
    await sizeAndOpen(page);
    await page.locator('.threads-header button[aria-label="Filter threads"]').click();
    await expect(page.locator('.thread-drawer .thread-filter-panel')).toBeVisible();

    // The TITLE's BOX, not its text run: the text is clipped to the box with an
    // ellipsis, so a range measurement reports the unclipped extent and would
    // read a legitimately truncated title as an overlap. The box is the
    // structural property anyway. That is exactly what broke: with the button
    // out of flow the title's box covered it, where a flex sibling cannot.
    const measure = () => page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement | undefined;
      if (!header) return null;
      const title = header.querySelector('.threads-header-title') as HTMLElement | null;
      if (!title) return null;
      const box = title.getBoundingClientRect();
      const overlap = (sel: string) => {
        const el = header.querySelector(sel) as HTMLElement | null;
        if (!el) return -1;
        const r = el.getBoundingClientRect();
        return Math.max(0, Math.min(r.right, box.right) - Math.max(r.left, box.left));
      };
      const search = header.querySelector('button[aria-label="Search threads"]') as HTMLElement;
      return {
        drawerWidth: header.getBoundingClientRect().width,
        filter: overlap('button[aria-label="Filter threads"]'),
        search: overlap('button[aria-label="Search threads"]'),
        titleWidth: box.width,
        searchInside: search.getBoundingClientRect().right
          <= header.getBoundingClientRect().right + 1,
      };
    });

    const divider = page.locator('.drawer-divider');
    for (const overlay of [false, true]) {
      const build = overlay ? 'packaged macOS' : 'web';
      // Stamp the build FIRST: the two rows lay out differently (the packaged
      // one indents past the traffic lights), so a drag run before the attribute
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
      // job. 300 rather than the exact 312, so a UI-scale default or a row
      // tweak does not re-tune this test: what it guards is the overlap below.
      expect(g!.drawerWidth, `${build}: the drawer stayed below its floor`)
        .toBeGreaterThan(300);
      expect(g!.titleWidth, `${build}: the title has no room left at all`)
        .toBeGreaterThan(20);
      expect(g!.searchInside, `${build}: the Search button is outside the drawer`).toBe(true);
      expect(g!.filter, `${build}: title overlaps the Filter button`).toBe(0);
      expect(g!.search, `${build}: title overlaps the Search button`).toBe(0);
    }
  });

  test('opens the merged Status + Thread type panel IN THE DRAWER PANE; picking a status closes it; a status greys the type section', async ({ page }) => {
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
    // button is the way out, wearing an X while the panel is up (asserted in its
    // own test below).
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Filters');
    await expect(panel.locator('.thread-filter-panel-header')).toHaveCount(0);
    await expect(panel.locator('.thread-filter-panel-footer')).toHaveCount(0);
    await expect(panel.locator('.thread-filter-close')).toHaveCount(0);

    // Two headings and one rule: everything either side of the rule is a single
    // set, and both sections are named.
    expect(await panel.locator('.thread-filter-title').allTextContents())
      .toEqual(['Status', 'By thread types']);
    await expect(panel.locator('.thread-filter-or')).toHaveText('or');
    // No hairline between "Include deleted" and that heading: a heading already
    // opens a section.
    await expect(panel.locator('.thread-filter-divider')).toHaveCount(0);

    // Five options: the four real statuses, then "All statuses" under the rule.
    // The thread types below are NOT a sixth, they narrow this one.
    const labels = await panel.locator('.drawer-view-option .drawer-view-label').allTextContents();
    expect(labels).toEqual([
      'Needs attention', 'Review', 'Running', 'Drafts', 'All statuses',
    ]);

    // The channel section is a named group, and its knobs are live. They are
    // live in EVERY view (asserted under a status view further down); here that
    // is merely the unremarkable case.
    const types = panel.locator('div.thread-filter-types');
    await expect(types).toHaveAttribute('role', 'group');
    await expect(types).not.toHaveClass(/thread-filter-types-dimmed/);
    const firstType = types.locator('input[type="checkbox"]').first();
    await expect(firstType).toBeEnabled();

    // Excluding the child rows keeps the Lucidos locator off a repo or app that
    // happens to share the channel's name.
    const allStatuses = panel.locator('.drawer-view-option', { hasText: 'All statuses' });
    const typesHeading = panel.locator('.thread-filter-title', { hasText: 'By thread types' });
    const lucidos = panel.locator(
      '.thread-filter-option:not(.thread-filter-option-child)', { hasText: 'Lucidos' },
    );
    const includeDeleted = panel.locator(
      'label.thread-filter-option', { hasText: 'Include deleted' },
    );
    await expect(allStatuses).toHaveAttribute('aria-checked', 'true');
    await expect(panel.locator('.drawer-view-suffix')).toHaveCount(0);
    await expect(panel.locator('button[aria-label="About Filtered"]')).toHaveCount(0);

    // "Include deleted" is off by default, and on this workspace nothing has
    // been deleted, so it EXCLUDES nothing and the row stays quiet either way.
    // What decides is the difference between what is SHOWN and all of it, never
    // whether a switch is at its widest. (The store-level cases, including a
    // deleted option that IS held back, are in
    // `store/threadFilterActive.test.ts`; there is no deleted trigger, repo or
    // app to make here without seeding one.)
    await includeDeleted.click();
    await expect(panel.locator('.drawer-view-suffix')).toHaveCount(0);
    await includeDeleted.click();
    await expect(panel.locator('.drawer-view-suffix')).toHaveCount(0);

    // Dropping a channel NARROWS All statuses rather than picking something
    // else, so the checkmark stays put and the row says "filtered": what is
    // being shown now differs from all of it. No parentheses, and the explainer
    // rides inside the note. The heading takes the accent AND its own checkmark,
    // which is a different element from the single-select set's mark.
    await lucidos.click();
    await expect(allStatuses).toHaveAttribute('aria-checked', 'true');
    await expect(panel.locator('.drawer-view-suffix')).toHaveText(/^filtered$/);
    await expect(panel.locator('.drawer-view-suffix button[aria-label="About Filtered"]'))
      .toHaveCount(1);
    await expect(typesHeading).toHaveClass(/thread-filter-title-active/);
    await expect(panel.locator('.thread-filter-title-check')).toHaveCount(1);
    // The single-select set's own mark stays on All statuses: the heading's is
    // a different element, so exactly one of each and no doubling up there.
    await expect(panel.locator('.drawer-view-check')).toHaveCount(1);

    // Taking All statuses closes the panel and KEEPS the types narrowing it:
    // one choice, so the row cannot throw away the state it is describing.
    await allStatuses.click();
    await expect(panel).toHaveCount(0);
    await filterBtn.click();
    await expect(panel).toBeVisible();
    await expect(panel.locator(
      '.thread-filter-option:not(.thread-filter-option-child)', { hasText: 'Lucidos' },
    ).locator('input[type="checkbox"]')).not.toBeChecked();
    await expect(typesHeading).toHaveClass(/thread-filter-title-active/);
    await expect(panel.locator('.drawer-view-suffix')).toHaveText(/^filtered$/);

    // Picking a status applies it and closes the panel (a terminal choice, and
    // closing is what reveals the list it just filtered). Deliberately taken
    // while the type selection is STILL narrow, so reopening exercises the gate
    // below rather than a neutral filter.
    await panel.locator('.drawer-view-option', { hasText: 'Review' }).click();
    await expect(panel).toHaveCount(0);

    // Reopening under a non-All status shows the type section DIMMED in place,
    // since that view bypasses it. Dim only: every knob in it stays live, here
    // and in "Include deleted", which sits outside the group (under All
    // statuses, inside the radiogroup) and dims to match.
    await filterBtn.click();
    await expect(panel).toBeVisible();
    await expect(types).toHaveClass(/thread-filter-types-dimmed/);
    await expect(types.locator('input[type="checkbox"]').first()).toBeEnabled();
    await expect(panel.locator(
      'label.thread-filter-option', { hasText: 'Include deleted' },
    ).locator('input[type="checkbox"]')).toBeEnabled();
    // The heading dims WITH its section and keeps both cues under the dim: the
    // Lucidos channel is still off, and a knob the user can still reach has to
    // keep reading as set. What the dim says is "not shaping this list", never
    // "not available".
    await expect(typesHeading).toHaveClass(/thread-filter-title-dimmed/);
    await expect(typesHeading).toHaveClass(/thread-filter-title-active/);
    await expect(panel.locator('.thread-filter-title-check')).toHaveCount(1);
    // The row's own note is gated on the view, though: it reports what is on
    // screen, and what is on screen here is Review.
    await expect(panel.locator('.drawer-view-suffix')).toHaveCount(0);
    await expect(panel.locator('button[aria-label="About Filtered"]')).toHaveCount(0);

    // The point of leaving them live: set the types you want from here and take
    // All statuses in one move. Ticking Lucidos back on under Review applies
    // immediately (the heading's cues clear) and does NOT close the panel, which
    // is what makes it one move rather than a trip through the `all` view.
    await lucidos.click();
    await expect(panel).toBeVisible();
    await expect(lucidos.locator('input[type="checkbox"]')).toBeChecked();
    await expect(typesHeading).not.toHaveClass(/thread-filter-title-active/);
    await expect(panel.locator('.thread-filter-title-check')).toHaveCount(0);

    // Take it off again from here, then take All statuses: the pick made under
    // a status view is what the `all` view lands on.
    await lucidos.click();
    await panel.locator('.drawer-view-option', { hasText: 'All statuses' }).click();
    await expect(panel).toHaveCount(0);
    await filterBtn.click();
    await expect(panel).toBeVisible();
    await expect(lucidos.locator('input[type="checkbox"]')).not.toBeChecked();
    await expect(panel.locator('.drawer-view-suffix')).toHaveText(/^filtered$/);

    // Tick the channel on again so the panel is left as this test found it.
    await lucidos.click();
    await expect(panel.locator('.drawer-view-suffix')).toHaveCount(0);
    await expect(panel.locator('.thread-filter-title', { hasText: 'By thread types' }))
      .not.toHaveClass(/thread-filter-title-active/);
    await expect(panel.locator('.thread-filter-title-check')).toHaveCount(0);

    await page.keyboard.press('Escape');
    await expect(panel).toHaveCount(0);
  });

  test('the Filter glyph becomes an X while the panel is up, and that X is the way out', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const panel = page.locator('.thread-drawer .thread-filter-panel');

    // Closed, the button wears the funnel (FilterIcon's three lines), the glyph
    // for the default `all` status.
    await expect(filterBtn.locator('svg line')).toHaveCount(3);
    await expect(filterBtn).toHaveAttribute('aria-expanded', 'false');

    await filterBtn.click();
    await expect(panel).toBeVisible();

    // Open, it wears the X (CloseIcon's two crossed paths). The panel dropped its
    // Close footer, so this button and Escape are the only exits and the glyph
    // has to say which one it is.
    await expect(filterBtn.locator('svg path')).toHaveCount(2);
    await expect(filterBtn.locator('svg path').first()).toHaveAttribute('d', 'M18 6 6 18');
    // The accessible NAME does not change with it: this is a disclosure, and
    // aria-expanded is what carries the state.
    await expect(filterBtn).toHaveAttribute('aria-expanded', 'true');

    // Pressing the X closes the panel, and closing is not a commit: the list is
    // back, the pane title says so, and the funnel is back on the button.
    await filterBtn.click();
    await expect(panel).toHaveCount(0);
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Threads');
    await expect(page.locator('.thread-drawer .thread-drawer-list')).toBeVisible();
    await expect(filterBtn.locator('svg line')).toHaveCount(3);
  });

  test('the X sheds the filtered highlight, and picking a status back up puts it on', async ({ page }) => {
    // While the panel is open the button is an exit, not a status line: the
    // panel underneath is already saying what the filter is, so the highlight
    // (and the needs-attention badge with it) comes off the glyph the user is
    // about to press.
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const panel = page.locator('.thread-drawer .thread-filter-panel');

    // Put a filter on: picking a status applies it and closes the panel, so the
    // button comes back highlighted.
    await filterBtn.click();
    await panel.locator('.drawer-view-option', { hasText: 'Review' }).click();
    await expect(panel).toHaveCount(0);
    await expect(filterBtn).toHaveClass(/view-selector-active/);

    // Reopening drops the highlight even though the filter is still on.
    await filterBtn.click();
    await expect(panel).toBeVisible();
    await expect(filterBtn).not.toHaveClass(/view-selector-active/);
    await expect(filterBtn.locator('.badge')).toHaveCount(0);

    // Closing hands the filter's own state back to the button.
    await filterBtn.click();
    await expect(panel).toHaveCount(0);
    await expect(filterBtn).toHaveClass(/view-selector-active/);
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
      const panel = document.querySelector('.thread-drawer .thread-filter-panel')!;
      const statusRow = panel.querySelector('.drawer-view-option')!;
      const typeRow = panel.querySelector('.thread-filter-option')!;
      const heading = panel.querySelector('.thread-filter-title')!;
      // The list's own column: the x a thread name starts at (`.thread-row`'s
      // padding-left at depth 0), read off the rule rather than hardcoded.
      const row = document.createElement('div');
      row.className = 'thread-row';
      list.appendChild(row);
      const listColumn = px(row, 'padding-left');
      row.remove();
      return {
        listColumn,
        panelTop: px(panel, 'padding-top'),
        listTop: px(list, 'padding-top'),
        panelLeft: px(panel, 'padding-left'),
        statusLeft: px(statusRow, 'padding-left'),
        typeLeft: px(typeRow, 'padding-left'),
        headingLeft: px(heading, 'padding-left'),
      };
    });

    // The panel takes the list's vertical padding and adds no gutter of its own,
    // so every row's inset is the row's own padding, as in the list.
    expect(geometry.panelTop).toBe(geometry.listTop);
    expect(geometry.panelLeft).toBe(0);
    // Both row families (the single-select rows and the checkbox rows) and the
    // section heading all start where a thread name does.
    expect(geometry.statusLeft).toBe(geometry.listColumn);
    expect(geometry.typeLeft).toBe(geometry.listColumn);
    expect(geometry.headingLeft).toBe(geometry.listColumn);
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
    await expect(panel).toHaveCount(0);
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Threads');

    // And it opens again on the next click.
    await filterBtn.click();
    await expect(panel).toBeVisible();
  });

  test('closing the drawer closes the panel, so nothing invisible holds Escape', async ({ page }) => {
    await sizeAndOpen(page);

    const filterBtn = page.locator('.threads-header button[aria-label="Filter threads"]');
    const panel = page.locator('.thread-drawer .thread-filter-panel');
    await filterBtn.click();
    await expect(panel).toBeVisible();

    // Hide the whole pane the panel is a view of. Its state is a signal and it
    // holds an Escape-registry entry, so leaving it "open" behind a hidden
    // drawer would eat the user's next Escape and reopen onto the filter.
    // The desktop toggle is one element in both drawer states, but the mobile
    // header's copy stays mounted under a desktop viewport, so click whichever
    // one is actually on screen.
    const toggled = await clickVisibleElement(page, 'button[aria-label^="Show or hide thread drawer"]');
    expect(toggled, 'drawer toggle was visible').toBe(true);
    await expect(page.locator('.thread-filter-panel')).toHaveCount(0);

    // Reopening lands on the thread list, and the title says so.
    await openThreadDrawer(page);
    await expect(page.locator('.thread-filter-panel')).toHaveCount(0);
    await expect(page.locator('.threads-header .threads-header-title')).toHaveText('Threads');
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
    await expect(panel).toHaveCount(0);
  });
});
