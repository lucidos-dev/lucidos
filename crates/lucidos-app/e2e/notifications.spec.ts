import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import {
  apiRequest, assertHealthy, clickThreadRow, clickVisibleElement, ensureMobileView,
  expectPushSent, gotoWithRetry, navigateToApp, openThreadDrawer, waitForExchangeCount,
  waitForVisibleElement, waitForVisibleInput,
} from './helpers';
import { clearNotifications, psql } from './db-helpers';

/** Send a notification through the script-facing endpoint that powers the
 *  `lucidos notify` CLI. Same shape as `send_notification` from the chat LLM.
 *
 *  `tap` is the structured discriminated union, and the engine API enforces the
 *  SDK `Tap` shape. This helper takes `unknown` for the field so the suite can
 *  post a malformed payload the engine is meant to reject. */
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
  const res = await apiRequest(page).post('/api/v1/notifications', {
    headers: { 'content-type': 'application/json' },
    data: body,
  });
  expect(res.ok(), `POST /api/v1/notifications -> ${res.status()}`).toBeTruthy();
}

/** Seed a chat thread with `exchanges` message/response pairs. The USER
 *  messages carry the height: a seeded chat response body does not render,
 *  since response text comes from streamed events rather than a bare
 *  ResponseGenerated. Inserts straight into the event store and projection, so
 *  the test is deterministic with no LLM round-trips. Returns the thread id and
 *  the first and last MessageReceived event ids, which are the deep-link
 *  targets. The frontend loads the thread LAZILY, fetching events on focus,
 *  which is the path the deep-link bug bit. */
function seedTallChatThread(exchanges: number): { threadId: string; firstEventId: string; lastEventId: string } {
  const threadId = randomUUID();
  const base = Date.now();
  // ~1 KB of wrapped text per message, so a handful of exchanges push the first
  // one far above the fold.
  const longText = 'This is seeded message text used to make the thread tall. '.repeat(16);
  const stmts: string[] = [];
  let firstEventId = '';
  let lastEventId = '';
  let lastCreated = '';
  let k = 0;
  for (let i = 1; i <= exchanges; i++) {
    const msgId = randomUUID();
    if (i === 1) firstEventId = msgId;
    lastEventId = msgId; // ends as the LAST exchange's MessageReceived (near the bottom)
    const respId = randomUUID();
    const msgCreated = new Date(base + k++ * 1000).toISOString();
    const respCreated = new Date(base + k++ * 1000).toISOString();
    lastCreated = respCreated;
    const msgPayload = JSON.stringify({ text: `Message ${i}: ${longText}`, channel: 'chat' }).replace(/'/g, "''");
    const respPayload = JSON.stringify({ text: 'Response.', images: [] }).replace(/'/g, "''");
    stmts.push(
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgId}', 'MessageReceived', '${msgPayload}'::jsonb, '${msgCreated}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${respId}', 'ResponseGenerated', '${respPayload}'::jsonb, '${respCreated}', 'thread', '${threadId}', '${threadId}')`,
    );
  }
  stmts.unshift(
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Deep-link target thread', 'chat', '${lastCreated}', ${exchanges}, false, true, 'idle', 'inbox', 'active', false, 0, 0, false, false, false)`,
  );
  psql(stmts.join(';\n'));
  return { threadId, firstEventId, lastEventId };
}

test.describe('Notification detail does not auto-open', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test('NotificationCreated on foregrounded page leaves the detail panel closed', async ({ page }) => {
    await navigateToApp(page);
    // Bell only renders on the content pane on mobile (no-op on desktop).
    await ensureMobileView(page, 'content');
    await expect(page.locator('.notifications-bell:visible').first()).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.notification-detail-body')).toHaveCount(0);

    await postNotification(page, {
      title: 'Heads up',
      message: 'A new thing happened',
    });

    // The bell badge bumps, which proves the SSE landed. Without that the
    // detail-closed assertion below would be vacuous.
    await expect(page.locator('.notifications-bell:visible .badge').first()).toHaveText('1', { timeout: 5_000 });

    // Wait-then-check. Playwright's auto-retrying assertions pass on the first
    // poll the condition holds, so `toHaveCount(0, { timeout })` returns
    // instantly against an already-closed detail. Sleep, then assert once, so a
    // delayed opener inside the window is caught.
    await page.waitForTimeout(500);
    await expect(page.locator('.notification-detail-body')).toHaveCount(0);
  });

  test('NotificationCreated while notifications panel is open leaves the detail panel closed', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'content');

    // Switch to the notifications panel via the bell, the path the user takes.
    await clickVisibleElement(page, '.notifications-bell');
    await expect(
      page.locator('.empty-state:has-text("No"), .notification-item').first(),
    ).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.notification-detail-body')).toHaveCount(0);

    await postNotification(page, {
      title: 'Heads up 2',
      message: 'Another new thing',
    });

    // The list reloads (handleNotificationSSE → loadNotifications when panel
    // is active). The new row should appear, but the DETAIL must stay closed.
    await expect(
      page.locator('.notification-item:has-text("Heads up 2")').first(),
    ).toBeVisible({ timeout: 5_000 });

    // Same wait-then-check as scenario 1: auto-retry would pass instantly
    // against an already-zero count.
    await page.waitForTimeout(500);
    await expect(page.locator('.notification-detail-body')).toHaveCount(0);
  });
});

test.describe('Notification row: jump, or read the card', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test('the chevron opens the card even though it sits over the row button', async ({ page }) => {
    await navigateToApp(page);

    // A source event makes this a jumping row, which is the only kind that has
    // a chevron. The thread need not exist: nothing here follows the jump.
    await postNotification(page, {
      title: 'Chevron reaches the card',
      message: 'the row body jumps to the thread instead',
      thread_id: randomUUID(),
      event_id: randomUUID(),
    });

    await ensureMobileView(page, 'content');
    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);

    // A REAL click, not the synthetic `el.click()` the helpers dispatch. The
    // chevron is absolutely positioned over the row button, so only hit-testing
    // proves the tap reaches it rather than the row underneath.
    const chevron = page.locator('.notification-row-detail-btn:visible').first();
    await expect(chevron).toBeVisible({ timeout: 10_000 });
    await chevron.click();

    await waitForVisibleElement(page, '.notification-detail-body', 10_000);
    await expect(page.locator('.notification-detail-body:visible').first()).toContainText(
      'the row body jumps to the thread instead',
    );
  });
});

test.describe('Notifications infinite scroll', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test('scrolling the list loads older pages beyond the first', async ({ page }) => {
    // Seed more than one page (PAGE_SIZE = 15), with distinct and strictly
    // decreasing created_at timestamps. A same-instant tie at the page boundary
    // would let the `before` cursor skip or repeat a row.
    const TOTAL = 30;
    psql(
      `INSERT INTO notifications (id, title, message, read, created_at) ` +
        `SELECT gen_random_uuid(), 'Infinite scroll ' || g, 'body ' || g, false, ` +
        `NOW() - (g || ' seconds')::interval ` +
        `FROM generate_series(1, ${TOTAL}) AS g`,
    );

    await navigateToApp(page);
    await ensureMobileView(page, 'content');

    // Open the notifications panel via the bell, the path the user takes.
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

    // The first page renders exactly one PAGE_SIZE. The poll samples those 15
    // well before an async page-2 fetch could land. So this also proves the
    // list paginates instead of dumping all 30 at once.
    await expect.poll(visibleCount, { timeout: 10_000 }).toBe(15);

    // Scroll the REAL scroll container, `.content-pane-body`. The inner
    // `.panel-content` has no overflow and never scrolls, so a load-more
    // trigger bound there never fires. Scroll inside the poll so a multi-page
    // list keeps advancing each iteration.
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

test.describe('Notification deep-link to an event in an unfocused thread', () => {
  let seeded: { threadId: string; firstEventId: string; lastEventId: string } | null = null;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test.afterEach(() => {
    if (!seeded) return;
    psql([
      `DELETE FROM events WHERE aggregate_id = '${seeded.threadId}'`,
      `DELETE FROM thread_summaries WHERE thread_id = '${seeded.threadId}'`,
    ].join(';\n'));
    seeded = null;
  });

  test('Open thread from the notifications panel lands on the source event, not the thread bottom', async ({ page }) => {
    // Focusing an unfocused thread lazily loads its events, and the
    // scroll-to-bottom on the eventsLoaded transition used to override the
    // deep-link's scrollIntoView. So opening the thread landed on the bottom
    // rather than the linked event.
    seeded = seedTallChatThread(8);
    const { threadId, firstEventId } = seeded;

    // Load the app with NO thread focused, which is the broken path. An
    // already-focused thread has its events in the DOM, so its deep-link scroll
    // resolves synchronously.
    await navigateToApp(page);

    await postNotification(page, {
      title: 'Jump to first message',
      message: 'tap to open the source event',
      thread_id: threadId,
      event_id: firstEventId,
    });

    // The reported surface: the in-app notifications panel. The row honours the
    // notification's own tap. A notification naming a source event carries a
    // derived navigate tap, so one click lands in the thread.
    await ensureMobileView(page, 'content');
    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);
    await clickVisibleElement(page, '.notification-item', 'Jump to first message');

    // The thread lazy-loads; wait for its exchanges to render.
    await ensureMobileView(page, 'thread');
    await waitForExchangeCount(page, 8, 15_000);

    // Precondition: the seeded thread must overflow the viewport. A short one
    // shows the first event either way, so the assertion below could not tell
    // "scrolled to the event" from "scrolled to the bottom".
    const scrollable = await page.evaluate(() => {
      const els = document.querySelectorAll('.thread-content');
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) return el.scrollHeight > el.clientHeight + 100;
      }
      return false;
    });
    expect(scrollable, 'seeded thread must overflow the viewport for this test to discriminate').toBeTruthy();

    // The FIRST exchange, the deep-link target, is scrolled into view. Before
    // the fix the events-load scroll-to-bottom left it above the fold.
    await page.waitForFunction((eid) => {
      const els = document.querySelectorAll(`[data-event-id="${eid}"]`);
      const vh = window.innerHeight || document.documentElement.clientHeight;
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue; // skip the dual-layout hidden copy
        if (r.bottom > 0 && r.top < vh) return true; // visible somewhere in the viewport
      }
      return false;
    }, firstEventId, { timeout: 10_000 });
  });

  test('deep-link overrides a saved scroll position on an unfocused thread', async ({ page }) => {
    // Focusing an unfocused thread re-runs useScrollMemory, whose restore
    // observer is created AFTER scrollToEventAndPulse's. It therefore fired
    // last and snapped back to the saved offset, so a deep-link landed there
    // instead of on the source event. An already-focused thread does not re-run
    // useScrollMemory, which is why the scroll worked only in that case. All
    // four surfaces share one router (inbox row, in-app toast, web push, native
    // banner), and the inbox row reproduces it deterministically.
    seeded = seedTallChatThread(16);
    const { threadId, lastEventId } = seeded;

    await navigateToApp(page);

    // Pre-seed a saved scroll near the TOP. A POSITIVE offset is what exercises
    // the bug, since the observer-driven restore is what snaps back over the
    // deep-link's scroll. An offset of exactly 0 restores synchronously and
    // loses the race, so it does not reproduce.
    await page.evaluate((tid) => {
      localStorage.setItem(`lucidos-scroll-thread-${tid}`, '100');
    }, threadId);

    await postNotification(page, {
      title: 'Jump to last message',
      message: 'tap to open the source event near the bottom',
      thread_id: threadId,
      event_id: lastEventId,
    });

    await ensureMobileView(page, 'content');
    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);
    await clickVisibleElement(page, '.notification-item', 'Jump to last message');

    await ensureMobileView(page, 'thread');
    await waitForExchangeCount(page, 16, 15_000);

    // The deep-link target (last exchange) is scrolled into view, NOT the
    // restored saved offset near the top.
    await page.waitForFunction((eid) => {
      const els = document.querySelectorAll(`[data-event-id="${eid}"]`);
      const vh = window.innerHeight || document.documentElement.clientHeight;
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue; // skip the dual-layout hidden copy
        if (r.bottom > 0 && r.top < vh) return true; // visible somewhere in the viewport
      }
      return false;
    }, lastEventId, { timeout: 10_000 });
  });
});

/** One short exchange, which is what the seen rule needs from a fixture.
 *
 *  `seedTallChatThread` is deliberately taller than the viewport. The *standing
 *  follow* seed ships armed, so such a thread opens on its live edge with the
 *  early cards above the fold. This rule measures the CARD, so the fixture has
 *  to fit whole on a phone. Returns the thread and its only `MessageReceived`,
 *  which is the exchange's stamped id. */
function seedShortChatThread(): { threadId: string; eventId: string } {
  const threadId = randomUUID();
  const eventId = randomUUID();
  const respId = randomUUID();
  const base = Date.now();
  const msgCreated = new Date(base).toISOString();
  const respCreated = new Date(base + 1000).toISOString();
  const msgPayload = JSON.stringify({ text: 'Short seeded message.', channel: 'chat' });
  const respPayload = JSON.stringify({ text: 'Response.', images: [] });
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Seen target thread', 'chat', '${respCreated}', 1, false, true, 'idle', 'inbox', 'active', false, 0, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${eventId}', 'MessageReceived', '${msgPayload}'::jsonb, '${msgCreated}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${respId}', 'ResponseGenerated', '${respPayload}'::jsonb, '${respCreated}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));
  return { threadId, eventId };
}

test.describe('A notification clears once its event card has been seen', () => {
  let seeded: { threadId: string } | null = null;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test.afterEach(() => {
    if (!seeded) return;
    psql([
      `DELETE FROM events WHERE aggregate_id = '${seeded.threadId}'`,
      `DELETE FROM thread_summaries WHERE thread_id = '${seeded.threadId}'`,
    ].join(';\n'));
    seeded = null;
  });

  test('reading the thread from the drawer drops the bell badge, with the panel never opened', async ({ page }) => {
    const short = seedShortChatThread();
    seeded = short;
    const { threadId, eventId } = short;

    await navigateToApp(page);
    // The bell first: it proves the page is up and subscribed. Posting before
    // the SSE stream is open loses the NotificationCreated, so the badge below
    // never bumps. That is a race rather than a verdict.
    await ensureMobileView(page, 'content');
    await expect(page.locator('.notifications-bell:visible').first())
      .toBeVisible({ timeout: 10_000 });

    await postNotification(page, {
      title: 'Seen target',
      message: 'cleared by reading the thread it points into',
      thread_id: threadId,
      event_id: eventId,
    });

    // Precondition: the badge is up. Without it the assertion below is vacuous.
    await expect(page.locator('.notifications-bell:visible .badge').first())
      .toHaveText('1', { timeout: 10_000 });

    // Arrive the way the user does. The Notifications panel is never opened,
    // the row is never tapped, and no deep link is dispatched.
    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureMobileView(page, 'thread');
    await waitForExchangeCount(page, 1, 15_000);

    // The card really is on screen, which is what the rule measures.
    await page.waitForFunction((eid) => {
      const els = document.querySelectorAll(`[data-event-id="${eid}"]`);
      const vh = window.innerHeight || document.documentElement.clientHeight;
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue; // skip the dual-layout hidden copy
        if (r.bottom > 0 && r.top < vh) return true;
      }
      return false;
    }, eventId, { timeout: 10_000 });

    // Hold the pane past SEEN_DWELL_MS. Swiping away earlier would cancel the
    // wait, which is the anti-glimpse rule doing its job rather than a flake.
    await page.waitForTimeout(2_000);

    // The bell lives on the content pane on mobile, so read it there.
    await ensureMobileView(page, 'content');
    await expect(page.locator('.notifications-bell:visible .badge'))
      .toHaveCount(0, { timeout: 10_000 });
  });
});

test.describe('Declarative Web Push payload', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test('push payload is the Declarative Web Push envelope with absolute iOS navigate URL', async ({ page, baseURL }) => {
    // Safari 18.5+ handles a push declaratively, bypassing the SW push handler,
    // only when the on-wire payload conforms to the declarative envelope. A
    // flat `{title, body, ...}` payload falls back to legacy SW dispatch, which
    // is the iOS-PWA push-tap regression this guards.
    //
    // Playwright cannot drive a real OS tap, so the on-wire shape is the whole
    // assertion: Safari does the rest if the envelope is right. The page-side
    // dispatcher (handleHashLocation, dispatchDeepLink) is exercised below via
    // the same URL Safari lands on.

    // Register a synthetic device and push subscription so the fan-out has a
    // target and writes to push_log. The push_test_log path skips a
    // subscription with no device_id, so the device row is inserted first.
    //
    // Do NOT navigate to the app before this: `navigateToApp` registers THIS
    // browser as a device and starts a presence heartbeat, and PresenceCheck
    // then suppresses the push, masking the payload-shape assertion. Hit the
    // API directly until the assertions are recorded.
    const deviceId = `e2e-declarative-${Date.now()}`;
    psql(
      `INSERT INTO devices (id, name, user_agent, push_enabled) ` +
        `VALUES ('${deviceId}', 'e2e-declarative', 'e2e-test-ua', true)`,
    );
    // Clear device_presence so no stale row from an earlier test counts as an
    // active candidate and suppresses the push. Clearing the whole table is in
    // scope here: Playwright projects run serially against one workspace DB.
    psql(`DELETE FROM device_presence`);
    expect(baseURL, 'Playwright baseURL is needed to seed the subscription scope').toBeTruthy();
    const scopeUrl = new URL('/', baseURL!).toString();
    const subRes = await apiRequest(page).post('/api/v1/push/subscribe', {
      headers: { 'content-type': 'application/json' },
      data: {
        endpoint: `https://push.test/synthetic/${Date.now()}`,
        p256dh: 'p256dh-test',
        auth: 'auth-test',
        device_id: deviceId,
        scope_url: scopeUrl,
      },
    });
    expect(subRes.ok(), `POST /api/v1/push/subscribe -> ${subRes.status()}`).toBeTruthy();

    // A UUID-formatted but synthetic thread/event id. focusThreadOrBootstrap
    // surfaces a "Thread not found" toast, which is fine: the engine-side
    // payload shape and the page-side mark-read dispatch both fire whether or
    // not the target resolves to a thread row.
    const fakeThreadId = '00000000-0000-4000-8000-000000000001';
    const fakeEventId = '00000000-0000-4000-8000-000000000002';

    const res = await apiRequest(page).post('/api/v1/notifications', {
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

    // PresenceCheck has zero candidates (device_presence cleared above), so the
    // push fan-out is immediate, with no deadline to wait through.
    const entry = await expectPushSent(page, notificationId, { timeoutMs: 5000 });
    expect(entry.payload, 'push_log row must carry the recorded payload bytes').toBeTruthy();
    const payload = JSON.parse(entry.payload!) as Record<string, unknown>;

    // (1) Declarative envelope: top-level `web_push: 8030` magic plus a
    //     `notification` object. Safari 18.5+ keys off these to bypass the SW.
    expect(payload.web_push).toBe(8030);
    expect(payload.notification).toBeTruthy();
    const notif = payload.notification as Record<string, unknown>;
    expect(notif.title).toBe('Claude is asking');
    expect(notif.body).toBe('tap me');

    // (2) Tag stamped from notification_id, so OS-level dedup matches the SW's
    //     own `tag: data.notification_id`.
    expect(notif.tag).toBe(notificationId);

    // (3) iOS navigate URL: a CROSS-DOCUMENT absolute query URL built from the
    //     subscription's stored service-worker scope. Safari's declarative-push
    //     handler reuses an already-open PWA window on tap, and a hash-only
    //     navigation is not applied to it. WebKit focuses the window, the URL
    //     never updates, and the deep link no-ops. A query string forces a real
    //     navigation; making it absolute avoids relying on WebKit or APNs to
    //     accept a query-only relative value.
    const navigateUrl = notif.navigate as string;
    expect(navigateUrl.startsWith(`${scopeUrl}?`), `iOS navigate must be an absolute scoped query URL, got ${navigateUrl}`).toBeTruthy();
    expect(navigateUrl).toContain(`notification=${notificationId}`);
    expect(navigateUrl).toContain(`thread=${fakeThreadId}`);
    expect(navigateUrl).toContain(`event=${fakeEventId}`);
    expect(navigateUrl).toContain('tap=');

    // (4) data.* carries the flat fields Chrome's notificationclick reads off
    //     event.notification.data. data.navigate is the HASH form (a warm
    //     `client.navigate()`, so no reload), carrying the SAME params as the
    //     iOS query URL: only the prefix differs, `?` against `#`.
    const data = notif.data as Record<string, unknown>;
    expect(data.notification_id).toBe(notificationId);
    expect(data.thread_id).toBe(fakeThreadId);
    expect(data.event_id).toBe(fakeEventId);
    const swNavigate = data.navigate as string;
    expect(swNavigate.startsWith('#'), `Chrome SW navigate must be a scope-relative hash URL, got ${swNavigate}`).toBeTruthy();
    expect(swNavigate.slice(1)).toBe(new URL(navigateUrl).search.slice(1)); // same params, different URL carrier
    expect(data.tap).toEqual({
      kind: 'navigate',
      to: { target: 'thread', id: fakeThreadId, event_id: fakeEventId },
    });

    // (5) Page-side dispatch contract. When iOS navigates the PWA window to the
    //     query navigate URL, the cold-start hash router runs
    //     handleHashLocation then dispatchDeepLink. That reads the query params
    //     and calls markReadOptimistic. Drive the exact URL Safari would land
    //     on. Navigation waited until now so the test browser did not count as
    //     an active device during the PresenceCheck above.
    await gotoWithRetry(page, navigateUrl);
    // Confirm the SPA actually mounted (so useStartup's cold-start hash router
    // runs and reads the query params). The deep-link targets a fake thread, so
    // a "Thread not found" toast is expected, and mark-read fires regardless.
    await waitForVisibleInput(page);

    // Wait for the engine to take POST /api/v1/notification/read and flip the
    // row. mark-read is fire-and-forget from the page, so poll the DB directly.
    // `psql -t` returns ` t` for true and ` f` for false, and the exact match
    // keeps a future text column from matching the 't' substring.
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
