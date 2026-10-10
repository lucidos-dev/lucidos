import { randomUUID } from 'crypto';
import { test, expect, type Page } from './fixtures';
import { apiRequest, assertHealthy, clickThreadRow, navigateToApp, newThread, openThreadDrawer } from './helpers';
import { ensureHomeThread, psql } from './db-helpers';

/**
 * `lucidos.ui.focusedThread()` and `onFocusedThreadChange()` from a Home widget
 * window floating over another thread. The window's widget belongs to Home, so
 * the value must follow the thread pane and never name Home.
 *
 * The widget re-reads the getter on every push, so each step checks both the
 * bridge answer and the push.
 */

const WIDGET_HTML = `<!doctype html><html><head><script src="/api/v1/sdk.js"></script></head><body>
<p>read <output id="read">none</output></p>
<p>pushed <output id="pushed">none</output></p>
<p>log <output id="log"></output></p>
<script>
  const text = (id) => id === null ? 'null' : id;
  const read = () => lucidos.ui.focusedThread().then(
    (id) => { document.querySelector('#read').textContent = text(id); },
    (err) => { document.querySelector('#read').textContent = 'error: ' + err.message; },
  );
  const pushes = [];
  lucidos.ui.onFocusedThreadChange((id) => {
    pushes.push(text(id));
    document.querySelector('#pushed').textContent = text(id);
    document.querySelector('#log').textContent = pushes.join(',');
    read();
  });
  read();
</script></body></html>`;

let written: string[] = [];
let seededThreads: string[] = [];
let appId = '';

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  written.push(path);
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, { headers: { 'Content-Type': 'text/plain' }, data: body });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

function seedThread(threadId: string, title: string, now: string): string[] {
  seededThreads.push(threadId);
  return [
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
      + `VALUES ('${threadId}', '${title}', 'chat', '${now}', 1, false, true, 'idle', 'inbox', false, 0, 'none', false)`,
    event('MessageReceived', threadId, `{"text":"${title}","mode":"human","channel":"chat"}`, now),
  ];
}

function event(type: string, threadId: string, payload: string, now: string): string {
  return `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
    + `VALUES ('${randomUUID()}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;
}

test.describe('The focused thread in the SDK', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    written = [];
    seededThreads = [];
    appId = `e2e-focused-thread-${randomUUID().slice(0, 8)}`;
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    const threads = seededThreads.map((id) => `'${id}'`).join(',');
    psql([
      `DELETE FROM events WHERE thread_id IN (${threads})`,
      `DELETE FROM thread_summaries WHERE thread_id IN (${threads})`,
      `DELETE FROM events WHERE event_type IN ('WidgetShown', 'WidgetPinned') AND payload->>'app_id' = '${appId}'`,
    ].join(';\n'));
    for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
  });

  test('a widget window follows the thread pane, not its own thread', async ({ page }) => {
    const home = ensureHomeThread();
    const threadA = randomUUID();
    const threadB = randomUUID();
    const now = new Date().toISOString();

    await writeFile(page, `apps/${appId}/index.html`, WIDGET_HTML);
    await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({ name: 'E2E focused thread', kind: 'widget', origin_thread_id: home }));
    psql([
      ...seedThread(threadA, 'E2E focus A', now),
      ...seedThread(threadB, 'E2E focus B', now),
      event('WidgetShown', home, `{"app_id":"${appId}"}`, now),
      event('WidgetPinned', home, `{"app_id":"${appId}"}`, now),
    ].join(';\n'));

    // Thread A open, with Home's widget already floating in a window over it.
    await page.evaluate(({ tid, homeId, key }) => {
      localStorage.setItem('lucidos-focused-thread', tid);
      localStorage.setItem('lucidos-widget-windows', JSON.stringify([{ threadId: homeId, instanceKey: key, x: 8, y: 120, raise: 1 }]));
    }, { tid: threadA, homeId: home, key: `${appId}?{}` });
    await navigateToApp(page);

    const frameSelector = `[data-widget-window^="${appId}?"] iframe[data-role="app-ui-frame"]`;
    await expect(page.locator(frameSelector)).toHaveAttribute('src', new RegExp(`/app/${appId}/`), { timeout: 15_000 });
    const widget = page.frameLocator(frameSelector);
    const read = widget.locator('#read');
    const pushed = widget.locator('#pushed');

    await expect(read).toHaveText(threadA, { timeout: 15_000 });
    await expect(pushed).toHaveText('none');

    await openThreadDrawer(page);
    await clickThreadRow(page, threadB);
    await expect(pushed).toHaveText(threadB);
    await expect(read).toHaveText(threadB);

    await newThread(page);
    await expect(pushed).toHaveText('null');
    await expect(read).toHaveText('null');
    await expect(widget.locator('#log')).toHaveText(`${threadB},null`);
  });
});
