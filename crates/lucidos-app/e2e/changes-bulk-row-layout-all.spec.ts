import { test, expect, type Locator, type Page } from './fixtures';
import { assertHealthy, gotoWithRetry, hideDashboardRail, mockChangesState } from './helpers';

test.use({ serviceWorkers: 'block' });

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
    needs_hardening: false,
    apply_ready: true,
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
  pending: [
    change('ready'),
    change('ready-2'),
    change('settling', { thread_unsettled: true, apply_ready: false, thread_settling: true }),
  ],
  applied: [],
  total_pending: 3,
  restart_required: false,
  restart_groups: [],
  client_update_available: false,
  has_more_applied: false,
  apply_all_in_progress: false,
  apply_all_batch: null,
  standing_apply_thread_ids: [] as string[],
};

const ARMED_STATE = { ...CHANGES_STATE, standing_apply_thread_ids: ['thread-settling'] };

async function openChanges(
  page: Page,
  state: typeof CHANGES_STATE,
  opts: { changesDelayMs?: number } = {},
): Promise<Locator[]> {
  const { reads } = await mockChangesState(page, state, opts);
  await page.addInitScript(() => {
    localStorage.setItem('lucidos-active-menu-item', 'changes');
    localStorage.setItem('lucidos-mobile-view', 'content');
  });
  await hideDashboardRail(page);
  await gotoWithRetry(page, '/');
  await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
  await expect.poll(reads).toBeGreaterThan(0);
  const rows = page.locator('.changes-bulk-actions:visible');
  await expect(rows).toHaveCount(2, { timeout: 15_000 });
  return [rows.nth(0), rows.nth(1)];
}

async function box(locator: Locator) {
  const b = await locator.boundingBox();
  if (!b) throw new Error('element has no box');
  return { left: b.x, right: b.x + b.width, top: b.y, bottom: b.y + b.height, width: b.width };
}

async function expectButtonLine(row: Locator): Promise<number> {
  const rowBox = await box(row);
  const pad = await row.evaluate((el) => {
    const s = getComputedStyle(el);
    return { left: parseFloat(s.paddingLeft), right: parseFloat(s.paddingRight) };
  });
  const buttons = await row.locator('.changes-bulk-buttons > button').all();
  const last = await box(buttons[buttons.length - 1]);
  expect(last.right).toBeCloseTo(rowBox.right - pad.right, 0);
  const middle = (last.top + last.bottom) / 2;
  for (const button of buttons) {
    const b = await box(button);
    expect((b.top + b.bottom) / 2).toBeCloseTo(middle, 0);
    const clipped = await button.evaluate((el) => {
      const range = document.createRange();
      range.selectNodeContents(el);
      const s = getComputedStyle(el);
      const content = el.clientWidth - parseFloat(s.paddingLeft) - parseFloat(s.paddingRight);
      return range.getBoundingClientRect().width > content + 0.5;
    });
    expect(clipped, `"${await button.textContent()}" is clipped`).toBe(false);
  }
  return last.right;
}

async function expectLayout(page: Page, [ready, notFinished]: Locator[]): Promise<void> {
  await expect(ready.locator('.changes-bulk-buttons > button')).toHaveText(['Discard All', 'Apply All']);
  await expect(notFinished.locator('.changes-bulk-buttons > button')).toHaveCount(1);
  const readyRight = await expectButtonLine(ready);
  const notFinishedRight = await expectButtonLine(notFinished);

  const readyBox = await box(ready);
  const padLeft = await ready.evaluate((el) => parseFloat(getComputedStyle(el).paddingLeft));
  const discard = await box(ready.getByRole('button', { name: 'Discard All', exact: true }));
  expect(discard.left).toBeCloseTo(readyBox.left + padLeft, 0);

  const rowAction = await box(page.locator('.list-row:visible .list-row-actions').first());
  expect(readyRight).toBeCloseTo(rowAction.right, 0);
  expect(notFinishedRight).toBeCloseTo(rowAction.right, 0);
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

  test('a changes read that lands after the bootstrap keeps the mocked state', async ({ page, isMobile }) => {
    test.skip(isMobile, 'desktop layout');
    await page.setViewportSize({ width: 1000, height: 800 });
    await openChanges(page, CHANGES_STATE, { changesDelayMs: 1500 });
  });

  for (const w of WIDTHS) {
    test(`both sections' bulk rows fit a ${w.name} pane`, async ({ page, isMobile }, testInfo) => {
      test.skip(isMobile !== w.mobile, `${w.name} belongs to the ${w.mobile ? 'mobile' : 'desktop'} layout`);
      await page.setViewportSize(w.viewport);
      const rows = await openChanges(page, CHANGES_STATE);
      await expectLayout(page, rows);
      await screenshot(page, testInfo.outputPath(`changes-bulk-row-${w.name}.png`));
    });

    test(`the armed toggle fits a ${w.name} pane`, async ({ page, isMobile }, testInfo) => {
      test.skip(isMobile !== w.mobile, `${w.name} belongs to the ${w.mobile ? 'mobile' : 'desktop'} layout`);
      await page.setViewportSize(w.viewport);
      const rows = await openChanges(page, ARMED_STATE);
      await expectLayout(page, rows);
      await screenshot(page, testInfo.outputPath(`changes-bulk-row-armed-${w.name}.png`));
    });
  }
});
