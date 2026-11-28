import type { APIRequestContext } from '@playwright/test';
import { test, expect, request as pwRequest } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse, uniqueMessage,
  assertHealthy, openThreadDrawer, userMessageBody, pickComposeDestination, newThread,
  waitForCCToFinish, waitForCCToStart, getBaseUrl,
} from './helpers';

test.describe('Chat - send and receive messages', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('send a message and see a response', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('chat-basic');
    await sendMessage(page, `Say exactly: "hello ${msg}"`);

    await expect(userMessageBody(page)).toContainText(msg, { timeout: 10_000 });

    const response = await waitForResponse(page);
    const responseText = await response.textContent();
    expect(responseText!.trim().length).toBeGreaterThan(0);
  });

  test('thread appears in the sidebar after sending a message', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('sidebar-thread');
    await sendMessage(page, `Say exactly: "acknowledged ${msg}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    const threadRows = page.locator('.thread-row:visible');
    await expect(threadRows.first()).toBeVisible({ timeout: 15_000 });

    const focusedRow = page.locator('.thread-row-focused:visible').first();
    await expect(focusedRow).toBeVisible();
  });

  test('response has non-empty content', async ({ page }) => {
    await navigateToApp(page);

    await sendMessage(page, `What is 2 + 2? Reply with just the number.`);
    const response = await waitForResponse(page);

    const text = await response.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);
  });

  test.describe('with Bash granted', () => {
    let apiContext: APIRequestContext | undefined;
    let allowedToolsSnapshot: string | undefined;
    test.beforeEach(async () => {
      apiContext = undefined;
      allowedToolsSnapshot = undefined;
      apiContext = await pwRequest.newContext({ baseURL: getBaseUrl(), ignoreHTTPSErrors: true });
      const resp = await apiContext.get('/api/v1/cc-allowed-tools');
      expect(resp.status()).toBe(200);
      allowedToolsSnapshot = (await resp.json()).contents as string;
      const put = await apiContext.put('/api/v1/cc-allowed-tools', {
        data: { contents: `${allowedToolsSnapshot}\nBash\n` },
      });
      expect(put.status(), await put.text()).toBe(204);
    });
    test.afterEach(async () => {
      if (!apiContext) return;
      try {
        if (allowedToolsSnapshot !== undefined) {
          const restore = await apiContext.put('/api/v1/cc-allowed-tools', {
            data: { contents: allowedToolsSnapshot },
          });
          expect(restore.status(), await restore.text()).toBe(204);
        }
      } finally {
        await apiContext.dispose();
      }
    });

    test('mid-turn user message reaches CC during active tool calls', { tag: '@real-claude-code' }, async ({ page }) => {
      test.setTimeout(180_000);

      await navigateToApp(page);
      await newThread(page);
      await pickComposeDestination(page);

      const slowTask =
        `Run this exact bash command and include the full output verbatim in your final answer: ` +
        `bash -c 'for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do echo widget$i; sleep 2; done'`;
      await sendMessage(page, slowTask);

      await waitForCCToStart(page, 60_000);

      const threadId = await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'));
      expect(threadId, 'focused thread id not in localStorage').toBeTruthy();

      await expect.poll(
        async () => {
          const resp = await page.request.get(`/api/v1/threads/${threadId}/events`);
          if (!resp.ok()) return false;
          const body = await resp.json();
          const events: Array<{ event_type?: string }> = Array.isArray(body)
            ? body
            : (body.events ?? []);
          return events.some(e => e.event_type === 'CodingAgentToolCalled');
        },
        { intervals: [500, 1000, 2000], timeout: 60_000 },
      ).toBe(true);

      await sendFollowUp(page, `Also remember the codeword "horseshoe" and include it in your final answer.`);

      await waitForCCToFinish(page, 120_000);

      const responseText = await page.evaluate(() => {
        const els = document.querySelectorAll('.response-content');
        const visible = Array.from(els).filter(el => {
          const rect = el.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
        });
        return visible.map(el => el.textContent ?? '').join('\n');
      });

      const lower = responseText.toLowerCase();
      expect(
        lower,
        `Response did not reference the original bash task. If CC restarted ` +
          `mid-turn (Phase 2 cancellation leaking into the within-turn path), ` +
          `the bash output is lost and only the codeword survives. ` +
          `Response: ${JSON.stringify(responseText)}`,
      ).toContain('widget15');
      expect(
        lower,
        `Response did not reference the mid-turn codeword. If the msg_rx → ` +
          `agent_input_tx pipe is broken, msg2 never reaches CC's stdin and ` +
          `the codeword is silently dropped. ` +
          `Response: ${JSON.stringify(responseText)}`,
      ).toContain('horseshoe');
    });
  });
});
