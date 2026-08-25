/**
 * Mobile header titles are TRUE-centered on the row middle (like the desktop
 * header and the pane dots), NOT centered between the leading/trailing icon
 * clusters — an in-flow between-clusters title drifts off the row middle whenever
 * the two clusters differ in width, which reads as "off-center".
 *
 * They must also never overlap EITHER icon cluster: a title wide enough to reach
 * one clamps + ellipsizes so its edges stay clear (guaranteed structurally by
 * the absolute centering + a symmetric max-width reserve; the brand's long
 * workspace name truncates within the centered box). This guards both the
 * off-center regression (flanking-spacer centering) and the original overlap
 * regression (a title spilling over the leading icons).
 *
 * Both edges are asserted because the reserve is SYMMETRIC: it is sized off the
 * wider cluster, so a button added to one side spends the other side's
 * clearance. The thread header's trailing cluster grew to compose + search +
 * menu when the hamburger moved to that edge, which is what makes the
 * right-edge assertion load-bearing rather than decorative. Each addition
 * spends real clearance at 375px, so measure against whatever is currently
 * leftmost in that cluster (`TRAILING_LEFTMOST`) rather than naming one button.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, navigateToApp, openThreadDrawer, ensureMobileView } from './helpers';

/** The leftmost thing on the THREADS row's trailing side, whichever it
 *  currently is: what the centred title actually has to clear there. That is
 *  the Lucidos mark today, and it is nearer the middle than the icon run behind
 *  it, because the mark is pinned to the centred nav cluster's trailing edge
 *  (the forward chevron's column) rather than packed against the row's own
 *  edge. Clearing it therefore clears Search behind it for free.
 *
 *  Add something to its left and this becomes a selector list:
 *  `querySelector` takes the first match in DOCUMENT order, not in selector
 *  order, so anywhere in the list works and the measurement follows the header.
 *  Naming one control once that happens would leave the assertions passing
 *  while the title overlapped the one nobody was measuring.
 *
 *  The thread row no longer needs this: its trailing side is a single
 *  hamburger, which that case names directly. */
const TRAILING_LEFTMOST = '.brand-mark-slot';

interface Metrics {
  center: number;
  rowCenter: number;
  left: number;
  right: number;
  leadingRight: number;
  trailingLeft: number;
  clientWidth: number;
  scrollWidth: number;
  /** The title's COMPUTED `max-width`, i.e. the reserve as the browser read it.
   *  `none` means the declaration was dropped as invalid. */
  maxWidth: string;
}

/** Measure a header's centered title against its own row and BOTH icon
 *  clusters. `leadingSel` is the rightmost in-flow leading element,
 *  `trailingSel` the leftmost trailing one. Both edges matter because the
 *  reserve is symmetric: it is sized off whichever cluster is wider, so a button
 *  added to either side eats the other side's clearance too. Returns null if the
 *  header, its row, the title, or either cluster element isn't visible. */
async function measure(
  page: Page,
  headerSel: string,
  titleSel: string,
  leadingSel: string,
  trailingSel: string,
  text?: string,
): Promise<Metrics | null> {
  return page.evaluate(({ headerSel, titleSel, leadingSel, trailingSel, text }) => {
    const header = document.querySelector(headerSel) as HTMLElement | null;
    if (!header || header.getBoundingClientRect().width === 0) return null;
    const row = header.querySelector('.mobile-header-row') as HTMLElement | null;
    const leading = header.querySelector(leadingSel) as HTMLElement | null;
    const trailing = header.querySelector(trailingSel) as HTMLElement | null;
    if (!row || !leading || leading.getBoundingClientRect().width === 0) return null;
    if (!trailing || trailing.getBoundingClientRect().width === 0) return null;
    const rowRect = row.getBoundingClientRect();
    for (const el of header.querySelectorAll(titleSel)) {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0) continue;
      if (text != null && (el.textContent ?? '').trim() !== text) continue;
      const h = el as HTMLElement;
      return {
        center: rect.left + rect.width / 2,
        rowCenter: rowRect.left + rowRect.width / 2,
        left: rect.left,
        right: rect.right,
        leadingRight: leading.getBoundingClientRect().right,
        trailingLeft: trailing.getBoundingClientRect().left,
        clientWidth: h.clientWidth,
        scrollWidth: h.scrollWidth,
        maxWidth: getComputedStyle(h).maxWidth,
      };
    }
    return null;
  }, { headerSel, titleSel, leadingSel, trailingSel, text });
}

test.describe('Mobile header titles are centered and clear of the leading icons', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('titles sit on the row middle and never overlap the leading cluster', async ({ page }) => {
    await navigateToApp(page);

    // Thread header: the nav cluster (back chevron, Lucidos mark, forward
    // chevron) is centered on the row middle and clears BOTH edge controls, the
    // thread-drawer toggle leading and the menu hamburger trailing.
    //
    // This cluster is FIXED WIDTH, unlike the title it replaced: it cannot
    // ellipsise its way out of a collision, so the clearance below is a hard
    // guarantee rather than a truncation budget, and anything added to either
    // edge spends it directly.
    const brand = await measure(
      page, '.mobile-thread-header', '.header-nav-cluster', '.thread-toggle', '.hamburger-panel',
    );
    expect(brand, 'nav cluster / drawer toggle / hamburger not found').not.toBeNull();
    expect(
      Math.abs(brand!.center - brand!.rowCenter),
      `cluster center=${brand!.center} vs row center=${brand!.rowCenter}`,
    ).toBeLessThan(2.5);
    expect(
      brand!.left,
      `cluster left=${brand!.left} overlaps drawer toggle right=${brand!.leadingRight}`,
    ).toBeGreaterThanOrEqual(brand!.leadingRight - 0.5);
    expect(
      brand!.right,
      `cluster right=${brand!.right} overlaps hamburger left=${brand!.trailingLeft}`,
    ).toBeLessThanOrEqual(brand!.trailingLeft + 0.5);

    // The mark is the connection light, so its state has to be READABLE, not
    // just coloured: colour alone is invisible to a screen reader and to anyone
    // who cannot distinguish the hue.
    const markLabel = await page.evaluate(() => {
      const el = document.querySelector('.mobile-thread-header [data-role="brand-menu-toggle"]');
      return el?.getAttribute('aria-label') ?? null;
    });
    expect(markLabel, 'the mark has no accessible name').not.toBeNull();
    expect(markLabel!.toLowerCase()).toMatch(/connect/);

    // Threads header: "Threads" is centered on the row middle, not truncated, and
    // clear of the Filter control.
    await openThreadDrawer(page);
    const threads = await measure(
      page, '.mobile-threads-header', '.pane-header-title', '.view-selector-slot', TRAILING_LEFTMOST, 'Threads',
    );
    expect(threads, 'Threads title / filter slot not found').not.toBeNull();
    expect(
      Math.abs(threads!.center - threads!.rowCenter),
      `Threads center=${threads!.center} vs row center=${threads!.rowCenter}`,
    ).toBeLessThan(2.5);
    expect(
      threads!.left,
      `Threads title left=${threads!.left} overlaps filter right=${threads!.leadingRight}`,
    ).toBeGreaterThanOrEqual(threads!.leadingRight - 0.5);
    expect(
      threads!.right,
      `Threads title right=${threads!.right} overlaps trailing cluster left=${threads!.trailingLeft}`,
    ).toBeLessThanOrEqual(threads!.trailingLeft + 0.5);
    expect(
      threads!.clientWidth,
      `Threads title truncated (clientWidth=${threads!.clientWidth} < scrollWidth=${threads!.scrollWidth})`,
    ).toBeGreaterThanOrEqual(threads!.scrollWidth);

    // The reserve itself survived into the computed style. It is a nested
    // `calc()` over two custom properties now (it clears the mark on the nav
    // cluster's trailing edge, which is clamped, so it cannot be a rem
    // constant), and both ways of getting it wrong are SILENT: a calc the
    // parser rejects, and a `var()` naming a property nothing defines, each
    // leave `max-width: none`. That drops the only structural guarantee that a
    // title cannot cross the mark, while every assertion above still passes,
    // because this row's two titles ("Threads", "Filters") are short enough
    // never to reach it.
    //
    // `none` is the whole assertion, and the computed value must NOT be read as
    // a px length: the expression carries a percentage (the cluster clamps
    // against the row), a percentage in `max-width` survives into the computed
    // value, so the browser hands back the unresolved
    // `calc(… + clamp(…, 100% - …, …))` rather than a number.
    expect(
      threads!.maxWidth,
      'the Threads title reserve was dropped, so nothing bounds the title',
    ).not.toBe('none');
  });

  /** The two chevrons of whichever mobile header is on screen, as viewport
   *  positions. Null while the pane is not the visible one, since a header in a
   *  `display:none` section measures zero. */
  async function chevrons(
    page: Page, headerSel: string, backSel: string, forwardSel: string,
  ): Promise<{ backLeft: number; forwardRight: number; backTop: number } | null> {
    return page.evaluate(({ headerSel, backSel, forwardSel }) => {
      const header = document.querySelector(headerSel) as HTMLElement | null;
      const back = header?.querySelector(backSel) as HTMLElement | null;
      const forward = header?.querySelector(forwardSel) as HTMLElement | null;
      if (!back || !forward) return null;
      const b = back.getBoundingClientRect();
      const f = forward.getBoundingClientRect();
      if (b.width === 0 || f.width === 0) return null;
      return { backLeft: b.left, forwardRight: f.right, backTop: b.top };
    }, { headerSel, backSel, forwardSel });
  }

  /** The threads row's Lucidos mark, as a viewport x position: its right edge,
   *  which is the cluster's trailing edge it is pinned to. Null while that pane
   *  is not the visible one. */
  async function threadsMarkRight(page: Page): Promise<number | null> {
    return page.evaluate(() => {
      const el = document.querySelector('.mobile-threads-header .header-nav-cluster .brand-mark-slot');
      const rect = el?.getBoundingClientRect();
      return rect && rect.width > 0 ? rect.right : null;
    });
  }

  test('the nav cluster edges land in the same places on all three panes', async ({ page }) => {
    // The ask this pins: navigation must not move under the thumb when the user
    // swipes between panes. All three clusters are the same fixed-width centred
    // box, so the chevrons agree by construction; what could break it is a
    // per-pane width, a shift, or a change to a pane's edge clusters that pushes
    // the clamp off its preferred span on one row and not the other.
    await navigateToApp(page);

    // Both thread chevrons carry the same class, so they are told apart by
    // their accessible names; the content pair has a class each.
    await ensureMobileView(page, 'thread');
    const threadArgs = [
      '.mobile-thread-header', 'button[aria-label="Previous thread"]', 'button[aria-label="Next thread"]',
    ] as const;
    await expect
      .poll(() => chevrons(page, ...threadArgs), { timeout: 10_000, message: 'thread pane chevrons never laid out' })
      .not.toBeNull();
    const thread = (await chevrons(page, ...threadArgs))!;

    await ensureMobileView(page, 'content');
    const contentArgs = ['.mobile-content-header', '.content-back-btn', '.content-forward-btn'] as const;
    await expect
      .poll(() => chevrons(page, ...contentArgs), { timeout: 10_000, message: 'content pane chevrons never laid out' })
      .not.toBeNull();
    const content = (await chevrons(page, ...contentArgs))!;

    expect(
      Math.abs(thread.backLeft - content.backLeft),
      `back chevron: thread pane at ${thread.backLeft.toFixed(1)}, content pane at ${content.backLeft.toFixed(1)}`,
    ).toBeLessThan(1);
    expect(
      Math.abs(thread.forwardRight - content.forwardRight),
      `forward chevron: thread pane at ${thread.forwardRight.toFixed(1)}, content pane at ${content.forwardRight.toFixed(1)}`,
    ).toBeLessThan(1);

    // The threads pane carries no chevrons, so its cluster holds one member,
    // the Lucidos mark, at the same trailing edge. The mark is the only control
    // on all three rows, and it was the one that moved as the user swiped: it
    // rode the trailing icon run beside Search, roughly 45px right of this
    // column at 375px.
    await openThreadDrawer(page);
    await expect
      .poll(() => threadsMarkRight(page), { timeout: 10_000, message: 'the threads mark never laid out' })
      .not.toBeNull();
    const markRight = (await threadsMarkRight(page))!;
    expect(
      Math.abs(markRight - thread.forwardRight),
      `threads mark at ${markRight.toFixed(1)}, forward chevron at ${thread.forwardRight.toFixed(1)}`,
    ).toBeLessThan(1);
  });

  test('the chevrons sit on the same LINE on the thread and content panes', async ({ page }) => {
    // The other axis of the same ask, and it needs its own test because it only
    // shows up at a scaled root. `top: 50%` plus a `translateY(-50%)` placed
    // each cluster by half its OWN height, and the two rows fill theirs
    // differently: the thread row's box is the mark (2.1rem), the content row's
    // is a chevron (1.75rem). Each rounded its own half, and the pair landed
    // 0.14px apart at the 18px root. A fifth of a device pixel moves a hairline
    // chevron's anti-aliasing, and the user saw the chevrons hop as they
    // swiped. Both clusters span their row now, so the box's height is out of
    // the placement and flexbox centres both pairs against one identical box.
    await navigateToApp(page);

    // 112.5% is the mobile stylesheet's own fallback root and a supported user
    // setting. It is also the one this reproduced at. At the 100% the suite
    // otherwise runs at, the same defect rounds to nothing, so a test taking
    // the default scale would pass over it.
    await page.evaluate(() => {
      document.documentElement.style.setProperty('--user-ui-scale', '112.5%');
    });

    await ensureMobileView(page, 'thread');
    const threadArgs = [
      '.mobile-thread-header', 'button[aria-label="Previous thread"]', 'button[aria-label="Next thread"]',
    ] as const;
    await expect
      .poll(() => chevrons(page, ...threadArgs), { timeout: 10_000, message: 'thread pane chevrons never laid out' })
      .not.toBeNull();
    const thread = (await chevrons(page, ...threadArgs))!;

    await ensureMobileView(page, 'content');
    const contentArgs = ['.mobile-content-header', '.content-back-btn', '.content-forward-btn'] as const;
    await expect
      .poll(() => chevrons(page, ...contentArgs), { timeout: 10_000, message: 'content pane chevrons never laid out' })
      .not.toBeNull();
    const content = (await chevrons(page, ...contentArgs))!;

    // The scale is what gives this test the power to fail, so prove it held.
    // The app writes `--user-ui-scale` itself (store/actions/preferences.ts),
    // and a preference load landing late takes the root back to 16px, where
    // the defect rounds to nothing. A revert BETWEEN the two measurements
    // above fails loudly, on the large difference it makes. One before either
    // is the case that would pass in silence, and this is what catches it.
    expect(
      await page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
      'the root left the scaled size, so the measurements above guard nothing',
    ).toBe('18px');

    // Tight, because the two are identical by construction rather than merely
    // close: one box, one header element, nothing left to round differently.
    // This absorbs float noise and nothing else. Widen it and the drift comes
    // back invisible.
    expect(
      Math.abs(thread.backTop - content.backTop),
      `chevron top: thread pane at ${thread.backTop.toFixed(3)}, content pane at ${content.backTop.toFixed(3)}`,
    ).toBeLessThan(0.02);
  });

  test('all three header rows are the same height', async ({ page }) => {
    // The drawer row used to stand 0.1rem taller than the other two, because it
    // was the only one carrying the Lucidos mark IN FLOW (the thread pane's sits
    // in the absolutely-positioned cluster and adds no height) and the row was
    // sized by a min-height. The header changed height on every swipe, taking
    // everything anchored to --mobile-header-height with it. Its mark is in a
    // cluster now too, so the fixed height is what guards the next in-flow
    // control rather than that one.
    await navigateToApp(page);

    const rowHeight = async (headerSel: string): Promise<number> => page.evaluate((sel) => {
      const row = document.querySelector(`${sel} .mobile-header-row`) as HTMLElement | null;
      return row ? row.getBoundingClientRect().height : 0;
    }, headerSel);

    await ensureMobileView(page, 'thread');
    const thread = await rowHeight('.mobile-thread-header');
    await ensureMobileView(page, 'content');
    const content = await rowHeight('.mobile-content-header');
    await openThreadDrawer(page);
    const threads = await rowHeight('.mobile-threads-header');

    expect(thread, 'thread row has no height').toBeGreaterThan(0);
    const heights = { thread, content, threads };
    expect(Math.abs(thread - content), `rows differ: ${JSON.stringify(heights)}`).toBeLessThan(0.5);
    expect(Math.abs(thread - threads), `rows differ: ${JSON.stringify(heights)}`).toBeLessThan(0.5);

    // And the fixed height must still contain the tallest control, or the mark
    // is clipped at the header's bottom edge rather than merely equal.
    const markHeight = await page.evaluate(() => {
      const el = document.querySelector('.mobile-threads-header [data-role="brand-menu-toggle"]') as HTMLElement | null;
      return el ? el.getBoundingClientRect().height : 0;
    });
    expect(markHeight, 'the drawer mark was not measurable').toBeGreaterThan(0);
    expect(threads, `row ${threads} is shorter than the mark ${markHeight} it holds`)
      .toBeGreaterThanOrEqual(markHeight - 0.5);
  });

  test('connected is the mark at full light, and nothing else', async ({ page }) => {
    // The mark says its connection in ONE dimension, strength: connected is the
    // brand at the header's full --header-fg, and the two states worth saying
    // something about recede from it with opacity (and say it in words through
    // the aria-label asserted above). So the assertion has two halves: the
    // colour IS the light one, and nothing else is painted on top of it.
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');

    const mark = '.mobile-thread-header [data-role="brand-menu-toggle"]';
    await expect
      .poll(() => page.evaluate((sel) => document.querySelector(sel)?.getAttribute('data-conn') ?? null, mark),
        { timeout: 15_000, message: 'the mark never reported a connected engine' })
      .toBe('connected');

    const readPaint = () => page.evaluate((sel) => {
      const tile = document.querySelector(`${sel} .brand-mark-glyph`) as HTMLElement | null;
      if (!tile) return null;
      const own = getComputedStyle(tile);
      const ring = getComputedStyle(tile, '::after');
      // Resolve both header tokens through a real element in the same subtree,
      // so the comparison is against computed colours rather than against the
      // token's source text (`#ffffff` never equals `rgb(255, 255, 255)`).
      const swatch = (token: string): string => {
        const probe = document.createElement('span');
        probe.style.color = `var(${token})`;
        tile.appendChild(probe);
        const resolved = getComputedStyle(probe).color;
        probe.remove();
        return resolved;
      };
      return {
        color: own.color,
        light: swatch('--header-fg'),
        muted: swatch('--header-fg-muted'),
        opacity: own.opacity,
        animationName: own.animationName,
        // A ring pseudo-element with no `content` is not generated at all.
        ringContent: ring.content,
      };
    }, mark);

    const first = await readPaint();
    expect(first, 'the mark tile was not found').not.toBeNull();

    // The glyph CROSSFADES between states (transition: color), so the frame
    // right after `data-conn` flips is mid-fade: WebKit read the alpha at 0.957
    // there and the muted base it is leaving is 0.72. Poll until it settles
    // rather than snapshotting one frame of the fade. The target is read once
    // up front, since polling the comparison as a boolean would report a
    // timeout as `false` and say nothing about the colour it settled on.
    await expect
      .poll(async () => (await readPaint())?.color ?? null,
        { timeout: 5_000, message: 'the mark never settled on the header foreground at full light' })
      .toBe(first!.light);

    const paint = await readPaint();
    expect(paint, 'the mark tile was not found').not.toBeNull();
    expect(paint!.color, 'connected must not sit at the muted base the receded states use')
      .not.toBe(paint!.muted);
    expect(paint!.opacity, 'connected must not be dimmed').toBe('1');
    expect(paint!.animationName, 'connected must not animate').toBe('none');
    expect(paint!.ringContent, 'connected must carry no ring').toBe('none');
  });
});
