/**
 * Text search in Search Everywhere, against the real engine (ADR 0383). A
 * phrase typed in any case finds its line inside a workspace file. The Text
 * section comes after the name sections, and picking a line opens the file
 * scrolled to it, with the line marked. Find in file opens on the phrase, with
 * the match on that line current and the caret in the field.
 *
 * The unit tests mock the engine and the navigation router. This covers the
 * route, the router and the preview's line landing together.
 */
import { test, expect } from './fixtures';
import { mkdirSync, rmSync, writeFileSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { assertHealthy, navigateToApp, waitForModalExitFade } from './helpers';

const DIR = 'artifacts/e2e-text-search';
const NOTES = `${DIR}/notes.md`;
const LOG = `${DIR}/log.txt`;
const PHRASE = 'quokka-zebra-phrase';
/** Far enough down that landing on it means the preview scrolled. */
const LINE = 90;

function write(rel: string, text: string) {
  const path = resolve(WORKSPACE, 'data', rel);
  mkdirSync(resolve(path, '..'), { recursive: true });
  writeFileSync(path, text);
}

test.describe('Search Everywhere → Text', () => {
  test.beforeAll(() => {
    const lines = Array.from({ length: 150 }, (_, i) => `filler line ${i + 1}`);
    lines[LINE - 1] = `  the ${PHRASE} sits here`;
    write(NOTES, lines.join('\n'));
    write(LOG, `${PHRASE} one\nnothing\n${PHRASE} two\n${PHRASE} three\n`);
  });

  test.afterAll(() => {
    rmSync(resolve(WORKSPACE, 'data', DIR), { recursive: true, force: true });
  });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await page.keyboard.press('ControlOrMeta+k');
    await expect(page.locator('.search-everywhere-input')).toBeVisible();
  });

  test('finds the phrase in any case and opens the file at its line', async ({ page }) => {
    await page.locator('.search-everywhere-input').fill(PHRASE.toUpperCase());

    const hit = page.locator('.search-everywhere-result', { hasText: `${NOTES}:${LINE}` });
    await expect(hit.locator('mark.search-match')).toHaveText(PHRASE);
    const headers = page.locator('.search-everywhere-section-header');
    await expect(headers.last()).toHaveText('text');

    await hit.click();
    const row = page.locator(`.code-line[data-line="${LINE}"]`);
    await expect(row).toHaveClass(/line-selected/, { timeout: 10_000 });
    await expect(row).toBeInViewport();
    await expect(page.locator('.file-preview-frame [data-role="find-input"]')).toHaveValue(PHRASE.toUpperCase());
    await expect(page.locator('.file-preview-frame [data-role="find-status"]')).toHaveText('1 of 1');
  });

  test('opens Find in file on the picked file, current on the picked line', async ({ page }) => {
    const input = page.locator('.search-everywhere-input');
    const findInput = page.locator('.file-preview-frame [data-role="find-input"]');
    const status = page.locator('.file-preview-frame [data-role="find-status"]');

    // Another file shows first, with its own find open, so the find must move
    // to the picked file rather than stay on this one.
    await input.fill(PHRASE);
    await page.locator('.search-everywhere-result', { hasText: `${NOTES}:${LINE}` }).click();
    await expect(status).toHaveText('1 of 1', { timeout: 10_000 });

    // The pick closed Search Everywhere, and its fading drawing keeps the
    // input's class until the fade ends.
    await waitForModalExitFade(page);
    await page.keyboard.press('ControlOrMeta+k');
    await input.fill(PHRASE);
    await page.locator('.search-everywhere-tab', { hasText: /^Text$/ }).click();
    await page.locator('.search-everywhere-result', { hasText: `${LOG}:3` }).click();

    await expect(page.locator('.code-line[data-line="3"]')).toHaveClass(/line-selected/, { timeout: 10_000 });
    await expect(findInput).toHaveValue(PHRASE);
    await expect(status).toHaveText('2 of 3');
    const painted = await page.evaluate(() => ({
      all: CSS.highlights.get('lucidos-find')?.size ?? 0,
      current: [...(CSS.highlights.get('lucidos-find-current') ?? [])]
        .map((r) => (r as Range).startContainer.parentElement?.closest('.code-line')?.getAttribute('data-line')),
    }));
    expect(painted).toEqual({ all: 3, current: ['3'] });

    // The field holds the caret, so Enter steps without a click first.
    await expect(findInput).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(status).toHaveText('3 of 3');
    await page.locator('.file-preview-frame button[aria-label="Next match"]').click();
    await expect(status).toHaveText('1 of 3');
  });

  test('the Text tab lists every line with its totals', async ({ page }) => {
    await page.locator('.search-everywhere-input').fill(PHRASE);
    await page.locator('.search-everywhere-tab', { hasText: /^Text$/ }).click();

    await expect(page.locator('.search-everywhere-summary')).toHaveText('4 matches in 2 files');
    await expect(page.locator('[data-role="search-result"]')).toHaveCount(4);
  });
});
