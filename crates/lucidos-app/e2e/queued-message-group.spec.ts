import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import { assertHealthy, ensureOnThreadPane, navigateToApp, openThreadDrawer, REAL_THREAD_ROW, USER_MSG_SELECTOR } from './helpers';
import { psql } from './db-helpers';

test.describe('Queued chat messages', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('stacks multiple persisted queued follow-ups in a collapsed group', async ({ page }) => {
    const threadId = randomUUID();
    const activeMessageId = randomUUID();
    const title = `Queued group e2e ${randomUUID().slice(0, 8)}`;
    const activeMarker = `active-${randomUUID().slice(0, 8)}`;
    const queuedOne = `queued-one-${randomUUID().slice(0, 8)}`;
    const queuedTwo = `queued-two-${randomUUID().slice(0, 8)}`;
    const t0 = new Date().toISOString();
    const t1 = new Date(Date.now() + 1000).toISOString();
    const t2 = new Date(Date.now() + 2000).toISOString();
    const t3 = new Date(Date.now() + 3000).toISOString();

    psql([
      `INSERT INTO thread_summaries (` +
        `thread_id, title, source, last_activity, message_count, is_saved, has_response, status, ` +
        `archive_state, state, is_coding_agent, active_children_count, total_children_count, ` +
        `coding_agent_change_state, coding_agent_is_external_repo` +
      `) VALUES (` +
        `'${threadId}', '${title}', 'chat', '${t3}', 3, false, false, 'running', ` +
        `'inbox', 'active', false, 0, 0, 'none', false` +
      `)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${activeMessageId}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: activeMarker, channel: 'chat' })}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'TextStreamed', ` +
        `'${JSON.stringify({ text: 'Still working...', request_event_id: activeMessageId })}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: queuedOne, channel: 'chat' })}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: queuedTwo, channel: 'chat' })}'::jsonb, '${t3}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      const row = page.locator(`${REAL_THREAD_ROW}:visible`, { hasText: title }).first();
      await expect(row).toBeVisible();
      await row.click();
      await ensureOnThreadPane(page);

      const group = page.locator('.queued-message-group:visible').first();
      await expect(group.locator('.queued-message-group-summary')).toContainText('Queued (2)');
      await expect(page.locator(`${USER_MSG_SELECTOR}:visible`)).toContainText(activeMarker);
      await expect(page.locator(`${USER_MSG_SELECTOR}:visible`).filter({ hasText: queuedOne })).toHaveCount(0);

      await group.locator('.queued-message-group-summary').click();
      await expect(page.locator(`${USER_MSG_SELECTOR}:visible`).filter({ hasText: queuedOne })).toHaveCount(1);
      await expect(page.locator(`${USER_MSG_SELECTOR}:visible`).filter({ hasText: queuedTwo })).toHaveCount(1);
      await expect(group.locator('.exchange-status-label:visible')).toHaveCount(2);
      await expect(group.locator('.response-panel:visible')).toHaveCount(0);

      const measured = await group.locator('.queued-message-remove').first().evaluate(el => {
        const btn = el as HTMLElement;
        const root = parseFloat(getComputedStyle(document.documentElement).fontSize);
        const line = getComputedStyle(document.documentElement).getPropertyValue('--turn-header-line');
        const box = btn.getBoundingClientRect();
        const cx = box.left + box.width / 2;
        const cy = box.top + box.height / 2;
        const hits = (dx: number, dy: number) => {
          const at = document.elementFromPoint(cx + dx, cy + dy);
          return !!at && (at === btn || btn.contains(at));
        };
        const overlay = getComputedStyle(btn, '::before');
        const reachX = parseFloat(overlay.width) / 2;
        const reachY = parseFloat(overlay.height) / 2;
        const stamp = btn.closest('.initiator-header')!
          .querySelector('.initiator-timestamp') as HTMLElement;
        const stampBox = stamp.getBoundingClientRect();
        const atStampEdge = document.elementFromPoint(stampBox.left + 2, stampBox.top + stampBox.height / 2);
        const label = document.createRange();
        label.selectNodeContents(btn.closest('.exchange-status-label')!.firstChild!);
        const labelRight = label.getBoundingClientRect().right;
        return {
          fieldHeight: btn.closest('.exchange-status-label')!.getBoundingClientRect().height,
          lineHeight: parseFloat(line) * root,
          targetHeight: parseFloat(overlay.height),
          fullTarget: 2.25 * root,
          insideTarget: [hits(-(reachX - 2), 0), hits(reachX - 2, 0), hits(0, -(reachY - 2)), hits(0, reachY - 2)],
          pastTarget: [hits(0, -(reachY + 3)), hits(0, reachY + 3), hits(-(reachX + 3), 0), hits(reachX + 3, 0)],
          stampIsOwnTarget: atStampEdge === stamp || stamp.contains(atStampEdge),
          labelClearance: (cx - reachX) - labelRight,
        };
      });

      expect(measured.insideTarget, 'the tap target no longer covers its own box').toEqual([true, true, true, true]);
      expect(measured.pastTarget, 'the tap target reaches past its own box')
        .toEqual([false, false, false, false]);
      expect(measured.targetHeight, 'the target gave up height it had room for')
        .toBeCloseTo(measured.fullTarget, 0);
      expect(
        Math.abs(measured.fieldHeight - measured.lineHeight),
        `status field ${measured.fieldHeight}px against a row unit of ${measured.lineHeight}px`,
      ).toBeLessThan(1.5);
      expect(measured.stampIsOwnTarget, 'the trash overlay swallowed the timestamp button').toBe(true);
      expect(
        measured.labelClearance,
        `the overlay's left edge is ${(-measured.labelClearance).toFixed(2)}px into the Queued label`,
      ).toBeGreaterThanOrEqual(0);
    } finally {
      psql([
        `DELETE FROM events WHERE thread_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
