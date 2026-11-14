import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { apiRequest, assertHealthy, hideDashboardRail, navigateToApp, setDevicePreference, waitForEventStream } from './helpers';
import { psql } from './db-helpers';

/**
 * The widget fit check (`docs/plans/2026-11-10-widget-fit-check.md`). The open
 * Lucidos window renders a widget at every size it declares, on the desk tile
 * and the phone column. The engine tells the agent what is cut off.
 *
 * The fixture is the Trip countdown the Demo Director rig's agent built: flight
 * rows that never wrap and end in an ellipsis. The phone column at 125% UI scale
 * is where its large box cut them off. The fixed widget wraps and shows less,
 * so it fits every box.
 */

const SCALE = '125';

const widgetHtml = (style: string, body: string) => '<!doctype html><html><head>'
  + '<script src="/api/v1/sdk-prefs.js"></script><link rel="stylesheet" href="/api/v1/sdk-iframe.css">'
  + `<script src="/api/v1/sdk.js"></script><style>${style}</style></head><body>${body}</body></html>`;

const NOWRAP_COUNTDOWN = widgetHtml(
  'body{margin:0;padding:0.75rem}h2{margin:0 0 0.5rem}'
    + '.flight{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}',
  '<h2>Stockholm in 12 days</h2>'
    + '<div class="flight">SK 1467 Oslo Gardermoen to Stockholm Arlanda, departs 07:15 from gate C12</div>'
    + '<div class="flight">SK 1484 Stockholm Arlanda to Oslo Gardermoen, departs 18:40 from gate F34</div>',
);

const WRAPPING_COUNTDOWN = widgetHtml(
  'body{margin:0;padding:0.5rem}h2{margin:0 0 0.25rem}p{margin:0}',
  '<h2>12 days</h2><p>SK 1467 07:15</p><p>SK 1484 18:40</p>',
);

let written: string[] = [];

async function writeFile(page: Page, path: string, body: string): Promise<void> {
  written.push(path);
  const resp = await apiRequest(page).put(`/api/v1/data/${path}`, {
    headers: { 'Content-Type': 'text/plain' },
    data: body,
  });
  expect(resp.ok(), `PUT ${path}`).toBeTruthy();
}

function seedThread(threadId: string): void {
  const now = new Date().toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
      + `VALUES ('${threadId}', 'E2E widget fit check', 'chat', '${now}', 1, false, true, 'idle', 'inbox', false, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) `
      + `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"count down to my trip","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));
}

/** The note `widgets fit` answers, through the HTTP op the CLI calls. */
async function fit(page: Page, appId: string, threadId: string): Promise<string> {
  const resp = await apiRequest(page).post('/api/v1/widgets/fit', { data: { app_id: appId, thread_id: threadId } });
  expect(resp.ok(), `fit: ${await resp.text()}`).toBeTruthy();
  return (await resp.json() as { note: string }).note;
}

test.describe('The widget fit check', () => {
  let threadId: string;
  let appId: string;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    written = [];
    threadId = randomUUID();
    appId = `e2e-trip-${randomUUID().slice(0, 8)}`;
    // The check renders at the largest scale any device holds, so a scale left
    // by another spec would change the boxes it measures.
    psql(`DELETE FROM preferences WHERE key = 'ui-scale'`);
    seedThread(threadId);
    await hideDashboardRail(page);
    await page.goto('/');
  });

  test.afterEach(async ({ page }) => {
    await setDevicePreference(page, 'ui-scale', '100');
    psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
    for (const path of written) await apiRequest(page).delete(`/api/v1/data/${path}`);
  });

  test('names the flight row the phone column cuts off at large, and passes the widget that wraps', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);
    await setDevicePreference(page, 'ui-scale', SCALE);
    await writeFile(page, `apps/${appId}/manifest.json`, JSON.stringify({
      name: 'Trip countdown', description: 'Days to Stockholm and the flights', kind: 'widget',
      origin_thread_id: threadId, sizes: ['small', 'medium', 'large'],
    }));
    await writeFile(page, `apps/${appId}/index.html`, NOWRAP_COUNTDOWN);

    const cut = await fit(page, appId, threadId);
    expect(cut, cut).toContain(`[WIDGET FIT] ${appId}:`);
    expect(cut, cut).toContain(`${SCALE}% UI scale`);
    const findings = cut.split('\n').filter((line) => line.startsWith('- '));
    const largePhone = findings.find((line) => line.includes('large') && line.includes('phone column'));
    expect(largePhone, cut).toBeDefined();
    expect(largePhone, cut).toContain('ellipsis');
    expect(largePhone, cut).toContain('SK 1467');

    await writeFile(page, `apps/${appId}/index.html`, WRAPPING_COUNTDOWN);
    const fits = await fit(page, appId, threadId);
    expect(fits, fits).toContain(`[WIDGET FIT] ${appId}: fits`);
    for (const size of ['small', 'medium', 'large']) expect(fits, fits).toContain(size);
    expect(fits, fits).toContain('desk tile');
    expect(fits, fits).toContain('phone column');
    expect(fits.split('\n').filter((line) => line.startsWith('- ')), fits).toEqual([]);
  });
});
