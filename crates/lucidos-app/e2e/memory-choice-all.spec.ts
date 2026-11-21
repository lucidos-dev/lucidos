/**
 * A workspace that never chose its memory module asks in the memory choice.
 * Every answer writes `memory_module`, closing included, so it asks once.
 *
 * The e2e reset stores `classic` so the modal stays out of every other spec.
 * Each test here deletes it to meet the modal, and stores Classic again after.
 *
 * No `-desktop` / `-mobile` suffix, so desktop Chromium, phone Chromium and
 * iPhone WebKit all run it.
 */
import { test, expect, type Page } from './fixtures';
import { psql } from './db-helpers';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

const CHOICE = '[data-role="memory-choice"]:visible';
const TILE = '[data-role="memory-choice-tile"]:visible';

/** Back to a workspace that never chose. The row and its source events go, so
 *  nothing can rebuild the old answer. */
function forgetMemoryModule(): void {
  psql([
    "DELETE FROM preferences WHERE key = 'memory_module'",
    "DELETE FROM events WHERE event_type = 'PreferencesChanged' AND payload->>'key' = 'memory_module'",
  ].join(';\n'));
}

async function storedMemoryModule(page: Page): Promise<string | undefined> {
  const res = await page.request.get('/api/v1/preferences');
  expect(res.ok(), `GET /api/v1/preferences -> ${res.status()}`).toBe(true);
  return ((await res.json()) as { preferences: Record<string, string> }).preferences.memory_module;
}

test.describe('the memory choice', () => {
  // Forget after the app has loaded, then reload. `navigateToApp` leaves Home
  // with a click on New thread, which the modal reads as an outside click.
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    forgetMemoryModule();
    await page.reload();
    await waitForEventStream(page);
  });

  // Classic again for the specs after this one. Classic also pauses the
  // compactor that Use Tree starts.
  test.afterEach(async ({ page }) => {
    const res = await apiRequest(page).put('/api/v1/preferences?key=memory_module', { data: { value: 'classic' } });
    expect(res.ok(), `PUT memory_module=classic -> ${res.status()}`).toBe(true);
  });

  test('asks once, and closing it stays on Classic', async ({ page }) => {
    const choice = page.locator(CHOICE);
    await expect(choice).toBeVisible({ timeout: 15_000 });
    await expect(choice).toContainText('Choose how this workspace remembers');
    await expect(choice.locator(TILE)).toHaveCount(3);
    await expect(choice.getByRole('button', { name: 'Use Classic' })).toBeVisible();

    // The panel fits the screen on a phone too, so nothing scrolls sideways.
    const box = await choice.boundingBox();
    const viewport = page.viewportSize();
    expect(box && viewport && box.x >= 0 && box.x + box.width <= viewport.width).toBe(true);

    await choice.getByRole('button', { name: 'Close, and stay on Classic' }).click();
    await expect(choice).toBeHidden();
    await expect.poll(() => storedMemoryModule(page)).toBe('classic');

    await page.reload();
    await waitForEventStream(page);
    await expect(page.locator('[data-role="prompt-input"]:visible').first()).toBeVisible();
    await expect(choice).toHaveCount(0);
  });

  test('Use Tree waits for the estimate, then writes Tree', async ({ page }) => {
    const choice = page.locator(CHOICE);
    await expect(choice).toBeVisible({ timeout: 15_000 });
    const useTree = choice.getByRole('button', { name: 'Use Tree' });
    await expect(useTree).toBeEnabled({ timeout: 30_000 });

    await useTree.click();
    await expect(choice).toBeHidden();
    await expect.poll(() => storedMemoryModule(page)).toBe('tree');
  });
});
