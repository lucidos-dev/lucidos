import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, apiRequest } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/** Half a second of silence as a 16-bit mono WAV: a clip every engine plays. */
function silentWav(): Buffer {
  const rate = 8_000;
  const data = rate; // 0.5s of 2-byte samples
  const wav = Buffer.alloc(44 + data);
  wav.write('RIFF', 0);
  wav.writeUInt32LE(36 + data, 4);
  wav.write('WAVEfmt ', 8);
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(1, 22);
  wav.writeUInt32LE(rate, 24);
  wav.writeUInt32LE(rate * 2, 28);
  wav.writeUInt16LE(2, 32);
  wav.writeUInt16LE(16, 34);
  wav.write('data', 36);
  wav.writeUInt32LE(data, 40);
  return wav;
}

/**
 * ADR 0415: a question option draws its widget in one box with its button.
 * The built-in sound player plays its clip through the `/data` mount, and a
 * press on the player never answers the card: only the button picks.
 */
test.describe('a question option with a widget', () => {
  test('draws the sound player beside the option, and playing it answers nothing', async ({ page }) => {
    await assertHealthy(page);
    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const clip = `artifacts/e2e-voices/marin-${suffix}.wav`;
    const put = await apiRequest(page).put(`/api/v1/data/${clip}`, {
      headers: { 'Content-Type': 'audio/wav' },
      data: silentWav(),
    });
    expect(put.ok(), `writing ${clip}`).toBeTruthy();
    const now = new Date().toISOString();
    const payload = JSON.stringify({
      tool_use_id: `tu-widget-${suffix}`,
      cc_session_id: 'sess-e2e',
      question: `Which voice ${suffix}?`,
      options: [
        {
          id: 'opt-0',
          label: 'Marin',
          description: 'Warm, unhurried',
          widget: { app_id: 'lucidos-sound-player', params: { clip }, label: 'Marin' },
        },
        { id: 'opt-1', label: 'Ash' },
      ],
    }).replace(/'/g, "''");
    const insert = (type: string, body: string) =>
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', '${type}', '${body}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'Option Widget E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
      insert('MessageReceived', '{"text":"start","channel":"claude_code"}'),
      insert('SessionStarted', '{"session_id":"sess-e2e"}'),
      insert('UserQuestionAsked', payload),
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      const row = page.locator(`.thread-row:has-text("Option Widget E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const box = page.locator('.question-option-boxed:visible').first();
      await expect(box).toBeVisible({ timeout: 10_000 });
      const frame = box.locator('iframe');
      await expect(frame).toHaveCount(1, { timeout: 10_000 });
      await expect(frame).toHaveAttribute('src', /\/app\/lucidos-sound-player\/.*params=/);
      expect(
        await box.locator('button.question-option iframe').count(),
        'the frame is never inside the option button',
      ).toBe(0);

      const player = box.frameLocator('iframe');
      await expect(player.locator('#audio')).toHaveAttribute('src', new RegExp(`/data/${clip.replace(/\./g, '\\.')}`));

      // The frame holds its iframe, so the player never spills over the next
      // option: an iframe's intrinsic 150px must not outgrow a smaller frame.
      const spill = await box.evaluate((el) => {
        const frame = el.querySelector('.widget-frame')!.getBoundingClientRect();
        const iframe = el.querySelector('iframe')!.getBoundingClientRect();
        return iframe.bottom - frame.bottom;
      });
      expect(spill, 'the iframe overflows its widget frame').toBeLessThanOrEqual(0.5);
      const ash = page.locator('.question-option:visible', { hasText: 'Ash' });
      await ash.scrollIntoViewIfNeeded();
      const ashHit = await ash.evaluate((el) => {
        const r = el.getBoundingClientRect();
        const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return hit !== null && el.contains(hit);
      });
      expect(ashHit, 'the next option takes its own tap').toBe(true);
      await player.locator('#play').click();

      // A press inside the player never reaches the answer route.
      await page.waitForTimeout(1_000);
      expect(psql(
        `SELECT COUNT(*) FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered'`,
      )).toBe('0');
      await expect(box.locator('button.question-option')).toBeEnabled();
    } finally {
      psql(`DELETE FROM events WHERE thread_id = '${threadId}'; DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`);
      await apiRequest(page).delete(`/api/v1/data/${clip}`);
    }
  });
});
