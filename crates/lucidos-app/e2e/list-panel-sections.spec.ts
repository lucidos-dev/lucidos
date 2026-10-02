/**
 * The list panels share one section header and one row hairline
 * (docs/plans/2026-09-30-list-panel-section-headers.md).
 *
 * Triggers, Changes and Thread queue each draw the drawer's header: a bold
 * uppercase label at the body step, with a count. Every row carries a hairline
 * inset to the content column, a collapsed header carries one too, and so does
 * the Changes button row. The unit scan pins the markup and the rules. Only a
 * real layout resolves the cascade and the insets.
 *
 * Each panel's read is mocked, so the sections are known. The service worker
 * is blocked, since a request it makes never reaches `page.route`.
 */
import { test, expect, type Page } from './fixtures';
import {
  apiRequest, assertHealthy, gotoWithRetry, navigateToApp, openTriggersPanel, waitForEventStream,
} from './helpers';

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
  pending: [change('ready'), change('second')],
  set_aside: [change('parked', { status: 'set_aside' })],
  applied: [
    change('done', { status: 'applied', resolved_at: '2026-01-01T01:00:00Z', pre_merge_sha: 'a1' }),
  ],
  total_pending: 2,
  restart_required: false,
  restart_groups: [],
  client_update_available: false,
  has_more_applied: false,
  apply_all_in_progress: false,
  apply_all_batch: null,
  standing_apply_thread_ids: [] as string[],
};

function trigger(id: string, name: string, groupId?: string) {
  return {
    id,
    name,
    cron_expressions: ['0 9 * * *'],
    timezone: 'UTC',
    paused: false,
    run: { type: 'intent', intent: 'send the digest' },
    ...(groupId ? { group_id: groupId } : {}),
  };
}

const TRIGGERS = [trigger('t1', 'Morning digest', 'daily'), trigger('t2', 'Inbox triage', 'daily'), trigger('t3', 'Loose one')];
const GROUPS = [{ id: 'daily', name: 'Daily', order: 0, created: '2026-01-01T00:00:00Z', member_count: 2 }];

const QUEUE = {
  entries: [{
    id: 'q1', kind: 'cron', summary: 'Morning digest', status: 'admitted',
    queued_at: '2026-01-01T00:00:00Z', admitted_at: '2026-01-01T00:00:01Z',
  }],
  policy: {
    max_concurrent_total: 4, max_concurrent_event_trigger: 2, max_concurrent_cron: 2,
    max_concurrent_sub_thread: 2, max_concurrent_coding_agent: 2, max_concurrent_per_trigger: 1,
    max_queued_per_trigger: 10, reserved_background: 1, max_event_trigger_depth: 5,
    overflow: 'drop-oldest',
  },
};

async function settle(page: Page): Promise<void> {
  await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
}

/** Each visible section header in `root`, measured against the body step. */
async function headerLooks(page: Page, root: string) {
  return page.evaluate((rootSel) => {
    const scope = Array.from(document.querySelectorAll(rootSel))
      .find(el => (el as HTMLElement).offsetParent !== null) as HTMLElement | undefined;
    if (!scope) return { headers: [], bodyStep: '', primary: '' };
    const probe = document.createElement('div');
    probe.style.fontSize = 'var(--font-size-md)';
    probe.style.color = 'var(--text-primary)';
    scope.appendChild(probe);
    const bodyStep = getComputedStyle(probe).fontSize;
    const primary = getComputedStyle(probe).color;
    probe.remove();
    const headers = Array.from(scope.querySelectorAll('.list-section-title-collapsible')).map(h => {
      const cs = getComputedStyle(h);
      return {
        text: h.querySelector('.section-label')?.textContent ?? '',
        weight: cs.fontWeight,
        transform: cs.textTransform,
        size: cs.fontSize,
        color: cs.color,
        background: cs.backgroundColor,
      };
    });
    return { headers, bodyStep, primary };
  }, root);
}

/** Every hairline under `selector` inside `root`: where the line runs against
 *  the element's content box. A panel header hands its left padding to its
 *  toggle, so its content starts where the toggle's does. */
async function hairlines(page: Page, root: string, selector: string) {
  return page.evaluate(({ rootSel, sel }) => {
    const scope = Array.from(document.querySelectorAll(rootSel))
      .find(el => (el as HTMLElement).offsetParent !== null);
    if (!scope) return [];
    return Array.from(scope.querySelectorAll(sel)).map(el => {
      const rect = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      const line = getComputedStyle(el, '::after');
      const toggle = el.querySelector(':scope > .list-section-toggle');
      const start = toggle ?? el;
      const startLeft = start.getBoundingClientRect().left + parseFloat(getComputedStyle(start).paddingLeft);
      return {
        drawn: line.content !== 'none' && line.borderBottomWidth === '1px',
        lineLeft: rect.left + parseFloat(line.left),
        lineRight: rect.right - parseFloat(line.right),
        contentLeft: startLeft,
        contentRight: rect.right - parseFloat(cs.paddingRight),
      };
    });
  }, { rootSel: root, sel: selector });
}

function expectInset(lines: Awaited<ReturnType<typeof hairlines>>, what: string) {
  expect(lines.length, `no ${what} found`).toBeGreaterThan(0);
  for (const l of lines) {
    expect(l.drawn, `${what} draws no hairline`).toBe(true);
    expect(Math.abs(l.lineLeft - l.contentLeft), `${what} line starts off the content column`).toBeLessThanOrEqual(1);
    expect(Math.abs(l.lineRight - l.contentRight), `${what} line ends off the content column`).toBeLessThanOrEqual(1);
  }
}

function expectDrawerLook(looks: Awaited<ReturnType<typeof headerLooks>>, labels: string[]) {
  expect(looks.headers.map(h => h.text)).toEqual(labels);
  for (const h of looks.headers) {
    expect(h.weight, h.text).toBe('700');
    expect(h.transform, h.text).toBe('uppercase');
    expect(h.size, h.text).toBe(looks.bodyStep);
    expect(h.color, h.text).toBe(looks.primary);
    expect(h.background, `${h.text} paints no band`).toBe('rgba(0, 0, 0, 0)');
  }
}

test.describe('list panel sections', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Changes: drawer headers, hairlines under every row and the button row, a collapse that sticks', async ({ page }) => {
    await page.route('**/api/v1/changes*', (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(CHANGES_STATE) }));
    await page.addInitScript(() => {
      localStorage.setItem('lucidos-active-menu-item', 'changes');
      localStorage.setItem('lucidos-mobile-view', 'content');
    });
    await gotoWithRetry(page, '/');
    await expect(page.locator('.changes-bulk-actions:visible')).toBeVisible({ timeout: 15_000 });
    await settle(page);

    const panel = '.panel-content.list-rows-divided';
    expectDrawerLook(await headerLooks(page, panel), ['Ready', 'Set aside', 'Recently applied']);
    expectInset(await hairlines(page, panel, '.list-row'), 'a change row');
    expectInset(await hairlines(page, panel, '.changes-bulk-actions'), 'the button row');
    await page.locator(panel).first().screenshot({ path: test.info().outputPath('changes.png') });

    const setAside = page.locator('.list-section-title-collapsible:visible', { hasText: 'Set aside' });
    await setAside.locator('.list-section-toggle').click();
    await expect(page.locator('.change-row:visible', { hasText: 'Thread parked' })).toHaveCount(0);
    await expect(setAside).toHaveClass(/\bcollapsed\b/);
    expectInset(
      await hairlines(page, panel, '.list-section-title-collapsible.collapsed'),
      'a collapsed header',
    );

    await page.reload();
    await expect(page.locator('.changes-bulk-actions:visible')).toBeVisible({ timeout: 15_000 });
    await expect(setAside).toHaveClass(/\bcollapsed\b/);
    await expect(page.locator('.change-row:visible', { hasText: 'Thread parked' })).toHaveCount(0);
  });

  test('Triggers: drawer headers with the count, rename kept, hairlines under every trigger', async ({ page }) => {
    await page.route(/\/api\/v1\/triggers(\?.*)?$/, (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ triggers: TRIGGERS }) }));
    await page.route(/\/api\/v1\/trigger-groups(\?.*)?$/, (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ groups: GROUPS }) }));
    await navigateToApp(page);
    await openTriggersPanel(page);
    await settle(page);

    const panel = '.list-rows.list-rows-divided';
    expectDrawerLook(await headerLooks(page, panel), ['Daily', 'Ungrouped']);
    await expect(page.locator('.trigger-row:visible')).toHaveCount(3);
    await expect(page.locator('.trigger-group-header:visible .section-count-open').first()).toHaveText('2');
    await expect(page.locator('.trigger-group-header:visible .trigger-group-rename')).toHaveCount(1);
    expectInset(await hairlines(page, panel, '.trigger-row'), 'a trigger row');
    await page.locator(panel).first().screenshot({ path: test.info().outputPath('triggers.png') });
  });

  test('Thread queue: drawer headers, the policy button beside the toggle, hairlines under rows', async ({ page }) => {
    await page.route('**/api/v1/thread-queue', (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(QUEUE) }));
    await navigateToApp(page);
    await waitForEventStream(page);
    const res = await apiRequest(page).post('/api/v1/ui/navigate', {
      headers: { 'content-type': 'application/json' },
      data: { target: 'thread-queue' },
    });
    expect(res.ok()).toBeTruthy();
    await expect(page.locator('.list-section-title .section-label:visible', { hasText: 'Running' }))
      .toBeVisible({ timeout: 10_000 });
    await settle(page);

    const panel = '.list-rows.list-rows-divided';
    expectDrawerLook(await headerLooks(page, panel), ['Running', 'Queued']);
    await expect(page.locator(`${panel} .list-section-title:visible .section-count-open`).first()).toHaveText('1/4');
    expectInset(await hairlines(page, panel, '.list-row'), 'a queue row');

    const policy = page.locator('.list-section-actions [data-role="toggle-capacity-policy"]:visible');
    await policy.click();
    await expect(page.locator('.thread-queue-policy:visible')).toBeVisible();
    await expect(policy).toHaveAttribute('aria-expanded', 'true');
    await page.locator(panel).first().screenshot({ path: test.info().outputPath('thread-queue.png') });
  });
});
