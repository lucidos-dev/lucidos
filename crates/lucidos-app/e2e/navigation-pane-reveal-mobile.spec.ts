import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, waitForEventStream } from './helpers';
import { clearNotifications } from './db-helpers';

async function expectMobileView(
  page: Page,
  view: 'thread' | 'threads' | 'content',
): Promise<void> {
  await expect(page.locator('.app-header').first()).toHaveAttribute('data-mobile-view', view, { timeout: 10_000 });
}

const CONTENT_LANDING_TARGETS = [
  'apps',
  'triggers',
  'files',
  'changes',
  'notifications',
  'settings',
  'new-app',
  'new-trigger',
] as const;

test.describe('Mobile pane-swipe — every navigation source reveals the content pane', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
    test.skip(!isMobileViewport(page), 'Mobile pane behavior only — skipped on desktop project');
  });

  for (const target of CONTENT_LANDING_TARGETS) {
    test(`POST /api/v1/ui/navigate { target: "${target}" } swipes to content`, async ({ page }) => {
      await navigateToApp(page);
      await waitForEventStream(page);
      await expectMobileView(page, 'thread');

      const res = await apiRequest(page).post('/api/v1/ui/navigate', {
        headers: { 'content-type': 'application/json' },
        data: { target },
      });
      expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

      await expectMobileView(page, 'content');
    });
  }

  test('notification deep-link (tap=navigate) hash-dispatch swipes to content', async ({ page }) => {
    await navigateToApp(page);
    await expectMobileView(page, 'thread');

    const postRes = await apiRequest(page).post('/api/v1/notifications', {
      headers: { 'content-type': 'application/json' },
      data: {
        title: 'Go to apps',
        message: 'tap me',
        tap: { kind: 'navigate', to: { target: 'apps' } },
      },
    });
    expect(postRes.ok(), `POST /api/v1/notifications -> ${postRes.status()}`).toBeTruthy();
    const { notification_id } = (await postRes.json()) as { notification_id: string };

    const tap = encodeURIComponent(JSON.stringify({ kind: 'navigate', to: { target: 'apps' } }));
    await page.evaluate(({ id, tap }) => {
      window.location.hash = `#notification=${id}&tap=${tap}`;
    }, { id: notification_id, tap });

    await expectMobileView(page, 'content');
  });
});
