/**
 * Text search in Search Everywhere, against the real engine (ADR 0383). A
 * phrase typed in any case finds its line inside a workspace file. The Text
 * section comes after the name sections, and picking a line opens the file
 * scrolled to it, with the line marked.
 *
 * The unit tests mock the engine and the navigation router. This covers the
 * route, the router and the preview's line landing together.
 */
import { test, expect } from './fixtures';
import { mkdirSync, rmSync, writeFileSync } from 'fs';
import { resolve } from 'path';
import { WORKSPACE } from './db-helpers';
import { assertHealthy, navigateToApp } from './helpers';

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
  });

  test('the Text tab lists every line with its totals', async ({ page }) => {
    await page.locator('.search-everywhere-input').fill(PHRASE);
    await page.locator('.search-everywhere-tab', { hasText: /^Text$/ }).click();

    await expect(page.locator('.search-everywhere-summary')).toHaveText('4 matches in 2 files');
    await expect(page.locator('[data-role="search-result"]')).toHaveCount(4);
  });
});
