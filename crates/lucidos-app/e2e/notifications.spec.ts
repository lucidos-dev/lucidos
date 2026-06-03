import { test, expect, Page } from '@playwright/test';
import { assertHealthy, clickVisibleElement, ensureMobileView, expectPushSent, navigateToApp, waitForVisibleInput } from './helpers';
import { clearNotifications, psql } from './db-helpers';

/** Send a notification through the script-facing endpoint that powers the
 *  `lucidos notify` CLI. Same shape as `send_notification` from the chat LLM.
 *
 *  `tap` is the structured discriminated union — engine API enforces the same
 *  shape as the SDK `Tap` type. This helper deliberately accepts `unknown` for
 *  the field so the e2e suite can probe both well-formed and (in future
 *  negative tests) malformed payloads without the type-checker rejecting
 *  fixture shapes the engine itself will validate. */
async function postNotification(
  page: Page,
  body: {
    title: string;
    message: string;
    thread_id?: string;
    event_id?: string;
    app_id?: string;
    tap?: unknown;
  },
): Promise<void> {
  const res = await page.request.post('/api/v1/notifications', {
    headers: { 'content-type': 'application/json' },
    data: body,
  });
  expect(res.ok(), `POST /api/v1/notifications -> ${res.status()}`).toBeTruthy();
}

test.describe('Notifications modal does not auto-open', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test('NotificationCreated on foregrounded page leaves the modal closed', async ({ page }) => {
    await navigateToApp(page);
    // Bell only renders on the content pane on mobile (no-op on desktop).
    await ensureMobileView(page, 'content');
    await expect(page.locator('.notifications-bell:visible').first()).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.notifications-modal')).toHaveCount(0);

    await postNotification(page, {
      title: 'Heads up',
      message: 'A new thing happened',
    });

    // Bell badge bumps — sanity check the SSE landed (otherwise the
    // modal-closed assertion below is vacuous).
    await expect(page.locator('.notifications-bell:visible .badge').first()).toHaveText('1', { timeout: 5_000 });

    // Wait-then-check: Playwright's auto-retrying assertions PASS on the
    // first poll the condition holds, so `toHaveCount(0, { timeout })`
    // returns instantly when the modal is already closed — useless for
    // proving absence over a window. Sleep then assert once so a delayed
    // opener that flips the signal inside the window is actually caught.
    await page.waitForTimeout(500);
    await expect(page.locator('.notifications-modal')).toHaveCount(0);
  });

  test('NotificationCreated while notifications panel is open leaves the modal closed', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'content');

    // Switch to the notifications panel via the bell — same path the user takes.
    await clickVisibleElement(page, '.notifications-bell');
    await expect(
      page.locator('.empty-state:has-text("No"), .notification-item').first(),
    ).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.notifications-modal')).toHaveCount(0);

    await postNotification(page, {
      title: 'Heads up 2',
      message: 'Another new thing',
    });

    // The list reloads (handleNotificationSSE → loadNotifications when panel
    // is active). The new row should appear, but the MODAL must stay closed.
    await expect(
      page.locator('.notification-item:has-text("Heads up 2")').first(),
    ).toBeVisible({ timeout: 5_000 });

    // Same wait-then-check as scenario 1 — Playwright's auto-retry would
    // pass instantly against an already-zero count.
    await page.waitForTimeout(500);
    await expect(page.locator('.notifications-modal')).toHaveCount(0);
  });
});

test.describe('Notifications infinite scroll', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test('scrolling the list loads older pages beyond the first', async ({ page }) => {
    // Seed more than one page (PAGE_SIZE = 15) of notifications with distinct,
    // strictly-decreasing created_at timestamps so cursor pagination is
    // deterministic — no same-instant ties at the page boundary that could
    // make the `before` cursor skip or repeat a row.
    const TOTAL = 30;
    psql(
      `INSERT INTO notifications (id, title, message, read, created_at) ` +
        `SELECT gen_random_uuid(), 'Infinite scroll ' || g, 'body ' || g, false, ` +
        `NOW() - (g || ' seconds')::interval ` +
        `FROM generate_series(1, ${TOTAL}) AS g`,
    );

    await navigateToApp(page);
    await ensureMobileView(page, 'content');

    // Open the notifications panel via the bell — same path the user takes.
    await clickVisibleElement(page, '.notifications-bell');

    // Count only physically-visible rows: the inactive dual-render layout copy
    // (desktop vs mobile) is display:none / 0x0 and must not be counted.
    const visibleCount = () =>
      page.evaluate(() => {
        const els = document.querySelectorAll('.notification-item');
        return Array.from(els).filter((el) => {
          const r = el.getBoundingClientRect();
          return r.width > 0 && r.height > 0;
        }).length;
      });

    // First page renders exactly one PAGE_SIZE — the local poll samples the
    // freshly-rendered 15 well before any async page-2 fetch could complete,
    // so this also proves the list paginates instead of dumping all 30 at once.
    await expect.poll(visibleCount, { timeout: 10_000 }).toBe(15);

    // Scroll the REAL scroll container (.content-pane-body) to the bottom. The
    // bug: the load-more trigger was bound to the inner .panel-content, which
    // has no overflow and never scrolls — so the next page never loaded and the
    // list stayed stuck at the first 15. Scroll inside the poll so multi-page
    // lists keep advancing each iteration.
    await expect
      .poll(
        async () => {
          await page.evaluate(() => {
            const els = document.querySelectorAll('.content-pane-body');
            for (const el of els) {
              const r = el.getBoundingClientRect();
              if (r.width > 0 && r.height > 0) {
                el.scrollTop = el.scrollHeight;
                return;
              }
            }
          });
          return visibleCount();
        },
        { timeout: 15_000, intervals: [200, 300, 500, 1000] },
      )
      .toBe(TOTAL);
  });
});

test.describe('Declarative Web Push payload', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test('push payload is the Declarative Web Push envelope with relative navigate URL', async ({ page }) => {
    // Regression guard for the iOS-PWA push-tap navigation bug:
    // Safari 18.5+ only handles push notifications declaratively (bypassing
    // the SW push handler so it doesn't depend on `notificationclick`) when
    // the on-wire payload conforms to the declarative envelope. Pre-fix,
    // the engine emitted a flat `{title, body, ...}` payload; Safari fell
    // back to legacy SW dispatch, which is the regression vector.
    //
    // We don't drive a real OS tap from Playwright (out of reach). The
    // assertion that protects the iOS path is purely the on-wire shape —
    // Safari does the rest if the envelope is right. The page-side
    // dispatcher (handleHashLocation → dispatchDeepLink) is exercised below
    // via window.location.hash since that's the same code path Safari
    // triggers on tap.

    // Register a synthetic device + push subscription so the engine fan-out
    // has a target and writes to push_log (the test-mode stub records under
    // the `e2e-test-hooks` feature). The push_test_log path skips subs with
    // no device_id (rows can't be attributed for assertions), so we insert a
    // device row first and bind the subscription to it. NOTE: don't navigate
    // to the app first — `navigateToApp` would register THIS browser as a
    // device and start a presence heartbeat, making it an "active" candidate.
    // The engine's PresenceCheck would then suppress the push (rightly so —
    // it's the user IS looking at the page rule), masking our payload shape
    // assertion. We hit the API directly until the assertions are recorded.
    const deviceId = `e2e-declarative-${Date.now()}`;
    psql(
      `INSERT INTO devices (id, name, user_agent, push_enabled) ` +
        `VALUES ('${deviceId}', 'e2e-declarative', 'e2e-test-ua', true)`,
    );
    // Clear device_presence so no stale row (from a previous serial test in
    // this project that called navigateToApp) counts as an active candidate
    // and suppresses the push. e2e workspace is the right scope for this —
    // Playwright projects run serially against a single workspace DB.
    psql(`DELETE FROM device_presence`);
    const subRes = await page.request.post('/api/v1/push/subscribe', {
      headers: { 'content-type': 'application/json' },
      data: {
        endpoint: `https://push.test/synthetic/${Date.now()}`,
        p256dh: 'p256dh-test',
        auth: 'auth-test',
        device_id: deviceId,
      },
    });
    expect(subRes.ok(), `POST /api/v1/push/subscribe -> ${subRes.status()}`).toBeTruthy();

    // Use a UUID-formatted but synthetic thread/event id — focusThreadOrBootstrap
    // surfaces a "Thread not found" toast for unknown ids, which is fine.
    // The assertions we care about are the engine-side payload shape and the
    // page-side mark-read dispatch, both of which fire regardless of whether
    // the deep-link target actually resolves to a thread row.
    const fakeThreadId = '00000000-0000-4000-8000-000000000001';
    const fakeEventId = '00000000-0000-4000-8000-000000000002';

    const res = await page.request.post('/api/v1/notifications', {
      headers: { 'content-type': 'application/json' },
      data: {
        title: 'Claude is asking',
        message: 'tap me',
        thread_id: fakeThreadId,
        event_id: fakeEventId,
        tap: {
          kind: 'navigate',
          to: { target: 'thread', id: fakeThreadId, event_id: fakeEventId },
        },
      },
    });
    expect(res.ok(), `POST /api/v1/notifications -> ${res.status()}`).toBeTruthy();
    const body = (await res.json()) as { notification_id: string };
    const notificationId = body.notification_id;

    // PresenceCheck has zero candidates (device_presence cleared above) so
    // push fan-out is immediate — no deadline to wait through.
    const entry = await expectPushSent(page, notificationId, { timeoutMs: 5000 });
    expect(entry.payload, 'push_log row must carry the recorded payload bytes').toBeTruthy();
    const payload = JSON.parse(entry.payload!) as Record<string, unknown>;

    // (1) Declarative envelope: top-level `web_push: 8030` magic plus a
    //     `notification` object — Safari 18.5+ keys off these to bypass the SW.
    expect(payload.web_push).toBe(8030);
    expect(payload.notification).toBeTruthy();
    const notif = payload.notification as Record<string, unknown>;
    expect(notif.title).toBe('Claude is asking');
    expect(notif.body).toBe('tap me');

    // (2) Tag stamped from notification_id so OS-level dedup works the same
    //     as the SW's prior `tag: data.notification_id`.
    expect(notif.tag).toBe(notificationId);

    // (3) iOS navigate URL — a CROSS-DOCUMENT (query) relative URL. Safari's
    //     declarative-push handler reuses an already-open PWA window on tap; a
    //     same-document (hash-only) navigation is NOT applied to it (WebKit just
    //     focuses the window, the URL never updates, the deep link silently
    //     no-ops — the "tap nav to thread only focuses the app" bug). A query
    //     string forces a real navigation. Relative per W3C Push API
    //     §"Receiving a Push Message" (PR #385) — base-resolved against the
    //     subscription scope so the engine doesn't track each device's origin.
    const navigateUrl = notif.navigate as string;
    expect(navigateUrl.startsWith('/?'), `iOS navigate must be a query URL, got ${navigateUrl}`).toBeTruthy();
    expect(navigateUrl).toContain(`notification=${notificationId}`);
    expect(navigateUrl).toContain(`thread=${fakeThreadId}`);
    expect(navigateUrl).toContain(`event=${fakeEventId}`);
    expect(navigateUrl).toContain('tap=');

    // (4) data.* mirrors the SW-side flat fields it replaces — Chrome's
    //     notificationclick reads them off event.notification.data. data.navigate
    //     is the HASH form (warm `client.navigate()` = no reload), carrying the
    //     SAME params as the iOS query URL — only the `?` vs `#` prefix differs.
    const data = notif.data as Record<string, unknown>;
    expect(data.notification_id).toBe(notificationId);
    expect(data.thread_id).toBe(fakeThreadId);
    expect(data.event_id).toBe(fakeEventId);
    const swNavigate = data.navigate as string;
    expect(swNavigate.startsWith('/#'), `Chrome SW navigate must be a hash URL, got ${swNavigate}`).toBeTruthy();
    expect(swNavigate.slice(2)).toBe(navigateUrl.slice(2)); // same params, different prefix
    expect(data.tap).toEqual({
      kind: 'navigate',
      to: { target: 'thread', id: fakeThreadId, event_id: fakeEventId },
    });

    // (5) Page-side dispatch contract: when iOS navigates the PWA window to the
    //     query navigate URL (a cross-document load), the cold-start /
    //     resume hash router runs handleHashLocation → dispatchDeepLink, which
    //     reads the query params and calls markReadOptimistic. Drive the exact
    //     URL Safari would land on. (We deferred navigation until now so the
    //     test browser didn't count as an active device during the
    //     PresenceCheck above.)
    await page.goto(navigateUrl);
    // Confirm the SPA actually mounted (so useStartup's cold-start hash router
    // runs and reads the query params). The deep-link targets a fake thread, so
    // a "Thread not found" toast is expected — but mark-read fires regardless.
    await waitForVisibleInput(page);

    // Wait for the engine to receive POST /api/v1/notification/read and flip
    // the row. mark-read is fire-and-forget from the page, so poll the DB
    // directly — surfacing "Thread not found" in the UI shouldn't block this.
    // `psql -t` returns ` t` for true and ` f` for false (tuples-only mode,
    // leading whitespace from the column alignment); exact match avoids a
    // future text column accidentally matching the 't' substring.
    await expect.poll(
      () => {
        const row = psql(
          `SELECT read FROM notifications WHERE id = '${notificationId}'`,
        ).trim();
        return row === 't';
      },
      { intervals: [200, 500, 1000], timeout: 5000 },
    ).toBeTruthy();
  });
});
