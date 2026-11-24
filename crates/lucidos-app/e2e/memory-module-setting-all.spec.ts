import { test, expect, type Page } from './fixtures';
import {
  apiRequest,
  assertHealthy,
  navigateToApp,
  openSettingsView,
  pickModelPair,
  uniqueMessage,
  waitForEventStream,
} from './helpers';

const MODULE_ROW = '[data-search-anchor="memory:module"]:visible';
const CONFIRM = '[data-role="tree-confirm"]:visible';
const ESTIMATE = '[data-role="tree-estimate"]:visible';
const BACKFILL = '[data-role="tree-backfill"]:visible';
const COMPACTION_LINK = '[data-role="memory-compaction-link"]:visible';
const COMPACTION_ROW = '[data-search-anchor="models:summary-compaction"]:visible';
const INSPECTOR = '.memory-stats-bar:visible';
const LABEL = '.progress-label:not(.memory-tree-detail)';

declare global {
  interface Window {
    __treeBackfillLabels?: string[];
  }
}

async function storedMemoryModule(page: Page): Promise<string | undefined> {
  const res = await page.request.get('/api/v1/preferences');
  expect(res.ok(), `GET /api/v1/preferences -> ${res.status()}`).toBe(true);
  return ((await res.json()) as { preferences: Record<string, string> }).preferences.memory_module;
}

async function resetToClassic(page: Page): Promise<void> {
  const res = await apiRequest(page).put('/api/v1/preferences?key=memory_module', { data: { value: 'classic' } });
  expect(res.ok(), `PUT memory_module=classic -> ${res.status()}`).toBe(true);
}

const BACKFILL_READ = '**/api/v1/memory/tree-backfill';

test.use({ serviceWorkers: 'block' });

async function readAsNeverStarted(page: Page): Promise<void> {
  await page.route(BACKFILL_READ, (route) => route.fulfill({ json: { state: 'off', started: false } }));
}

async function seedThread(page: Page): Promise<void> {
  const res = await apiRequest(page).post('/api/v1/chat/stream', {
    headers: { 'content-type': 'application/json' },
    data: { message: uniqueMessage('tree-backfill'), mode: 'human' },
  });
  expect(res.ok(), `POST /api/v1/chat/stream -> ${res.status()}`).toBe(true);
}

async function openMemoryPage(page: Page): Promise<void> {
  await openSettingsView(page, 'memory');
  await expect(page.locator(MODULE_ROW)).toBeVisible({ timeout: 15_000 });
}

function moduleButton(page: Page, name: 'Classic' | 'Tree') {
  return page.locator(MODULE_ROW).getByRole('button', { name, exact: true });
}

async function recordBackfillLabels(page: Page): Promise<void> {
  await page.evaluate(() => {
    const labels: string[] = [];
    window.__treeBackfillLabels = labels;
    const read = () => {
      const label = [...document.querySelectorAll('[data-role="tree-backfill"] .progress-label')]
        .map((el) => el.textContent?.trim() ?? '')
        .filter((text) => text !== '')
        .join(' | ');
      if (label && labels[labels.length - 1] !== label) labels.push(label);
    };
    new MutationObserver(read).observe(document.body, {
      subtree: true,
      childList: true,
      characterData: true,
    });
  });
}

const BUILDING = /^Building memory, (\d+)%.* \| (\d+) of (\d+) trees/;

test.describe('the memory module setting', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await waitForEventStream(page);
    await resetToClassic(page);
  });

  test.afterEach(async ({ page }) => {
    await resetToClassic(page);
  });

  test('Tree asks first: the estimate shows and Cancel spends nothing', async ({ page }) => {
    expect(await storedMemoryModule(page)).toBe('classic');
    await readAsNeverStarted(page);

    await openMemoryPage(page);
    await expect(moduleButton(page, 'Classic')).toHaveAttribute('aria-pressed', 'true');
    await expect(page.locator(INSPECTOR).first()).toBeVisible();
    await expect(page.locator(COMPACTION_LINK)).toHaveCount(0);
    await moduleButton(page, 'Tree').click();

    await expect(page.locator(CONFIRM)).toBeVisible();
    await expect(page.locator(ESTIMATE)).toHaveAttribute('data-state', 'loaded', { timeout: 30_000 });
    await expect(page.locator(ESTIMATE)).toContainText(/calls?/i);
    await expect(page.locator(CONFIRM).getByRole('button', { name: 'Start Tree' })).toBeEnabled();
    expect(await storedMemoryModule(page)).toBe('classic');

    await page.locator(CONFIRM).getByRole('button', { name: 'Cancel' }).click();
    await expect(page.locator(CONFIRM)).toHaveCount(0);
    await expect(moduleButton(page, 'Classic')).toHaveAttribute('aria-pressed', 'true');
    expect(await storedMemoryModule(page)).toBe('classic');
  });

  test('Start runs the backfill: the bar moves to Ready, and Tree survives a reload', async ({ page }) => {
    await readAsNeverStarted(page);
    await seedThread(page);
    await openMemoryPage(page);
    await moduleButton(page, 'Tree').click();
    const start = page.locator(CONFIRM).getByRole('button', { name: 'Start Tree' });
    await expect(start).toBeEnabled({ timeout: 30_000 });
    await page.unroute(BACKFILL_READ);

    await recordBackfillLabels(page);
    await start.click();
    await expect(moduleButton(page, 'Tree')).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => storedMemoryModule(page)).toBe('tree');

    const bar = page.locator(BACKFILL);
    await expect(bar).toBeVisible();
    await expect(bar.locator(LABEL)).toHaveText('Ready', { timeout: 90_000 });
    await expect(bar).toHaveAttribute('data-state', 'ready');
    await expect(page.locator(INSPECTOR)).toHaveCount(0);

    const labels = await page.evaluate(() => window.__treeBackfillLabels ?? []);
    const counts = labels
      .map((l) => BUILDING.exec(l))
      .filter((m): m is RegExpExecArray => m !== null)
      .map((m) => ({ done: Number(m[2]), total: Number(m[3]) }));
    expect(counts.length, `labels seen: ${JSON.stringify(labels)}`).toBeGreaterThan(0);
    expect(counts.some((c) => c.done < c.total), JSON.stringify(labels)).toBe(true);
    for (let i = 1; i < counts.length; i++) expect(counts[i].done).toBeGreaterThanOrEqual(counts[i - 1].done);
    expect(labels[labels.length - 1]).toBe('Ready');

    const box = await bar.locator('.progress-bar').boundingBox();
    const viewport = page.viewportSize();
    expect(box && viewport && box.x >= 0 && box.x + box.width <= viewport.width).toBe(true);

    await page.reload();
    await waitForEventStream(page);
    await openMemoryPage(page);
    await expect(moduleButton(page, 'Tree')).toHaveAttribute('aria-pressed', 'true');
    await expect(moduleButton(page, 'Classic')).toHaveAttribute('aria-pressed', 'false');
    await expect(page.locator(BACKFILL).locator(LABEL)).toHaveText('Ready', { timeout: 15_000 });

    await expect(page.locator(COMPACTION_LINK)).toContainText("writes Tree's summaries. Change it under");

    await moduleButton(page, 'Classic').click();
    await expect.poll(() => storedMemoryModule(page)).toBe('classic');
    await expect(page.locator(INSPECTOR).first()).toBeVisible();
    await moduleButton(page, 'Tree').click();
    await expect.poll(() => storedMemoryModule(page)).toBe('tree');
    await expect(page.locator(CONFIRM)).toHaveCount(0);
    await expect(page.locator(BACKFILL).locator(LABEL)).toHaveText('Ready', { timeout: 90_000 });

    await page.locator(COMPACTION_LINK).getByRole('button', { name: 'Models → Background tasks' }).click();
    await expect(page.locator(COMPACTION_ROW)).toBeInViewport({ timeout: 10_000 });
  });

  test('picking GPT-6.1 Sol in the confirm modal prices it', async ({ page }) => {
    await readAsNeverStarted(page);
    await openMemoryPage(page);
    await moduleButton(page, 'Tree').click();
    await expect(page.locator(ESTIMATE)).toHaveAttribute('data-state', 'loaded', { timeout: 30_000 });

    await page.locator(CONFIRM).locator('.dropdown-trigger').click();
    await pickModelPair(page, 'gpt-6.1-sol');

    await expect(page.locator(ESTIMATE)).not.toContainText('No estimate for this model');
    await expect(page.locator(ESTIMATE)).toContainText('$');
  });

  test('Classic hides the compactor model on Models', async ({ page }) => {
    await openSettingsView(page, 'models');
    await expect(page.locator('[data-search-anchor="models:conversation-summary"]:visible')).toBeVisible({ timeout: 15_000 });
    await expect(page.locator(COMPACTION_ROW)).toHaveCount(0);
  });
});
