/**
 * The Changes panel bulk row is one line of buttons at every pane width.
 *
 * Discard All at the left edge, "Apply all on settle", then Apply All at the
 * right edge. The unit test (`components/changes/__tests__/bulk-row-layout`)
 * pins the markup and the CSS. Only a real layout shows the two ends, the
 * no-wrap, and the right edge meeting the change rows below.
 *
 * The one `/changes` read is mocked, so every member of the row draws: one
 * change ready now, one whose thread is still settling.
 */
import { test, expect, type Locator, type Page } from './fixtures';
import { assertHealthy, gotoWithRetry } from './helpers';

function change(id: string, over: Record<string, unknown> = {}) {
  return {
    id,
    request_id: '00000000-0000-0000-0000-000000000000',
    thread_id: `thread-${id}`,
    thread_title: `Thread ${id}`,
    branch_name: `b-${id}`,
    repo_root: '/r',
    description: `fix: change ${id}`,
    file_count: 1,
    files: ['a.rs'],
    requires_restart: false,
    hardened: true,
    status: 'pending',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: null,
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    summary: null,
    incomplete: false,
    ...over,
  };
}

const CHANGES_STATE = {
  pending: [change('ready'), change('settling', { thread_unsettled: true, thread_settling: true })],
  applied: [],
  total_pending: 2,
  restart_required: false,
  restart_groups: [],
  client_update_available: false,
  has_more_applied: false,
  apply_all_in_progress: false,
  apply_all_batch: null,
  standing_apply_thread_ids: [] as string[],
  settling_thread_count: 1,
};

/** The same three buttons with the sweep armed, so the toggle wears its
 *  longer cancel face. */
const ARMED_STATE = { ...CHANGES_STATE, standing_apply_thread_ids: ['thread-settling'] };

async function openChanges(page: Page, state: typeof CHANGES_STATE): Promise<Locator> {
  await page.route('**/api/v1/changes*', (route) =>
    route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(state) }));
  await page.addInitScript(() => {
    localStorage.setItem('lucidos-active-menu-item', 'changes');
    localStorage.setItem('lucidos-mobile-view', 'content');
  });
  await gotoWithRetry(page, '/');
  const row = page.locator('.changes-bulk-actions:visible');
  await expect(row).toBeVisible({ timeout: 15_000 });
  // The boot splash is an opaque overlay that clears once layout settles. A
  // box measured or shot under it can still move, or shows only the splash.
  await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
  return row;
}

async function box(locator: Locator) {
  const b = await locator.boundingBox();
  if (!b) throw new Error('element has no box');
  return { left: b.x, right: b.x + b.width, top: b.y, bottom: b.y + b.height, width: b.width };
}

/** The button line's claims: Discard All and Apply All on the two ends, and
 *  every button on one line inside the gutter. */
async function expectButtonLine(row: Locator) {
  const rowBox = await box(row);
  const pad = await row.evaluate((el) => {
    const s = getComputedStyle(el);
    return { left: parseFloat(s.paddingLeft), right: parseFloat(s.paddingRight) };
  });
  const discard = await box(row.getByRole('button', { name: 'Discard All', exact: true }));
  const apply = await box(row.getByRole('button', { name: 'Apply All', exact: true }));
  expect(discard.left).toBeCloseTo(rowBox.left + pad.left, 0);
  expect(apply.right).toBeCloseTo(rowBox.right - pad.right, 0);
  // One line, centred, and no label clipped by a squeezed button.
  const discardMiddle = (discard.top + discard.bottom) / 2;
  for (const button of await row.locator('.changes-bulk-buttons > button').all()) {
    const b = await box(button);
    expect((b.top + b.bottom) / 2).toBeCloseTo(discardMiddle, 0);
    // The text's own box against the content box: scrollWidth would count the
    // mobile touch target, which extends past the button on purpose.
    const clipped = await button.evaluate((el) => {
      const range = document.createRange();
      range.selectNodeContents(el);
      const s = getComputedStyle(el);
      const content = el.clientWidth - parseFloat(s.paddingLeft) - parseFloat(s.paddingRight);
      return range.getBoundingClientRect().width > content + 0.5;
    });
    expect(clipped, `"${await button.textContent()}" is clipped`).toBe(false);
  }
  return { rowBox, pad, apply };
}

/** Every geometry claim the layout makes, measured at the current width. */
async function expectLayout(page: Page, row: Locator): Promise<void> {
  await expect(row.locator('.changes-bulk-buttons > button')).toHaveCount(3);
  const { apply } = await expectButtonLine(row);

  // The right edge meets the change rows' own action buttons below.
  const rowAction = await box(page.locator('.list-row:visible .list-row-actions').first());
  expect(apply.right).toBeCloseTo(rowAction.right, 0);
}

const WIDTHS = [
  { name: 'wide', viewport: { width: 1600, height: 900 }, mobile: false },
  { name: 'medium', viewport: { width: 1000, height: 800 }, mobile: false },
  { name: 'phone', viewport: { width: 375, height: 812 }, mobile: true },
  { name: 'narrow-phone', viewport: { width: 320, height: 700 }, mobile: true },
];

async function screenshot(page: Page, path: string): Promise<void> {
  await page.locator('.panel-content:visible').first().screenshot({ path });
}

test.describe('Changes panel bulk row', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  for (const w of WIDTHS) {
    test(`three buttons fit a ${w.name} pane`, async ({ page, isMobile }, testInfo) => {
      test.skip(isMobile !== w.mobile, `${w.name} belongs to the ${w.mobile ? 'mobile' : 'desktop'} layout`);
      await page.setViewportSize(w.viewport);
      const row = await openChanges(page, CHANGES_STATE);
      await expectLayout(page, row);
      await screenshot(page, testInfo.outputPath(`changes-bulk-row-${w.name}.png`));
    });

    test(`the armed three-button line fits a ${w.name} pane`, async ({ page, isMobile }, testInfo) => {
      test.skip(isMobile !== w.mobile, `${w.name} belongs to the ${w.mobile ? 'mobile' : 'desktop'} layout`);
      await page.setViewportSize(w.viewport);
      const row = await openChanges(page, ARMED_STATE);
      await expectLayout(page, row);
      await screenshot(page, testInfo.outputPath(`changes-bulk-row-armed-${w.name}.png`));
    });
  }
});
