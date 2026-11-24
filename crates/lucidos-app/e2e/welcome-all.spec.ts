import { test, expect, Locator, Page } from './fixtures';
import { navigateToApp, assertHealthy, assertUserMessagesVisible, waitForVisibleInput, isMobileViewport } from './helpers';
import { clearAllThreads, psql, resetWelcomePreference } from './db-helpers';

async function pressInterview(page: Page, button: Locator): Promise<void> {
  if (isMobileViewport(page)) await button.tap();
  else await button.click();
}

test.describe('Welcome surface', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    resetWelcomePreference();
    await assertHealthy(page);
  });

  test('shows on a fresh compose view with real (unclipped) height', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });
    await expect(welcome.getByText('Hi, there!')).toBeVisible();

    await expect
      .poll(async () => welcome.evaluate((el) => getComputedStyle(el).opacity), { timeout: 5_000 })
      .toBe('1');

    const wrap = page.locator('.thread-content-wrap:visible').first();
    const box = await wrap.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.height).toBeGreaterThan(100);
  });

  test('the setup-interview button is the only action, and it sends on one click', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    const start = page.locator('.welcome-setup-interview-btn:visible').first();
    await expect(start).toBeVisible();
    await expect(start).toHaveText(/Help me get the most out of Lucidos/);
    await expect(welcome.locator('button')).toHaveCount(2);

    await pressInterview(page, start);
    await assertUserMessagesVisible(page, ['Help me get the most out of Lucidos']);
    const input = await waitForVisibleInput(page, 10_000);
    await expect.poll(async () => (await input.inputValue()).trim(), { timeout: 5_000 }).toBe('');
  });

  test('draft in progress: the interview takes a draft of its own and asks nothing', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    const input = await waitForVisibleInput(page, 10_000);
    await input.click();
    await input.fill('my own half-typed idea');
    await expect.poll(async () => (await input.inputValue()).trim()).toBe('my own half-typed idea');

    const start = page.locator('.welcome-setup-interview-btn:visible').first();
    await pressInterview(page, start);

    await expect(page.locator('.confirm-dialog')).toHaveCount(0);
    await assertUserMessagesVisible(page, ['Help me get the most out of Lucidos']);

    await expect
      .poll(
        () => psql(
          "select count(*) from thread_summaries where compose_text = 'my own half-typed idea'",
        ).trim(),
        { timeout: 10_000 },
      )
      .toBe('1');
  });

  test('its text column lands on the composer box, both edges', async ({ page }) => {
    await navigateToApp(page);

    const welcome = page.locator('.welcome-message:visible').first();
    await expect(welcome).toBeVisible({ timeout: 10_000 });

    const edges = await page.evaluate(() => {
      const visible = <T extends Element>(els: NodeListOf<T>): T | null => {
        for (let i = els.length - 1; i >= 0; i--) {
          if (els[i].getBoundingClientRect().width > 0) return els[i];
        }
        return null;
      };
      const w = visible(document.querySelectorAll<HTMLElement>('.welcome-message'));
      const box = visible(document.querySelectorAll<HTMLElement>('.prompt-box'));
      if (!w || !box) return null;
      const wRect = w.getBoundingClientRect();
      const wStyle = getComputedStyle(w);
      const boxRect = box.getBoundingClientRect();
      return {
        left: wRect.left + parseFloat(wStyle.paddingLeft),
        right: wRect.right - parseFloat(wStyle.paddingRight),
        boxLeft: boxRect.left,
        boxRight: boxRect.right,
      };
    });
    expect(edges, 'welcome / prompt box not laid out').not.toBeNull();
    const e = edges!;

    expect(e.left, `welcome left=${e.left} vs composer left=${e.boxLeft}`)
      .toBeCloseTo(e.boxLeft, 0);
    expect(e.right, `welcome right=${e.right} vs composer right=${e.boxRight}`)
      .toBeCloseTo(e.boxRight, 0);
  });

  test('"Don\'t show this again" dismisses it and it stays dismissed after reload', async ({ page }) => {
    await navigateToApp(page);

    const dismiss = page.locator('.welcome-dismiss:visible').first();
    await expect(dismiss).toBeVisible({ timeout: 10_000 });
    const saved = page.waitForResponse((res) => res.request().method() === 'PUT'
      && res.url().includes('key=welcome_suggestions_dismissed'));
    await dismiss.click();

    await expect(page.locator('.welcome-message')).toHaveCount(0, { timeout: 5_000 });
    expect((await saved).ok()).toBe(true);

    await page.reload();
    await navigateToApp(page);
    await waitForVisibleInput(page);
    await expect(page.locator('.welcome-message')).toHaveCount(0, { timeout: 10_000 });
  });
});
