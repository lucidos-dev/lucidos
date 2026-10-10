/**
 * The theme review loop: PNGs of the default theme and a square phosphor theme,
 * so a theme-wide change can be looked at, and compared, before it is applied.
 *
 *   THEME_SHOTS=1 ./scripts/e2e-browser.sh -f theme-shots.spec.ts
 *   THEME_SHOTS=1 ./scripts/e2e-browser.sh --webkit -f theme-shots.spec.ts
 *
 * The PNGs land in `test-results/theme-shots/`, or in `THEME_SHOTS_DIR`. Run it
 * before and after a change and compare the default-theme files byte for byte:
 * the scene is fixed markup, the caret is still and motion is off, so an equal
 * rendering gives equal bytes.
 *
 * Skipped unless `THEME_SHOTS=1`: it asserts nothing, like header-shots.
 */
import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect, type Page } from './fixtures';
import {
  apiRequest, ensureMobileView, gotoWithRetry, isMobileViewport, setDevicePreference, waitForPaneAtRest,
  waitForVisibleInput,
} from './helpers';

const SHOOTING = process.env.THEME_SHOTS === '1';
const here = dirname(fileURLToPath(import.meta.url));
const DIR = process.env.THEME_SHOTS_DIR ?? resolve(here, '..', 'test-results', 'theme-shots');
const SQUARE = 'e2e-square-phosphor';

/** The retro fixture with every radius token the catalog offers set to 0. */
async function squareTheme(page: Page): Promise<string> {
  const theme = JSON.parse(readFileSync(resolve(here, 'themes', 'retro-phosphor.json'), 'utf-8'));
  const res = await page.request.get('/api/v1/themes/tokens');
  expect(res.ok()).toBe(true);
  const names: string[] = (await res.json()).tokens.map((t: { name: string }) => t.name);
  theme.name = 'Square phosphor';
  theme.tokens = {
    ...theme.tokens,
    ...Object.fromEntries(names.filter(n => n.startsWith('--radius-')).map(n => [n, '0'])),
  };
  return JSON.stringify(theme);
}

/** A fixed transcript, a row of chips and the two scroll buttons, at the top
 *  of the thread pane. The new-thread view mounts no scroll buttons, so the
 *  scene brings its own pair. */
async function injectScene(page: Page): Promise<void> {
  await page.evaluate(() => {
    document.querySelector('#ls-root')?.remove();
    const host = ['.thread-content', '.thread-pane']
      .flatMap(s => [...document.querySelectorAll<HTMLElement>(s)])
      .find(el => el.getBoundingClientRect().width > 0);
    if (!host) throw new Error('no visible thread pane');
    const root = document.createElement('div');
    root.id = 'ls-root';
    root.innerHTML = `
      <div class="response-body"><div class="response-content"><div class="response-chunk markdown-content">
        <h3>What the theme adds</h3>
        <p>Scanlines across the whole screen. <strong>Bold text</strong> stays crisp,
        and <code>inline code</code> keeps its box.</p>
        <ul><li><strong>Block caret</strong> in the composer.</li><li>Double borders on menus.</li></ul>
      </div></div></div>
      <div style="display: flex; gap: 0.5rem; flex-wrap: wrap; padding: 0.5rem 0">
        <span class="section-count-badge">3</span>
        <span class="label">label</span>
        <span class="pill-bar-btn active">pill</span>
        <button class="action-btn">Archive</button>
        <span class="status-dot" style="width: 0.5rem; height: 0.5rem; background: currentcolor"></span>
      </div>
      <div style="position: relative; height: 6.5rem">
        <button class="scroll-to-top visible" style="top: 0.5rem" aria-label="Scroll to top">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2"><path d="M6 15l6-6 6 6"/></svg>
        </button>
        <button class="scroll-to-bottom visible" style="bottom: 0.5rem" aria-label="Scroll to bottom">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2"><path d="M6 9l6 6 6-6"/></svg>
        </button>
      </div>`;
    host.prepend(root);
  });
}

async function shoot(page: Page, project: string, theme: string): Promise<void> {
  await gotoWithRetry(page, '/');
  await expect(page.locator('.boot-splash')).toHaveCount(0, { timeout: 30_000 });
  if (isMobileViewport(page)) {
    await ensureMobileView(page, 'thread');
    await waitForPaneAtRest(page);
  }
  const composer = await waitForVisibleInput(page);
  await injectScene(page);
  await composer.fill('Some parts way too blurry');
  await composer.focus();
  await page.evaluate(() => document.fonts.ready);
  // The thread pane only: the content pane beside it lists workspace files,
  // which differ from run to run.
  const pane = await page.locator('#ls-root').locator('..').boundingBox();
  if (!pane) throw new Error('no thread pane');
  const viewport = page.viewportSize()!;
  mkdirSync(DIR, { recursive: true });
  await page.screenshot({
    path: resolve(DIR, `${project}-${theme}.png`),
    caret: 'initial',
    animations: 'disabled',
    clip: { x: 0, y: 0, width: pane.x + pane.width, height: viewport.height },
  });
}

test.describe('theme shots', () => {
  test.skip(!SHOOTING, 'set THEME_SHOTS=1 to take the theme shots');

  test.afterEach(async ({ page }) => {
    await setDevicePreference(page, 'theme', 'lucidos');
    await setDevicePreference(page, 'motion', 'system');
    await apiRequest(page).delete(`/api/v1/data/themes/${SQUARE}.json`);
  });

  test('the default theme and a square phosphor theme', async ({ page }, testInfo) => {
    await gotoWithRetry(page, '/');
    await setDevicePreference(page, 'motion', 'reduce');
    await setDevicePreference(page, 'theme', 'lucidos');
    await shoot(page, testInfo.project.name, 'default');

    const res = await apiRequest(page).put(`/api/v1/data/themes/${SQUARE}.json`, { data: await squareTheme(page) });
    expect(res.ok(), await res.text()).toBe(true);
    await setDevicePreference(page, 'theme', SQUARE);
    await shoot(page, testInfo.project.name, 'square-phosphor');
  });
});
