import { test, expect, type Page } from './fixtures';
import {
  assertHealthy,
  disableMobileDynamicBars,
  enableMobileDynamicBars,
  ensureOnThreadPane,
  isMobileViewport,
  navigateToApp,
  openThreadDrawer,
} from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

test.use({ reducedMotion: 'no-preference' });


const KEYBOARD_APP_HEIGHT_PX = 490;
const FULL_APP_HEIGHT_PX = 852;

const PANE = '.mobile-swipe-pane';

interface ChromeFrame {
  headerTop: number;
  headerBottom: number;
  titleTop: number;
}

async function openThreadWithDraft(page: Page, title: string): Promise<void> {
  await navigateToApp(page);
  await openThreadDrawer(page);
  await page.locator(`.thread-row:has-text("${title}")`).first().click();
  await ensureOnThreadPane(page);
  await page.evaluate((h) => {
    document.documentElement.style.setProperty('--app-height', `${h}px`);
  }, KEYBOARD_APP_HEIGHT_PX);
  const input = page.locator(`${PANE} [data-role="prompt-input"]`).first();
  await input.focus();
  await input.fill('line one\nline two\nline three\nline four');
  await expect(page.locator(`${PANE} button[aria-label="Send message"]:not(:disabled)`)).toBeVisible();
}

test.describe('A mobile follow-up send moves as one piece', () => {
  test.use({ viewport: { width: 393, height: FULL_APP_HEIGHT_PX } });

  let threadId = '';
  let title = '';

  test.beforeEach(async ({ page }) => {
    test.skip(!isMobileViewport(page), 'the keyboard and the dynamic bars are mobile-only');
    await assertHealthy(page);
    await enableMobileDynamicBars(page);
    threadId = randomUUID();
    title = `E2E Send Motion ${threadId.slice(0, 8)}`;
    const now = new Date().toISOString();
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', '${title}', 'chat', '${now}', 1, false, true, 'idle', 'inbox', 'active', false, 0, 0, 'none', false)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"seed","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Seeded.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));
  });

  test.afterEach(async ({ page }) => {
    if (!threadId) return;
    psql([
      `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
      `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
    ].join(';\n'));
    await disableMobileDynamicBars(page);
  });

  test('the box eases shut, and Stop arrives at full strength', async ({ page }) => {
    await openThreadWithDraft(page, title);

    const atPress = await page.evaluate((pane) => {
      const box = document.querySelector<HTMLTextAreaElement>(`${pane} [data-role="prompt-input"]`)!;
      const before = box.getBoundingClientRect().height;
      document.querySelector<HTMLElement>(`${pane} .send-cancel-morph`)!.click();
      return { before, after: box.getBoundingClientRect().height, value: box.value };
    }, PANE);
    expect(atPress.value, 'the send did not empty the box').toBe('');
    expect(atPress.after, 'the box snapped shut instead of easing').toBeCloseTo(atPress.before, 0);

    const stop = page.locator(`${PANE} .send-cancel-morph[aria-label="Cancel"]`);
    await expect(stop, 'the send never turned the button into Stop').toBeVisible();
    await expect(stop).toBeDisabled();
    await expect(stop).toHaveCSS('opacity', '1');

    await expect.poll(
      () => page.locator(`${PANE} [data-role="prompt-input"]`).evaluate((el) => el.getBoundingClientRect().height),
      { message: 'the box never reached its one-line height' },
    ).toBeLessThan(atPress.before - 20);
  });

  test('the title bar comes back under the header, never ahead of it', async ({ page }) => {
    await openThreadWithDraft(page, title);
    await expect.poll(
      () => page.locator('.app-header').evaluate((el) => el.getBoundingClientRect().bottom),
      { message: 'the keyboard never sent the header away' },
    ).toBeLessThanOrEqual(0.5);

    await page.evaluate(({ pane, fullHeight }) => {
      const w = window as unknown as { __chromeFrames: ChromeFrame[] };
      w.__chromeFrames = [];
      const header = document.querySelector<HTMLElement>('.app-header')!;
      const titleRow = document.querySelector<HTMLElement>(`${pane} .mobile-thread-title-row`)!;
      const started = performance.now();
      const sample = () => {
        const h = header.getBoundingClientRect();
        w.__chromeFrames.push({
          headerTop: h.top,
          headerBottom: h.bottom,
          titleTop: titleRow.getBoundingClientRect().top,
        });
        if (performance.now() - started < 1_200) requestAnimationFrame(sample);
      };
      requestAnimationFrame(sample);
      document.querySelector<HTMLElement>(`${pane} .send-cancel-morph`)!.click();
      setTimeout(() => document.documentElement.style.setProperty('--app-height', `${fullHeight}px`), 150);
    }, { pane: PANE, fullHeight: FULL_APP_HEIGHT_PX });

    await page.waitForFunction(() => {
      const frames = (window as unknown as { __chromeFrames?: unknown[] }).__chromeFrames;
      return !!frames && frames.length > 0
        && document.querySelector('.app-header')!.getBoundingClientRect().top === 0;
    }, undefined, { timeout: 5_000 });
    await page.waitForTimeout(1_300);

    const frames = await page.evaluate(
      () => (window as unknown as { __chromeFrames: ChromeFrame[] }).__chromeFrames,
    );
    const midGlide = frames.filter((f) => f.headerTop < -10 && f.headerBottom > 0);
    expect(midGlide.length, 'no frame caught the header mid-glide').toBeGreaterThan(0);
    const ahead = midGlide.filter((f) => f.titleTop >= f.headerBottom - 1);
    expect(ahead, `the title bar led the header in: ${JSON.stringify(ahead.slice(0, 3))}`).toEqual([]);
    const last = frames[frames.length - 1];
    expect(last.titleTop, 'the title bar did not come back under the header').toBeCloseTo(last.headerBottom, 0);
  });
});
