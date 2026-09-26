/**
 * The first-paint split (ADR 0288), observed in a real browser.
 *
 * The entry chunk is the data layer and startup. The UI is the shell chunk,
 * which loads beside it under the boot splash. Two on-demand surfaces load
 * once the splash lifts. These tests pin the three properties no unit test can
 * see: the order of network and render, the idle loads, and that an idle
 * surface never opens empty.
 */
import { test, expect } from './fixtures';
import { navigateToApp } from './helpers';

test.describe('first-paint split', () => {
  test('startup reaches the engine before the shell draws anything', async ({ page }) => {
    // Recorded in the page, so the two times share one clock. The health probe
    // is startup's first request, and nothing holds it back, where
    // `fetchWithDefaults` holds every other GET for engine readiness. A shell drawn
    // first would mean startup is back inside the UI tree, where every fetch
    // waits for the whole UI to parse and render.
    await page.addInitScript(() => {
      const w = window as unknown as { __healthFetchAt?: number; __shellAt?: number };
      const realFetch = window.fetch.bind(window);
      window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
        if (w.__healthFetchAt === undefined && /\/api\/v1\/health$/.test(url)) {
          w.__healthFetchAt = performance.now();
        }
        return realFetch(input, init);
      };
      new MutationObserver((_, observer) => {
        if (document.querySelector('.app-shell')) {
          w.__shellAt = performance.now();
          observer.disconnect();
        }
      }).observe(document, { childList: true, subtree: true });
    });

    await navigateToApp(page);

    const times = await page.evaluate(() => {
      const w = window as unknown as { __healthFetchAt?: number; __shellAt?: number };
      return { fetchAt: w.__healthFetchAt, shellAt: w.__shellAt };
    });
    expect(times.fetchAt, 'startup must probe the engine').toBeDefined();
    expect(times.shellAt, 'the shell must render').toBeDefined();
    expect(times.fetchAt!).toBeLessThan(times.shellAt!);
  });

  test('the shell chunk is preloaded, and the idle chunks load after the splash', async ({ page }) => {
    const scripts: string[] = [];
    page.on('request', (r) => {
      if (r.resourceType() === 'script') scripts.push(new URL(r.url()).pathname);
    });

    await navigateToApp(page);
    await expect(page.locator('.boot-splash')).toHaveCount(0);

    const html = await page.evaluate(() => document.head.innerHTML);
    expect(html, 'index.html must modulepreload the shell chunk').toMatch(/rel="modulepreload"[^>]*App-[\w-]+\.js/);

    // No user action: the idle prefetch alone must fetch both.
    await expect.poll(() => scripts.some((p) => /\/WorkspaceSwitcher-[\w-]+\.js$/.test(p))).toBe(true);
    await expect.poll(() => scripts.some((p) => /\/TodoListPanel-[\w-]+\.js$/.test(p))).toBe(true);
  });

  test('the brand menu never draws without its workspace row', async ({ page }) => {
    // A mutation observer runs before the next paint, so it sees every tree a
    // frame could show. A menu committed without the row is the pop-in the
    // idle prefetch and the open-after-preload exist to prevent.
    await page.addInitScript(() => {
      const w = window as unknown as { __menuWithoutRow?: boolean };
      new MutationObserver(() => {
        const menu = document.querySelector('.brand-menu');
        if (menu && !menu.textContent?.includes('Workspaces')) w.__menuWithoutRow = true;
      }).observe(document, { childList: true, subtree: true });
    });

    await navigateToApp(page);
    await page.locator('[data-role="brand-menu-toggle"]:visible').first().click();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await expect(page.locator('.brand-menu')).toContainText('Workspaces');

    const sawEmpty = await page.evaluate(
      () => (window as unknown as { __menuWithoutRow?: boolean }).__menuWithoutRow ?? false,
    );
    expect(sawEmpty).toBe(false);
  });
});
