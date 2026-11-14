import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { assertHealthy, ensureMobileView, navigateToApp } from './helpers';
import { psql } from './db-helpers';

// Reads real motion, so it opts out of the calm default (playwright.config.ts).
test.use({ reducedMotion: 'no-preference' });

// Every mobile pane stays mounted, and an infinite animation in one off screen
// still restyles and repaints it on every frame. On an iPhone, that work shares
// the main thread with typing. The drawer's running shimmer and spinners
// stalled composer keystrokes by over a second. So a pane that is not the
// current one holds its animations paused.

/** A running thread, so the drawer draws its running shimmer and spinner. */
function seedRunningThread(): string {
  const threadId = randomUUID();
  const now = new Date().toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Running thread', 'chat', '${now}', 1, false, false, 'running', 'inbox', 'active', false, 0, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"Keep working","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));
  return threadId;
}

/** The play state of every animation inside the threads drawer's pane. */
function drawerPaneAnimationStates(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const pane = document.querySelector('.mobile-threads-pane')?.closest('.mobile-swipe-pane');
    if (!pane) throw new Error('no swipe pane holds the threads drawer');
    return document.getAnimations()
      .filter((a) => {
        const target = (a.effect as KeyframeEffect | null)?.target;
        return target instanceof Element && pane.contains(target);
      })
      .map((a) => a.playState);
  });
}

test.describe('A mobile pane off screen', () => {
  let threadId: string | null = null;

  test.afterEach(() => {
    if (!threadId) return;
    psql([
      `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
      `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
    ].join(';\n'));
    threadId = null;
  });

  test('holds its animations paused, and runs them again once it is the current pane', async ({ page }) => {
    await assertHealthy(page);
    threadId = seedRunningThread();
    await navigateToApp(page);

    // On the drawer, its running animations play.
    await ensureMobileView(page, 'threads');
    await expect.poll(() => drawerPaneAnimationStates(page), { timeout: 10_000 }).toContain('running');

    // Off screen, every one of them holds still.
    await ensureMobileView(page, 'thread');
    await expect.poll(async () => {
      const states = await drawerPaneAnimationStates(page);
      return states.length > 0 && states.every((s) => s === 'paused');
    }, { timeout: 5_000 }).toBe(true);

    // Back on the drawer, they play again.
    await ensureMobileView(page, 'threads');
    await expect.poll(async () => {
      const states = await drawerPaneAnimationStates(page);
      return states.length > 0 && states.every((s) => s === 'running');
    }, { timeout: 5_000 }).toBe(true);
  });
});
