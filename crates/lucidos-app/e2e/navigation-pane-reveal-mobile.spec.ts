/**
 * Mobile pane-swipe consistency — every navigation source reveals content.
 *
 * Contract: every user-intent navigation that lands content into the right
 * pane MUST swipe mobile to the `content` pane via `revealContentPane()` —
 * `data-mobile-view` flips from `thread`/`threads` to `content`. See
 * `.claude/rules/frontend.md` § "Navigation that lands content must call
 * revealContentPane()" and `crates/lucidos-app/src/store/actions/pane.ts`.
 *
 * The vitest suite (menu.test, thread-sync-navigation.test, triggers.test)
 * pins each helper at the unit level. This spec covers the real-DOM, real-
 * browser path on both mobile Chromium (375×812) AND iOS WebKit (390×844),
 * where tap→click translation, CSS transform animations, and Preact signal
 * commits actually happen.
 *
 * What's covered here vs. elsewhere:
 *   - SDK `lucidos.ui.navigate(...)` POST → `handleNavigationRequest` — every
 *     target the router knows about. The mobile user might invoke this from
 *     any pane (the `thread` pane is the default after navigateToApp); the
 *     pane MUST end on `content`.
 *   - Notification deep-link hash → `dispatchDeepLink` →
 *     `handleNavigationRequest`. Same router, different entry point.
 *
 * What's NOT covered here:
 *   - Drawer click on mobile. The hamburger button only renders on the
 *     mobile *content* header (MobileAppHeader.tsx:168), so the regression
 *     scenarios the unit tests pin (`item === prev`, `mobileView !== 'thread'`)
 *     aren't reachable from the drawer on mobile. They ARE pinned in
 *     menu.test.ts via the `setActiveMenu (pure plumbing)` describe.
 *   - Chat markdown nav-link tap in an assistant response — would require a
 *     deterministic LLM response. Covered at the linkifier + click-handler
 *     unit level (chat-link-click.test.ts END-TO-END test) instead.
 */
import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, waitForEventStream } from './helpers';
import { clearNotifications } from './db-helpers';

async function expectMobileView(
  page: Page,
  view: 'thread' | 'threads' | 'content',
): Promise<void> {
  await expect(page.locator('.app-header').first()).toHaveAttribute('data-mobile-view', view, { timeout: 10_000 });
}

/** Targets the router accepts that all land on the content pane. Excluded:
 *  `thread` (lands on `thread` pane via focusThread), `new-chat` (stays on
 *  `thread` pane for compose), `url` (no router behaviour to test without a
 *  real URL + Tauri webview), `app`/`file`/`trigger` (require seeded data
 *  in the e2e workspace; pane behaviour is the same as the panel cases via
 *  the shared revealContentPane helper). */
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
      // The SDK calls `lucidos.ui.navigate(target)` from inside an app iframe.
      // The engine POSTs `/api/v1/ui/navigate`, emits NavigationRequested via
      // EventBus, the frontend SSE handler calls handleNavigationRequest.
      // Each target routes to a different branch (switchMenuItem, setActiveMenu
      // + form overlay, openSettingsSubview); every content-landing branch
      // MUST end with the pane on `content`.
      await navigateToApp(page);
      // The stream must be OPEN before the POST. `NavigationRequested` is a
      // transient event: the engine broadcasts it over SSE and never replays
      // it. A page still connecting misses it outright, and no amount of
      // polling recovers it. That is what flaked here on a cold WebKit start,
      // where `data-mobile-view` sat on `thread` for the whole wait.
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
    // Maps to the production push-tap path: Safari's declarative push sets
    // window.location.hash to `#notification=<id>&tap=<json>` when the user
    // taps the OS notification; hashchange → handleHashLocation →
    // dispatchDeepLink → handleNavigationRequest. We drive the same hash-set
    // directly here — the engine's push fan-out is exercised in
    // notifications.spec.ts; this test asserts the pane behavior on land.
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

    // Build the same hash shape parseDeepLinkFromUrl consumes — the structured
    // Tap object as JSON in the `tap=` param.
    const tap = encodeURIComponent(JSON.stringify({ kind: 'navigate', to: { target: 'apps' } }));
    await page.evaluate(({ id, tap }) => {
      window.location.hash = `#notification=${id}&tap=${tap}`;
    }, { id: notification_id, tap });

    await expectMobileView(page, 'content');
  });
});
