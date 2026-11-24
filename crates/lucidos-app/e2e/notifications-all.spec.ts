import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import {
  apiRequest, assertHealthy, clickThreadRow, clickVisibleElement, ensureMobileView,
  expectPushSent, gotoWithRetry, navigateToApp, openThreadDrawer, waitForExchangeCount,
  waitForEventStream, waitForVisibleElement, waitForVisibleInput,
} from './helpers';
import { clearNotifications, psql } from './db-helpers';

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

function seedThreadEvent(): { threadId: string; eventId: string } {
  const threadId = randomUUID();
  const eventId = randomUUID();
  psql(
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${eventId}', 'UserQuestionAsked', '{}'::jsonb, NOW(), 'thread', '${threadId}', '${threadId}')`,
  );
  return { threadId, eventId };
}

function seedTallChatThread(exchanges: number): { threadId: string; firstEventId: string; lastEventId: string } {
  const threadId = randomUUID();
  const base = Date.now();
  const longText = 'This is seeded message text used to make the thread tall. '.repeat(16);
  const stmts: string[] = [];
  let firstEventId = '';
  let lastEventId = '';
  let lastCreated = '';
  let k = 0;
  for (let i = 1; i <= exchanges; i++) {
    const msgId = randomUUID();
    if (i === 1) firstEventId = msgId;
    lastEventId = msgId;
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
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Deep-link target thread', 'chat', '${lastCreated}', ${exchanges}, false, true, 'idle', 'inbox', 'active', false, 0, 0, 'none', false)`,
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
    await ensureMobileView(page, 'content');
    await expect(page.locator('.notifications-bell:visible').first()).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.notification-detail-body')).toHaveCount(0);
    await waitForEventStream(page);

    await postNotification(page, {
      title: 'Heads up',
      message: 'A new thing happened',
    });

    await expect(page.locator('.notifications-bell:visible .badge').first()).toHaveText('1', { timeout: 5_000 });

    await page.waitForTimeout(500);
    await expect(page.locator('.notification-detail-body')).toHaveCount(0);
  });

  test('NotificationCreated while notifications panel is open leaves the detail panel closed', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'content');

    await clickVisibleElement(page, '.notifications-bell');
    await expect(
      page.locator('.empty-state:has-text("No"), .notification-item').first(),
    ).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.notification-detail-body')).toHaveCount(0);

    await postNotification(page, {
      title: 'Heads up 2',
      message: 'Another new thing',
    });

    await expect(
      page.locator('.notification-item:has-text("Heads up 2")').first(),
    ).toBeVisible({ timeout: 5_000 });

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
    const source = seedThreadEvent();
    await postNotification(page, {
      title: 'Chevron reaches the card',
      message: 'the row body jumps to the thread instead',
      thread_id: source.threadId,
      event_id: source.eventId,
    });
    await navigateToApp(page);

    await ensureMobileView(page, 'content');
    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);

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
    const TOTAL = 30;
    psql(
      `INSERT INTO notifications (id, title, message, read, created_at) ` +
        `SELECT gen_random_uuid(), 'Infinite scroll ' || g, 'body ' || g, false, ` +
        `NOW() - (g || ' seconds')::interval ` +
        `FROM generate_series(1, ${TOTAL}) AS g`,
    );

    await navigateToApp(page);
    await ensureMobileView(page, 'content');

    await clickVisibleElement(page, '.notifications-bell');

    const visibleCount = () =>
      page.evaluate(() => {
        const els = document.querySelectorAll('.notification-item');
        return Array.from(els).filter((el) => {
          const r = el.getBoundingClientRect();
          return r.width > 0 && r.height > 0;
        }).length;
      });

    await expect.poll(visibleCount, { timeout: 10_000 }).toBe(15);

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
    seeded = seedTallChatThread(8);
    const { threadId, firstEventId } = seeded;

    await navigateToApp(page);

    await postNotification(page, {
      title: 'Jump to first message',
      message: 'tap to open the source event',
      thread_id: threadId,
      event_id: firstEventId,
    });

    await ensureMobileView(page, 'content');
    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);
    await clickVisibleElement(page, '.notification-item', 'Jump to first message');

    await ensureMobileView(page, 'thread');
    await waitForExchangeCount(page, 8, 15_000);

    const scrollable = await page.evaluate(() => {
      const els = document.querySelectorAll('.thread-content');
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) return el.scrollHeight > el.clientHeight + 100;
      }
      return false;
    });
    expect(scrollable, 'seeded thread must overflow the viewport for this test to discriminate').toBeTruthy();

    await page.waitForFunction((eid) => {
      const els = document.querySelectorAll(`[data-event-id="${eid}"]`);
      const vh = window.innerHeight || document.documentElement.clientHeight;
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue;
        if (r.bottom > 0 && r.top < vh) return true;
      }
      return false;
    }, firstEventId, { timeout: 10_000 });
  });

  test('deep-link overrides a saved scroll position on an unfocused thread', async ({ page }) => {
    seeded = seedTallChatThread(16);
    const { threadId, lastEventId } = seeded;

    await navigateToApp(page);

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

    await page.waitForFunction((eid) => {
      const els = document.querySelectorAll(`[data-event-id="${eid}"]`);
      const vh = window.innerHeight || document.documentElement.clientHeight;
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue;
        if (r.bottom > 0 && r.top < vh) return true;
      }
      return false;
    }, lastEventId, { timeout: 10_000 });
  });
});

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
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Seen target thread', 'chat', '${respCreated}', 1, false, true, 'idle', 'inbox', 'active', false, 0, 0, 'none', false)`,
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

  test('reading the thread from the drawer drops the bell badge and the toast, with the panel never opened', async ({ page }) => {
    const short = seedShortChatThread();
    seeded = short;
    const { threadId, eventId } = short;

    await navigateToApp(page);
    await ensureMobileView(page, 'content');
    await expect(page.locator('.notifications-bell:visible').first())
      .toBeVisible({ timeout: 10_000 });
    await waitForEventStream(page);

    await postNotification(page, {
      title: 'Seen target',
      message: 'cleared by reading the thread it points into',
      thread_id: threadId,
      event_id: eventId,
    });

    await expect(page.locator('.notifications-bell:visible .badge').first())
      .toHaveText('1', { timeout: 10_000 });
    const toast = page.locator('.toast-container .toast', { hasText: 'Seen target' });
    await expect(toast).toHaveCount(1, { timeout: 10_000 });

    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureMobileView(page, 'thread');
    await waitForExchangeCount(page, 1, 15_000);

    await page.waitForFunction((eid) => {
      const els = document.querySelectorAll(`[data-event-id="${eid}"]`);
      const vh = window.innerHeight || document.documentElement.clientHeight;
      for (const el of els) {
        const r = el.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) continue;
        if (r.bottom > 0 && r.top < vh) return true;
      }
      return false;
    }, eventId, { timeout: 10_000 });

    await page.waitForTimeout(2_000);

    await expect(toast).toHaveCount(0, { timeout: 10_000 });

    await ensureMobileView(page, 'content');
    await expect(page.locator('.notifications-bell:visible .badge'))
      .toHaveCount(0, { timeout: 10_000 });
  });
});

test.describe('A notification that lands where the reader already is', () => {
  let seeded: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test.afterEach(async ({ page }) => {
    await putNotificationToasts(page, 'true');
    for (const threadId of seeded) {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
    seeded = [];
  });

  function seedThreads(count: number): string[] {
    for (let i = 0; i < count; i++) seeded.push(seedShortChatThread().threadId);
    return [...seeded];
  }

  async function openThread(page: Page, threadId: string): Promise<void> {
    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureMobileView(page, 'thread');
    await waitForExchangeCount(page, 1, 15_000);
  }

  async function putNotificationToasts(page: Page, value: 'true' | 'false'): Promise<void> {
    const res = await apiRequest(page).put('/api/v1/preferences?key=notification_toasts', { data: { value } });
    expect(res.ok(), `notification_toasts=${value}`).toBe(true);
  }

  async function switchNotificationToastsLive(page: Page, value: 'true' | 'false'): Promise<void> {
    await putNotificationToasts(page, value);
    await page.waitForFunction(
      (v) => localStorage.getItem('lucidos-notification-toasts') === v,
      value,
      { timeout: 10_000 },
    );
  }

  async function notifyFromThread(page: Page, threadId: string, title: string): Promise<void> {
    const landed = page.waitForResponse(async (res) =>
      res.url().includes('/api/v1/notifications') && res.url().includes('filter=unread')
        && (await res.text()).includes(title));
    await postNotification(page, {
      title,
      message: 'sent from the thread the reader has open',
      thread_id: threadId,
      tap: { kind: 'navigate', to: { target: 'thread', id: threadId } },
    });
    await landed;
  }

  function readState(title: string): string {
    return psql(`SELECT read FROM notifications WHERE title = '${title}'`);
  }

  test('the toast shows and stays, and the row stays unread', async ({ page }) => {
    const [here] = seedThreads(1);
    await navigateToApp(page);
    await openThread(page, here);
    await waitForEventStream(page);

    await notifyFromThread(page, here, 'Landed here');
    const toast = page.locator('.toast-container .toast', { hasText: 'Landed here' });
    await expect(toast).toHaveCount(1, { timeout: 10_000 });

    await page.waitForTimeout(2_000);
    await expect(toast).toHaveCount(1);
    expect(readState('Landed here')).toBe('f');
  });

  test('with toasts switched off live, nothing pops and the row stays unread until a later visit', async ({ page }) => {
    const [here, elsewhere] = seedThreads(2);
    await navigateToApp(page);
    await openThread(page, here);
    await waitForEventStream(page);
    await switchNotificationToastsLive(page, 'false');

    await notifyFromThread(page, here, 'Quiet here');
    await page.waitForTimeout(2_000);
    await expect(page.locator('.toast-container .toast', { hasText: 'Quiet here' })).toHaveCount(0);
    expect(readState('Quiet here')).toBe('f');

    await openThread(page, elsewhere);
    await openThread(page, here);
    await expect.poll(() => readState('Quiet here'), { timeout: 10_000 }).toBe('t');
  });
});

test.describe('Declarative Web Push payload', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    clearNotifications();
  });

  test('push payload is the Declarative Web Push envelope with absolute iOS navigate URL', async ({ page, baseURL }) => {

    const deviceId = `e2e-declarative-${Date.now()}`;
    psql(
      `INSERT INTO devices (id, name, user_agent, push_enabled) ` +
        `VALUES ('${deviceId}', 'e2e-declarative', 'e2e-test-ua', true)`,
    );
    psql(`DELETE FROM device_presence`);
    expect(baseURL, 'Playwright baseURL is needed to seed the subscription scope').toBeTruthy();
    const scopeUrl = new URL('/', baseURL!).toString();
    const subRes = await apiRequest(page).post('/api/v1/push/subscribe', {
      headers: { 'content-type': 'application/json', 'x-lucidos-device-id': deviceId },
      data: {
        endpoint: `https://push.test/synthetic/${Date.now()}`,
        p256dh: 'p256dh-test',
        auth: 'auth-test',
        device_id: deviceId,
        scope_url: scopeUrl,
      },
    });
    expect(subRes.ok(), `POST /api/v1/push/subscribe -> ${subRes.status()}`).toBeTruthy();
    expect(await subRes.json(), 'push subscribe must be stored').toMatchObject({ success: true });

    const { threadId: fakeThreadId, eventId: fakeEventId } = seedThreadEvent();

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

    const entry = await expectPushSent(page, notificationId, { timeoutMs: 5000 });
    expect(entry.payload, 'push_log row must carry the recorded payload bytes').toBeTruthy();
    const payload = JSON.parse(entry.payload!) as Record<string, unknown>;

    expect(payload.web_push).toBe(8030);
    expect(payload.notification).toBeTruthy();
    const notif = payload.notification as Record<string, unknown>;
    expect(notif.title).toBe('Claude is asking');
    expect(notif.body).toBe('tap me');

    expect(notif.tag).toBe(notificationId);

    const navigateUrl = notif.navigate as string;
    expect(navigateUrl.startsWith(`${scopeUrl}?`), `iOS navigate must be an absolute scoped query URL, got ${navigateUrl}`).toBeTruthy();
    expect(navigateUrl).toContain(`notification=${notificationId}`);
    expect(navigateUrl).toContain(`thread=${fakeThreadId}`);
    expect(navigateUrl).toContain(`event=${fakeEventId}`);
    expect(navigateUrl).toContain('tap=');

    const data = notif.data as Record<string, unknown>;
    expect(data.notification_id).toBe(notificationId);
    expect(data.thread_id).toBe(fakeThreadId);
    expect(data.event_id).toBe(fakeEventId);
    const swNavigate = data.navigate as string;
    expect(swNavigate.startsWith('#'), `Chrome SW navigate must be a scope-relative hash URL, got ${swNavigate}`).toBeTruthy();
    expect(swNavigate.slice(1)).toBe(new URL(navigateUrl).search.slice(1));
    expect(data.tap).toEqual({
      kind: 'navigate',
      to: { target: 'thread', id: fakeThreadId, event_id: fakeEventId },
    });

    await gotoWithRetry(page, navigateUrl);
    await waitForVisibleInput(page);

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
