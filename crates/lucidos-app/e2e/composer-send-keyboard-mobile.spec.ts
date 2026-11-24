import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, waitForPaneAtRest, isMobileViewport, touchPress } from './helpers';
import { psql, createCCThreadWithChange, cleanupCCThread } from './db-helpers';
import { randomUUID } from 'crypto';


const KEYBOARD_APP_HEIGHT_PX = 490;

const LATE_CLICK_GRACE_MS = 1_500;

test.describe('Composer Send with the mobile keyboard up', () => {
  test.use({ viewport: { width: 393, height: 852 } });

  test('sends on one touch tap, and only once', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const title = `E2E Send ${suffix}`;
    const followUp = `one tap only ${suffix}`;
    const now = new Date().toISOString();

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', '${title}', 'chat', '${now}', 1, false, true, 'idle', 'inbox', 'active', false, 0, 0, 'none', false)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"seed","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Seeded.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const sentCount = () => psql(
      `SELECT count(*) FROM events WHERE thread_id = '${threadId}'`
      + ` AND event_type = 'MessageReceived' AND payload->>'text' = '${followUp}'`,
    );

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("${title}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);

      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--user-ui-scale', '112.5%');
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill(followUp);
      await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');

      const send = page.locator('button[aria-label="Send message"]:visible:not(:disabled)').first();
      await expect(send).toBeVisible({ timeout: 10_000 });

      const probe = await page.evaluate(() => {
        const btn = Array.from(document.querySelectorAll<HTMLElement>('button[aria-label="Send message"]'))
          .find(b => b.getBoundingClientRect().width > 0);
        if (!btn) return null;
        const r = btn.getBoundingClientRect();
        const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        const shell = document.querySelector('.app-shell') as HTMLElement;
        return {
          rect: { top: r.top, bottom: r.bottom, left: r.left, right: r.right },
          pointerEvents: getComputedStyle(btn).pointerEvents,
          centreIsSend: hit === btn || btn.contains(hit),
          centreHit: hit ? `${hit.tagName.toLowerCase()}.${hit.getAttribute('class') ?? ''} ${hit.getAttribute('aria-label') ?? ''}` : 'nothing',
          shellBottom: shell.getBoundingClientRect().bottom,
        };
      });

      expect(probe, 'the Send button never rendered').not.toBeNull();
      expect(probe!.pointerEvents, 'Send is inert while the keyboard is up').not.toBe('none');
      expect(probe!.centreIsSend, `${probe!.centreHit} answers the pointer at the centre of Send`).toBe(true);
      expect(probe!.rect.bottom, 'Send sits below the app shell, behind the keyboard')
        .toBeLessThanOrEqual(probe!.shellBottom + 0.5);

      if (browserName !== 'chromium') return;

      const r = probe!.rect;
      await touchPress(page, [
        { type: 'touchStart', x: Math.round((r.left + r.right) / 2), y: Math.round((r.top + r.bottom) / 2) },
        { type: 'touchEnd' },
      ]);

      await expect
        .poll(sentCount, { timeout: 15_000, message: 'a touch tap on Send never produced a message' })
        .toBe('1');
      await expect(input, 'the draft survived the send').toHaveValue('');

      await page.waitForTimeout(LATE_CLICK_GRACE_MS);
      expect(sentCount(), 'one tap sent the message twice').toBe('1');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('sends the draft when the box has lost it', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    test.skip(browserName !== 'chromium', 'needs CDP touch injection');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const title = `E2E Draft Split ${suffix}`;
    const followUp = `only the draft has this ${suffix}`;
    const now = new Date().toISOString();

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', '${title}', 'chat', '${now}', 1, false, true, 'idle', 'inbox', 'active', false, 0, 0, 'none', false)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"seed","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Seeded.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const sentCount = () => psql(
      `SELECT count(*) FROM events WHERE thread_id = '${threadId}'`
      + ` AND event_type = 'MessageReceived' AND payload->>'text' = '${followUp}'`,
    );

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("${title}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);

      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill(followUp);

      const send = page.locator('button[aria-label="Send message"]:visible:not(:disabled)').first();
      await expect(send).toBeVisible({ timeout: 10_000 });

      await page.evaluate(() => {
        const els = document.querySelectorAll<HTMLTextAreaElement>('[data-role="prompt-input"]');
        for (const el of els) el.value = '';
      });
      await expect(send).toBeVisible();
      await expect(input, 'the box was not blanked, so the state is not the reported one')
        .toHaveValue('');

      const box = await send.boundingBox();
      expect(box, 'the Send button never rendered').not.toBeNull();
      const x = Math.round(box!.x + box!.width / 2);
      const y = Math.round(box!.y + box!.height / 2);
      await touchPress(page, [{ type: 'touchStart', x, y }, { type: 'touchEnd' }]);

      await expect
        .poll(sentCount, { timeout: 15_000, message: 'the press died on a box the send never needed' })
        .toBe('1');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('a press that travels before lifting still sends', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    test.skip(browserName !== 'chromium', 'needs CDP touch injection');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const title = `E2E Travel ${suffix}`;
    const followUp = `finger rolled ${suffix}`;
    const now = new Date().toISOString();

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', '${title}', 'chat', '${now}', 1, false, true, 'idle', 'inbox', 'active', false, 0, 0, 'none', false)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"seed","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Seeded.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    const sentCount = () => psql(
      `SELECT count(*) FROM events WHERE thread_id = '${threadId}'`
      + ` AND event_type = 'MessageReceived' AND payload->>'text' = '${followUp}'`,
    );

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("${title}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);

      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill(followUp);

      const send = page.locator('button[aria-label="Send message"]:visible:not(:disabled)').first();
      await expect(send).toBeVisible({ timeout: 10_000 });
      const box = await send.boundingBox();
      expect(box, 'the Send button never rendered').not.toBeNull();

      const x = Math.round(box!.x + box!.width / 2);
      const y = Math.round(box!.y + box!.height / 2);
      const startHit = await page.evaluate(({ px, py }) => {
        const hit = document.elementFromPoint(px, py);
        if (hit?.closest('button[aria-label="Send message"]')) return 'send';
        return hit ? `${hit.tagName.toLowerCase()}.${hit.getAttribute('class') ?? ''} ${hit.getAttribute('aria-label') ?? ''}` : 'nothing';
      }, { px: x, py: y });
      expect(startHit, `${startHit} answers the pointer where the press starts`).toBe('send');
      await touchPress(page, [
        { type: 'touchStart', x, y },
        { type: 'touchMove', x, y: y + 20 },
        { type: 'touchEnd' },
      ]);

      await expect
        .poll(sentCount, { timeout: 15_000, message: 'a press that moved 20px was thrown away' })
        .toBe('1');
      await expect(input, 'the draft survived the send').toHaveValue('');

      await expect(page.locator('.toast', { hasText: 'Tap ignored' })).toHaveCount(0);

      await page.waitForTimeout(LATE_CLICK_GRACE_MS);
      expect(sentCount(), 'one press sent the message twice').toBe('1');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });

  test('Diff opens on one touch tap with the keyboard up', async ({ page, browserName }) => {
    test.skip(!isMobileViewport(page), 'the keyboard-active block is a mobile-only rule');
    test.skip(browserName !== 'chromium', 'needs CDP touch injection');
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const cc = createCCThreadWithChange('E2E Diff Tap', suffix);

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("E2E Diff Tap ${suffix}")`).first().click();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);

      await page.evaluate((h: number) => {
        document.documentElement.style.setProperty('--app-height', `${h}px`);
      }, KEYBOARD_APP_HEIGHT_PX);

      const input = page.locator('[data-role="prompt-input"]:visible').first();
      await input.focus();
      await input.fill(`typing while reviewing ${suffix}`);
      await expect(page.locator('html')).toHaveAttribute('data-keyboard-active', '');

      const diff = page.locator('.prompt-actions-row button[data-role="thread-diff"]').first();
      await expect(diff).toBeVisible({ timeout: 10_000 });
      const box = await diff.boundingBox();
      expect(box, 'the Diff button never rendered').not.toBeNull();

      const x = Math.round(box!.x + box!.width / 2);
      const y = Math.round(box!.y + box!.height / 2);
      await touchPress(page, [{ type: 'touchStart', x, y }, { type: 'touchEnd' }]);

      await expect(page.locator('.app-header'))
        .toHaveAttribute('data-mobile-view', 'content', { timeout: 15_000 });
      await expect(page.locator('.files-source-switcher').first()).toBeVisible({ timeout: 15_000 });

      await page.waitForTimeout(LATE_CLICK_GRACE_MS);
      await expect(page.locator('html')).not.toHaveAttribute('data-keyboard-active', '');
    } finally {
      cleanupCCThread(cc.threadId, cc.changeId, cc.branch, cc.file);
    }
  });
});
