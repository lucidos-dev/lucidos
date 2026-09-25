import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** "Move to top level" in the thread menu (ADR 0278): the item is offered on a
 *  nested row only, it confirms, and the row leaves its family live.
 *
 *  Runs on every project. A desktop row opens its menu from its ⋯. A mobile row
 *  opens the same menu from a hold. */

function seedParentChild(): { parentId: string; childId: string } {
  const parentId = randomUUID();
  const childId = randomUUID();
  const now = new Date().toISOString();
  psql([
    seedThreadRow({ id: parentId, title: `parent-${Date.now()}`, totalChildren: 1, now }),
    seedThreadRow({ id: childId, title: `child-${Date.now()}`, parentId, now }),
  ].join(';\n'));
  return { parentId, childId };
}

/** The wrapper that carries a row's nesting (`is-nested`). */
const rowWrap = (page: Page, threadId: string) =>
  page.locator(`.thread-drawer .thread-row-wrap:has(.thread-row[data-thread-nav="${threadId}"])`).first();

/** Open one row's menu: its ⋯ where the row draws one, else a hold. */
async function openRowMenu(page: Page, threadId: string): Promise<void> {
  const opened = await page.evaluate(async (id) => {
    const row = [...document.querySelectorAll(`.thread-drawer .thread-row[data-thread-nav="${id}"]`)]
      .find((el) => el.getBoundingClientRect().width > 0);
    if (!row) return 'none';
    const wrap = row.closest('.thread-row-wrap') ?? row;
    const trigger = wrap.querySelector('button[aria-label="More thread actions"]');
    if (trigger && trigger.getBoundingClientRect().width > 0) {
      (trigger as HTMLElement).click();
      return 'trigger';
    }
    const box = row.getBoundingClientRect();
    const at = (type: string) => row.dispatchEvent(new PointerEvent(type, {
      bubbles: true,
      cancelable: true,
      button: 0,
      isPrimary: true,
      clientX: box.left + box.width / 2,
      clientY: box.top + box.height / 2,
    }));
    at('pointerdown');
    await new Promise((resolve) => setTimeout(resolve, 600));
    at('pointerup');
    return 'hold';
  }, threadId);
  expect(opened, `no drawer row for ${threadId}`).not.toBe('none');
  await expect(page.locator('.thread-overflow-menu')).toHaveCount(1);
}

const moveItem = (page: Page) =>
  page.locator('.thread-overflow-menu .thread-overflow-item', { hasText: 'Move to top level' });

test.describe('Move to top level', () => {
  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    // A collapsed Archive section from another spec would hide the seeded rows.
    await context.addInitScript(() => {
      localStorage.removeItem('lucidos-drawer-collapsed');
    });
    clearAllThreads();
  });

  test('a nested row moves to top level after the confirm', async ({ page }) => {
    const { parentId, childId } = seedParentChild();
    await navigateToApp(page);
    await openThreadDrawer(page);
    await expect(rowWrap(page, childId)).toHaveClass(/is-nested/);

    await openRowMenu(page, childId);
    await moveItem(page).click();

    const dialog = page.locator('.confirm-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('It keeps running and finishes on its own');
    // Not red: nothing is stopped or lost.
    await expect(page.locator('.confirm-btn-ok')).toHaveCount(0);
    await page.locator('.confirm-btn-ok-default', { hasText: 'Move out' }).click();

    await expect(rowWrap(page, childId)).not.toHaveClass(/is-nested/);
    expect(psql(`SELECT parent_thread_id FROM thread_summaries WHERE thread_id = '${childId}'`)).toBe('');
    expect(psql(
      `SELECT COUNT(*) FROM events WHERE aggregate_id = '${parentId}' AND event_type = 'ChildThreadDetached'`,
    )).toBe('1');
  });

  test('cancelling the confirm leaves the family as it was', async ({ page }) => {
    const { childId } = seedParentChild();
    await navigateToApp(page);
    await openThreadDrawer(page);

    await openRowMenu(page, childId);
    await moveItem(page).click();
    await expect(page.locator('.confirm-dialog')).toBeVisible();
    await page.locator('.confirm-btn-cancel').click();

    await expect(page.locator('.confirm-dialog')).toHaveCount(0);
    await expect(rowWrap(page, childId)).toHaveClass(/is-nested/);
  });

  test('a top-level row is not offered the move', async ({ page }) => {
    const { parentId } = seedParentChild();
    await navigateToApp(page);
    await openThreadDrawer(page);

    await openRowMenu(page, parentId);
    await expect(page.locator('.thread-overflow-menu .thread-overflow-item').first()).toBeVisible();
    await expect(moveItem(page)).toHaveCount(0);
  });
});
