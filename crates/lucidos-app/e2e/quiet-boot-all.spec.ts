import { randomUUID } from 'node:crypto';
import { psql, seedChatThread } from './db-helpers';
import { test, expect, type Page } from './fixtures';

test.use({ reducedMotion: 'no-preference', serviceWorkers: 'block' });

type BootWindow = Window & { splashRemovedMs?: number };

async function measureSplashRemoval(page: Page): Promise<void> {
  await page.addInitScript(() => {
    let sawSplash = false;
    const observer = new MutationObserver(() => {
      if (document.querySelector('.boot-splash')) {
        sawSplash = true;
      } else if (sawSplash) {
        (window as BootWindow).splashRemovedMs = performance.now();
        observer.disconnect();
      }
    });
    observer.observe(document, { childList: true, subtree: true });
  });
}

async function splashRemovalMs(page: Page): Promise<number> {
  await page.waitForFunction(() => (window as BootWindow).splashRemovedMs !== undefined, undefined, {
    timeout: 8_000,
  });
  return page.evaluate(() => (window as BootWindow).splashRemovedMs!);
}

test('a local switch consumes the quiet flag and skips the reveal floor', async ({ page }) => {
  await page.goto('/', { waitUntil: 'domcontentloaded' });
  await measureSplashRemoval(page);

  await page.evaluate(() => {
    sessionStorage.setItem('lucidos-splash-quiet', '1');
    location.replace('/?switch-test=1');
  });
  await page.waitForURL('**/?switch-test=1', { waitUntil: 'commit' });

  await expect(page.locator('html')).toHaveAttribute('data-boot-splash-quiet', '');
  expect(await page.evaluate(() => sessionStorage.getItem('lucidos-splash-quiet'))).toBeNull();
  const removedMs = await splashRemovalMs(page);
  test.info().annotations.push({ type: 'splash removed (ms)', description: String(Math.round(removedMs)) });
  expect(removedMs).toBeLessThan(1_200);
});

test('a notification tap reveals its event while the thread list is still pending', async ({ page }) => {
  const threadId = seedChatThread({ turns: 1, needles: [], title: 'Quiet boot target' });
  const eventId = psql(`SELECT id FROM events WHERE thread_id = '${threadId}' AND event_type = 'MessageReceived'`);
  const tap = JSON.stringify({ kind: 'navigate', to: { target: 'thread', id: threadId, event_id: eventId } });
  const params = new URLSearchParams({ notification: randomUUID(), thread: threadId, event: eventId, tap });

  let releaseList!: () => void;
  const listGate = new Promise<void>(resolve => { releaseList = resolve; });
  let requestedList = false;

  try {
    await measureSplashRemoval(page);
    await page.route(url => url.pathname === '/api/v1/threads', async route => {
      requestedList = true;
      await listGate;
      await route.continue();
    });
    await page.goto(`/?${params}`, { waitUntil: 'commit' });
    await expect(page.locator('html')).toHaveAttribute('data-boot-splash-quiet', '');
    await expect.poll(() => requestedList, { timeout: 5_000 }).toBe(true);
    const removedMs = await splashRemovalMs(page);
    await expect(page.locator(`[data-event-id="${eventId}"]:visible`).first()).toBeInViewport();
    expect(removedMs).toBeLessThan(15_000);
    test.info().annotations.push({ type: 'splash removed (ms)', description: String(Math.round(removedMs)) });

    const listResponse = page.waitForResponse(res => new URL(res.url()).pathname === '/api/v1/threads');
    releaseList();
    expect((await listResponse).ok()).toBe(true);
  } finally {
    releaseList();
    await page.unrouteAll({ behavior: 'wait' });
    psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}';`);
  }
});
