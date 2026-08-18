/**
 * The desktop content header's title and chevrons hold ONE position, whatever
 * the trailing action cluster contains.
 *
 * The report, 2026-08-13: "we should keep the same position for title/chevrons
 * in content pane header whether the overflow menu icon is present or not."
 * They were the `flex: 1` middle of a 3-zone row, so their position was the
 * midpoint between the hamburger and a cluster that is 1 to 3 icon boxes wide
 * depending on the content view. The ⋯ trigger coming and going was the
 * sharpest case; navigating from a plain view (the bell alone) to Files (an
 * icon and the bell) moved them just as much.
 *
 * The fix is the arrangement every other header row already had: the cluster is
 * a fixed span centred on the row, and the actions are in flow at its trailing
 * edge (`.pane-header-content-title` in styles/panels/shell.css). This spec is
 * the rendered proof, over three views whose clusters differ:
 *
 * | View            | Trailing cluster        | ⋯   |
 * |-----------------|-------------------------|-----|
 * | Settings        | the bell alone          | no  |
 * | Apps            | search + the bell       | no  |
 * | An app's UI     | ⋯ + the bell            | yes |
 *
 * The mirror case has its own test: two app views whose CLUSTERS are identical
 * and whose TITLES are not, one of them long enough to ellipsize. That is the
 * variable the report named second ("the chevrons still move around for
 * different titles"), and the three views above cannot isolate it, since each
 * of them changes the title and the cluster together.
 *
 * Desktop-only: the mobile row has its own centred cluster and its own spec
 * (mobile-content-title-overlap.spec.ts). The arithmetic behind the reserve the
 * box clears is pinned by source scan in
 * src/styles/__tests__/content-header-title-centring.test.ts, which is where a
 * regression at a ui-scale this viewport never renders would show up.
 */
import { test, expect, type Page } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { assertHealthy, gotoWithRetry, clickVisibleElement } from './helpers';

const APP_ID = 'e2e-desktop-title-position';
const APP_NAME = 'Half Marathon';
// The same view type under a title long enough to outgrow --desktop-nav-span
// (20rem), so it is the ellipsis case rather than merely a wider one.
const APP_ID_LONG = 'e2e-desktop-title-position-long';
const APP_NAME_LONG = 'Half Marathon Training Plan and Weekly Readiness Review';
let fixture: { cleanup: () => void };
let fixtureLong: { cleanup: () => void };

test.use({ viewport: { width: 1280, height: 800 } });

interface HeaderMetrics {
  backX: number;        // the back chevron's left edge
  forwardX: number;     // the forward chevron's right edge
  boxCentre: number;
  boxRight: number;
  boxWidth: number;
  rowCentre: number;
  rowWidth: number;     // the box's containing block, which its 100% resolves against
  navRight: number;     // the hamburger's inner edge
  actionsLeft: number;  // the trailing cluster's inner edge
  minSpan: number;      // --desktop-nav-min-span, resolved to px
  sideReserve: number;  // --content-side-reserve, resolved to px
  titleText: string;    // the box's rendered text, ellipsis excluded (it is CSS)
  hasOverflow: boolean;
  trailingIcons: number;
}

/** Measure the desktop content row's centred box against its two clusters.
 *
 *  Scoped to `.desktop-header`: MobileAppHeader renders FIRST and carries
 *  copies of several of these classes, so an unscoped querySelector can return
 *  the hidden mobile one (the 0x0-rect trap in .claude/rules/frontend.md). */
async function measure(page: Page): Promise<HeaderMetrics | null> {
  return page.evaluate(() => {
    const row = document.querySelector('.desktop-header .content-header-elements') as HTMLElement | null;
    if (!row || row.getBoundingClientRect().width === 0) return null;
    const box = row.querySelector('.pane-header-content-title') as HTMLElement | null;
    const back = row.querySelector('.content-back-btn') as HTMLElement | null;
    const forward = row.querySelector('.content-forward-btn') as HTMLElement | null;
    const nav = row.querySelector('.hamburger-panel') as HTMLElement | null;
    const actions = row.querySelector('.content-header-actions') as HTMLElement | null;
    if (!box || !back || !forward || !nav || !actions) return null;
    const b = box.getBoundingClientRect();
    const r = row.getBoundingClientRect();
    const a = actions.getBoundingClientRect();
    if (b.width === 0 || a.width === 0) return null;
    // The clamp's floor, resolved by the browser rather than restated as a
    // number here: it is a rem token, so a literal would be right at exactly
    // one ui-scale. Probed on a detached-from-flow div appended to <body>, the
    // token being declared on :root, so nothing in the header is disturbed.
    const resolve = (token: string) => {
      const probe = document.createElement('div');
      probe.style.cssText = `position:absolute;visibility:hidden;width:var(${token})`;
      document.body.appendChild(probe);
      const px = probe.getBoundingClientRect().width;
      probe.remove();
      return px;
    };
    const minSpan = resolve('--desktop-nav-min-span');
    const sideReserve = resolve('--content-side-reserve');
    return {
      backX: back.getBoundingClientRect().left,
      forwardX: forward.getBoundingClientRect().right,
      boxCentre: b.left + b.width / 2,
      boxRight: b.right,
      boxWidth: b.width,
      rowCentre: r.left + r.width / 2,
      rowWidth: r.width,
      navRight: nav.getBoundingClientRect().right,
      actionsLeft: a.left,
      minSpan,
      sideReserve,
      titleText: (box.querySelector('.pane-header-title-text')?.textContent ?? '').trim(),
      hasOverflow: actions.querySelectorAll('.content-header-more').length > 0,
      trailingIcons: actions.querySelectorAll('.icon-btn.header-icon').length,
    };
  });
}

/** Settled = the row is laid out AND the collapse hook has measured it (it runs
 *  in a layout effect, so a read before it lands sees the pre-collapse row). */
async function settled(page: Page): Promise<HeaderMetrics> {
  let last: HeaderMetrics | null = null;
  await expect.poll(async () => {
    const now = await measure(page);
    const stable = now !== null && last !== null
      && Math.abs(now.actionsLeft - last.actionsLeft) < 0.5
      && Math.abs(now.boxCentre - last.boxCentre) < 0.5;
    last = now;
    return stable;
  }, { timeout: 10_000 }).toBe(true);
  return last!;
}

/** True of every view: the box sits on the row's axis and clears both clusters. */
function expectCentredAndClear(m: HeaderMetrics, view: string): void {
  expect(
    Math.abs(m.boxCentre - m.rowCentre),
    `${view}: box centre ${m.boxCentre.toFixed(1)} is off the row centre ${m.rowCentre.toFixed(1)}`,
  ).toBeLessThan(1.5);
  expect(
    m.boxRight,
    `${view}: the title box reaches the actions cluster at ${m.actionsLeft.toFixed(1)}`,
  ).toBeLessThanOrEqual(m.actionsLeft + 0.6);
  expect(
    m.backX,
    `${view}: the back chevron reaches the hamburger at ${m.navRight.toFixed(1)}`,
  ).toBeGreaterThanOrEqual(m.navRight - 0.6);
}

test.describe('the desktop content title holds its position across views', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: APP_NAME, description: 'e2e fixture' },
      html: `<!DOCTYPE html><html><head><meta charset="UTF-8"><title>${APP_NAME}</title></head><body><div id="ready">ready</div></body></html>`,
      js: '',
    });
    fixtureLong = createIframeAppFixture(APP_ID_LONG, {
      manifest: { id: APP_ID_LONG, name: APP_NAME_LONG, description: 'e2e fixture' },
      html: `<!DOCTYPE html><html><head><meta charset="UTF-8"><title>${APP_NAME_LONG}</title></head><body><div id="ready">ready</div></body></html>`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
    fixtureLong.cleanup();
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the chevrons land on the same x for a short and a long title', async ({ page }) => {
    // The report's own wording, 2026-08-13: "the chevrons still move around for
    // different titles, should stay in one place (as long as header width is
    // the same)". The case below it holds the TITLE roughly constant and varies
    // the action cluster; this one is its mirror, and it is the sharper
    // isolation of the two: both loads are the same view TYPE (an app's UI, so
    // the same three context actions folded to the same ⋯ + bell) at the same
    // split, so the title is the only thing that differs.
    //
    // What makes the chevrons immune to it is that the box is a FIXED SPAN with
    // `space-between`: the title is its one shrinking member, so a title too
    // long for the span ellipsizes inside it rather than pushing an end out.
    // A regression here looks like a `width` on the box that reads its content
    // (`fit-content`, `max-content`, or dropping the clamp for a `max-width`).
    const titled: Partial<Record<'short' | 'long', HeaderMetrics>> = {};
    // One load first, purely to put the page on the app's origin: the restore
    // key is written with `evaluate` rather than `addInitScript` so the second
    // pass can overwrite it, and localStorage is per-origin.
    await gotoWithRetry(page, '/');
    for (const [label, id] of [['short', APP_ID], ['long', APP_ID_LONG]] as const) {
      await page.evaluate((appId) => localStorage.setItem('app-window-open', appId), id);
      await gotoWithRetry(page, '/');
      await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
      titled[label] = await settled(page);
    }

    // The titles really did differ, or the comparison below proves nothing.
    expect(
      titled.long!.titleText.length,
      `the long fixture rendered "${titled.long!.titleText}", no longer than the short one`,
    ).toBeGreaterThan(titled.short!.titleText.length + 20);
    expect(titled.long!.hasOverflow && titled.short!.hasOverflow, 'both views carry the ⋯').toBe(true);
    expect(titled.long!.actionsLeft, 'the clusters differ, so this is not a title-only comparison')
      .toBeCloseTo(titled.short!.actionsLeft, 0);

    for (const edge of ['backX', 'forwardX', 'boxWidth'] as const) {
      expect(
        titled.long![edge],
        `${edge} moved to ${titled.long![edge].toFixed(1)} from ${titled.short![edge].toFixed(1)} `
          + `on a longer title alone`,
      ).toBeCloseTo(titled.short![edge], 0);
    }
    expectCentredAndClear(titled.long!, 'long title');
  });

  test('the chevrons land on the same x whether the ⋯ is present or not', async ({ page }) => {
    // Restore-on-load opens the app in the content pane (the same hook
    // sdk-iframe-mount uses): panelOverlay = {type:'app-ui'}, whose three
    // context actions fold whole, so this view carries the ⋯.
    await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
    await gotoWithRetry(page, '/');
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });
    const withOverflow = await settled(page);
    expect(withOverflow.hasOverflow, 'the app view should carry the ⋯ trigger').toBe(true);
    expect(withOverflow.trailingIcons, 'the app view: ⋯ + the bell').toBe(2);
    expectCentredAndClear(withOverflow, 'app UI');

    // Apps: one context action (search) riding the row beside the bell. Same
    // icon count as the app view, no ⋯, and a different total width once the
    // gap between the two is counted.
    await clickVisibleElement(page, '.hamburger-panel');
    await expect.poll(() => clickVisibleElement(page, '.drawer-item', 'Apps'), { timeout: 10_000 }).toBe(true);
    const oneAction = await settled(page);
    expect(oneAction.hasOverflow, 'one action does not fold: ⋯ would replace it 1:1').toBe(false);
    expectCentredAndClear(oneAction, 'apps');

    // Settings: no context actions at all, so the bell stands alone and the
    // cluster is a whole icon box narrower than either view above. This is the
    // pair the report is about, at its widest.
    await clickVisibleElement(page, '.hamburger-panel');
    await expect.poll(() => clickVisibleElement(page, '.drawer-item', 'Settings'), { timeout: 10_000 }).toBe(true);
    const noActions = await settled(page);
    expect(noActions.hasOverflow, 'nothing to collapse, so no ⋯').toBe(false);
    expect(noActions.trailingIcons, 'settings: the bell alone').toBe(1);
    expectCentredAndClear(noActions, 'settings');

    // The ask. The clusters really did differ, and the chevrons did not move.
    expect(
      noActions.actionsLeft,
      'the three views produced the same trailing cluster, so this proves nothing',
    ).toBeGreaterThan(withOverflow.actionsLeft + 1);
    for (const [view, m] of [['apps', oneAction], ['settings', noActions]] as const) {
      expect(
        m.backX,
        `${view}: the back chevron moved to ${m.backX.toFixed(1)} from ${withOverflow.backX.toFixed(1)}`,
      ).toBeCloseTo(withOverflow.backX, 0);
      expect(
        m.forwardX,
        `${view}: the forward chevron moved to ${m.forwardX.toFixed(1)} from ${withOverflow.forwardX.toFixed(1)}`,
      ).toBeCloseTo(withOverflow.forwardX, 0);
    }
  });

  test('the box still clears the cluster with the Canvas pane near its floor', async ({ page }) => {
    // The narrowest this pane legally gets, where the box has given up the most
    // width to the two side reserves. The collapse measurement is the other
    // half here, folding the actions into ⋯: the reason the centring survives
    // at all.
    //
    // The pane width is SEEDED, not dragged. `splitRatio` is read straight out
    // of localStorage with no load-time clamp (store/store.ts), so a ratio IS a
    // pane width, where a divider drag has to find a live divider and land a
    // real pointer on it. Every sibling desktop spec seeds it for that reason
    // (split-resize-desktop, repo-files, header-drawer-toggle-travel-desktop);
    // this test hand-rolled the drag instead and it silently moved nothing,
    // leaving the pane at its 765px default and the assertion below unreached.
    //
    // 0.7 of this 1280 viewport leaves the Canvas pane ~381px: above its
    // MIN_CONTENT_PANE_REM floor (22.5rem = 360px at these projects' 16px
    // root), so it is a width a drag could also stop at, and the narrowest
    // regime the clamp has to hold this box clear in.
    await page.addInitScript((id) => {
      localStorage.setItem('lucidos-split-ratio', '0.7');
      localStorage.setItem('lucidos-thread-drawer-open', 'false');
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');
    await expect(page.locator('iframe[data-role="app-ui-frame"]:visible')).toBeVisible({ timeout: 15_000 });

    const narrow = await settled(page);
    const paneWidth = await page.evaluate(
      () => document.querySelector('.pane-content')!.getBoundingClientRect().width,
    );
    expect(paneWidth, 'the seeded ratio did not narrow the Canvas pane').toBeLessThan(400);
    expect(paneWidth, 'the seeded ratio went under the Canvas pane floor').toBeGreaterThan(360);
    // On the RESERVE arm, all the way down to the pane's floor, which is the
    // shape that makes the clearance structural. The min-span arm is what the
    // box would fall back to, and it cannot be reached above this floor:
    // `2 * --content-side-reserve + --desktop-nav-min-span` is 22.1rem, inside
    // MIN_CONTENT_PANE_REM. So assert the arm rather than the number, and
    // assert the floor stays under it (store/__tests__/conversation-pane-floor
    // .test.ts owns that inequality at every ui-scale).
    expect(
      narrow.boxWidth,
      `the box is ${narrow.boxWidth.toFixed(1)} wide, not the reserve arm's `
        + `${(narrow.rowWidth - 2 * narrow.sideReserve).toFixed(1)}`,
    ).toBeCloseTo(narrow.rowWidth - 2 * narrow.sideReserve, 0);
    expect(
      narrow.boxWidth,
      `the box fell to its ${narrow.minSpan} min-span floor above the pane's own floor`,
    ).toBeGreaterThan(narrow.minSpan);
    // And the fold is what keeps them apart there: the app view's three context
    // actions are in the ⋯ menu, leaving ⋯ + the bell.
    expect(narrow.hasOverflow, 'the app view folds its three actions whole').toBe(true);
    expect(narrow.trailingIcons, 'the narrow app view: ⋯ + the bell').toBe(2);
    expectCentredAndClear(narrow, 'canvas pane near its floor');
  });
});
