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
        `coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, coding_agent_has_diff` +
      `) VALUES (` +
        `'${threadId}', '${title}', 'chat', '${t3}', 3, false, false, 'running', ` +
        `'inbox', 'active', false, 0, 0, false, false, false, false` +
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

      // The remove button takes its tap target from an overlay, not from its
      // box (`.icon-btn.inline-icon`, global/host-components.css). That is the
      // one claim in the rule a source scan cannot make: it is about rendered
      // boxes, and about where a thumb actually lands.
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
        const reach = 2.25 * root / 2;
        const stamp = btn.closest('.initiator-header')!
          .querySelector('.initiator-timestamp') as HTMLElement;
        const stampBox = stamp.getBoundingClientRect();
        const atStampEdge = document.elementFromPoint(stampBox.left + 2, stampBox.top + stampBox.height / 2);
        // The label on the other side is a bare text node. Measure it with a
        // range rather than looking for a box it does not have. Measured at the
        // EDGE: a centre probe passes while the overlay eats the last letters,
        // and this is the thinnest gap in the layout.
        const label = document.createRange();
        label.selectNodeContents(btn.closest('.exchange-status-label')!.firstChild!);
        const labelRight = label.getBoundingClientRect().right;
        return {
          // The field the button sits in, which is what the trash used to
          // stretch. It wraps on a narrow pane, so measure the field itself
          // rather than the header around it.
          fieldHeight: btn.closest('.exchange-status-label')!.getBoundingClientRect().height,
          lineHeight: parseFloat(line) * root,
          insideTarget: [hits(-(reach - 2), 0), hits(reach - 2, 0), hits(0, -(reach - 2)), hits(0, reach - 2)],
          pastTarget: [hits(-(reach + 3), 0), hits(reach + 3, 0)],
          stampIsOwnTarget: atStampEdge === stamp || stamp.contains(atStampEdge),
          labelClearance: (cx - reach) - labelRight,
        };
      });

      // A thumb landing anywhere in the 2.25rem target still hits the trash,
      // which is what the box used to guarantee and the overlay now does.
      expect(measured.insideTarget, 'the tap target no longer covers 2.25rem').toEqual([true, true, true, true]);
      expect(measured.pastTarget, 'the tap target reaches past 2.25rem').toEqual([false, false]);
      // And it takes no space in the line it interrupts. The reported defect
      // was the button holding this field at 2.25rem, nearly twice the row
      // unit, with the extra showing as air around the glyph.
      expect(
        Math.abs(measured.fieldHeight - measured.lineHeight),
        `status field ${measured.fieldHeight}px against a row unit of ${measured.lineHeight}px`,
      ).toBeLessThan(1.5);
      // The overlay reaches past the glyph, so both neighbours have to keep
      // their own ground. The timestamp is a button of its own, and the label
      // is what the clickable header folds the turn on. The label side is the
      // thinner of the two and the only one nothing else would catch: the
      // 0.375rem gap against a reach the chip and the glyph size both feed.
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
