import { test, expect } from '@playwright/test';
import { randomUUID } from 'crypto';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy } from './helpers';
import { psql } from './db-helpers';

test.describe('Message route panel', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('clicking the route badge opens the panel; outside-click closes it', async ({ page }) => {
    await navigateToApp(page);
    const msg = uniqueMessage('route-panel');
    await sendMessage(page, `Say exactly: "ok ${msg}"`);
    await waitForResponse(page);

    const badge = page.locator('.initiator-actor:visible').first();
    await expect(badge).toBeVisible();
    await badge.click();

    const panel = page.locator('.message-route-panel');
    await expect(panel).toBeVisible();
    // Clicking the user actor opens the origin-only popover (executor info has its
    // own button on the response panel since the per-panel popover split).
    await expect(panel).toContainText('Origin');

    // The frontend always sends x-lucidos-device-id (auto-generated in
    // localStorage on first call), so origin is Device with the engine-derived
    // fallback label "device-<short id>" until the user names the device.
    const originSection = panel.locator('.route-section').first();
    await expect(originSection).toContainText(/API client|device-|Workspace/);

    // Outside-click dismisses the panel.
    await page.locator('.app-shell').click({ position: { x: 5, y: 5 } });
    await expect(panel).toBeHidden();
  });

  test('Escape key closes the panel', async ({ page }) => {
    await navigateToApp(page);
    const msg = uniqueMessage('route-panel-esc');
    await sendMessage(page, `Say exactly: "ack ${msg}"`);
    await waitForResponse(page);

    const badge = page.locator('.initiator-actor:visible').first();
    await badge.click();
    const panel = page.locator('.message-route-panel');
    await expect(panel).toBeVisible();

    await page.keyboard.press('Escape');
    await expect(panel).toBeHidden();
  });

  test('clicking the same badge a second time closes the panel', async ({ page }) => {
    await navigateToApp(page);
    const msg = uniqueMessage('route-panel-toggle');
    await sendMessage(page, `Say exactly: "ok ${msg}"`);
    await waitForResponse(page);

    const badge = page.locator('.initiator-actor:visible').first();
    await badge.click();
    const panel = page.locator('.message-route-panel');
    await expect(panel).toBeVisible();

    await badge.click();
    await expect(panel).toBeHidden();
  });

  test('scrolling the chat keeps the panel open and re-anchored to the badge', async ({ page }) => {
    await navigateToApp(page);
    // Send several messages so the chat pane has something to scroll.
    for (let i = 0; i < 4; i++) {
      const m = uniqueMessage(`route-panel-scroll-${i}`);
      await sendMessage(page, `Say exactly: "ok ${m}"`);
      await waitForResponse(page);
    }

    const badge = page.locator('.initiator-actor:visible').first();
    await badge.click();
    const panel = page.locator('.message-route-panel');
    await expect(panel).toBeVisible();

    const before = await badge.boundingBox();
    expect(before).not.toBeNull();
    const panelBefore = await panel.boundingBox();
    expect(panelBefore).not.toBeNull();

    // Chat pane is its own scroll container, not window — walk up from the badge
    // to find the actual scroller.
    await badge.evaluate((el) => {
      let n: HTMLElement | null = el;
      while (n) {
        const oy = getComputedStyle(n).overflowY;
        if ((oy === 'auto' || oy === 'scroll') && n.scrollHeight > n.clientHeight) {
          n.scrollTop = Math.max(0, n.scrollTop - 100);
          return;
        }
        n = n.parentElement;
      }
    });

    await expect(panel).toBeVisible();

    const after = await badge.boundingBox();
    const panelAfter = await panel.boundingBox();
    expect(after).not.toBeNull();
    expect(panelAfter).not.toBeNull();
    const anchorDelta = (after!.y) - (before!.y);
    const panelDelta = (panelAfter!.y) - (panelBefore!.y);
    expect(Math.abs(anchorDelta - panelDelta)).toBeLessThan(2);
  });

  test('engine-origin MessageReceived renders the engine explainer copy in popover', async ({ page }) => {
    // Inject a thread with a MessageReceived event stamped with engine origin
    // (mirrors what the engine emits after Tasks 8-10). This bypasses the live
    // CC recovery path so the test is deterministic.
    const threadId = randomUUID();
    const eventId = randomUUID();
    const respId = randomUUID();
    const now = new Date().toISOString();
    const enginePayload = JSON.stringify({
      text: 'engine-origin route panel test',
      channel: 'chat',
      mode: 'engine',
      origin: { kind: 'engine', reason: { kind: 'session_recovered' } },
    });

    try {
      psql([
        `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'Engine origin e2e', 'chat', '${now}', 1, false, true, 'done', 'inbox', false, 0, false, false, false)`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${eventId}', 'MessageReceived', '${enginePayload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
        `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${respId}', 'ResponseGenerated', '{"text":"ok","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      ].join(';\n'));

      // Pre-seed the focused thread so navigateToApp lands on it directly
      // (mirrors the pattern used by cc-stuck-waiting.spec.ts).
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await assertHealthy(page);

      // The actor button (origin badge) on the user message opens the popover.
      const badge = page.locator('.initiator-actor:visible').first();
      await expect(badge).toBeVisible();
      await badge.click();

      const panel = page.locator('.message-route-panel');
      await expect(panel).toBeVisible();
      const originSection = panel.locator('.route-section').first();
      // Engine origins now render the explainer ("why the engine acted" + body)
      // instead of the old "Engine · Auto-resumed after restart" single-line label.
      await expect(originSection).toContainText('Why the engine acted');
      await expect(originSection).toContainText(/auto-resumed/i);
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
