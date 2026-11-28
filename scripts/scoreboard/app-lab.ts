import { randomUUID } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium, webkit, type Request } from '@playwright/test';
import { getBaseUrl } from '../../crates/lucidos-app/e2e/address';
import { psql, seedChatThread } from '../../crates/lucidos-app/e2e/db-helpers';
import { openThreadDrawer } from '../../crates/lucidos-app/e2e/helpers';
import { stampPageRequests } from '../../crates/lucidos-app/e2e/harnessStamp';
import { appMark, SAMPLE_COUNT } from './lab.mjs';

const TURNS = 600;

function seed(file: string) {
  const target = seedChatThread({ turns: TURNS, needles: [], title: 'Lab transcript' });
  const origin = seedChatThread({ turns: 1, needles: [], title: 'Lab navigation start' });
  psql(`UPDATE thread_summaries SET archive_state = 'inbox' WHERE thread_id IN ('${target}', '${origin}')`);
  const middle = psql(`SELECT id FROM events WHERE thread_id = '${target}' AND event_type = 'MessageReceived'
    ORDER BY sequence OFFSET ${Math.floor(TURNS / 2)} LIMIT 1`);
  const events = Number(psql(`SELECT count(*) FROM events WHERE thread_id = '${target}'`));
  if (events !== TURNS * 3 || !middle) throw new Error('The synthetic lab thread did not seed correctly');
  const tap = JSON.stringify({ kind: 'navigate', to: { target: 'thread', id: target, event_id: middle } });
  const notifications = Array.from({ length: SAMPLE_COUNT }, () => randomUUID());
  psql(`INSERT INTO notifications (id, thread_id, event_id, title, message, tap) VALUES
    ${notifications.map(id => `('${id}', '${target}', '${middle}', 'Lab notification', 'Open the synthetic transcript', '${tap}')`).join(', ')}`);
  writeFileSync(file, JSON.stringify({ target, origin, middle, events, notifications, tap }));
}

function fromRequest(request: Request, journey: string, target: string): number | undefined {
  if (request.method() !== 'POST' || !new URL(request.url()).pathname.endsWith('/internal/client-logs')) return;
  const batch = request.postDataJSON();
  if (!Array.isArray(batch)) return;
  return batch.map(entry => appMark(journey, entry, target)).find(value => Number.isFinite(value));
}

async function measure(browserName: string, journey: string, fixture: string, output: string, sampleIndex: string) {
  const { target, origin, middle, notifications, tap } = JSON.parse(readFileSync(fixture, 'utf8'));
  const engine = browserName === 'webkit' ? webkit : chromium;
  const browser = await engine.launch(browserName === 'webkit' ? {
    proxy: { server: 'http://127.0.0.1:1', bypass: 'localhost,127.0.0.1,::1' },
  } : {});
  try {
    const context = await browser.newContext({
      baseURL: getBaseUrl(),
      ignoreHTTPSErrors: true,
      viewport: { width: 1280, height: 800 },
      reducedMotion: 'reduce',
      serviceWorkers: 'block',
    });
    await stampPageRequests(context);
    const page = await context.newPage();
    if (journey === 'thread-navigation') {
      await page.goto('/');
      await openThreadDrawer(page);
      await page.locator(`.thread-row[data-thread-nav="${origin}"]:visible`).click();
      await page.getByText('Answer 0.', { exact: true }).waitFor();
      await openThreadDrawer(page);
    }
    const sample = page.waitForRequest(
      request => fromRequest(request, journey, target) !== undefined,
      { timeout: 30_000 },
    );
    const url = journey === 'notification-deep-link'
      ? `/?${new URLSearchParams({ notification: notifications[Number(sampleIndex) - 1], thread: target, event: middle, tap })}`
      : '/';
    await Promise.all([
      sample,
      journey === 'thread-navigation'
        ? page.locator(`.thread-row[data-thread-nav="${target}"]:visible`).click()
        : page.goto(url),
    ]);
    const value = fromRequest(await sample, journey, target);
    writeFileSync(output, JSON.stringify({ journey, value, browser: browserName, browserVersion: browser.version() }));
  } finally {
    await browser.close();
  }
}

const [command, ...args] = process.argv.slice(2);
if (command === 'seed') seed(args[0]);
else if (command === 'measure') await measure(args[0], args[1], args[2], args[3], args[4]);
else throw new Error(`Unknown app lab command: ${command}`);
