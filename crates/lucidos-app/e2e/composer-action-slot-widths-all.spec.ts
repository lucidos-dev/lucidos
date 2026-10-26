import { test, expect, Page } from './fixtures';
import {
  assertHealthy, navigateToApp, ensureOnThreadPane, uniqueMessage, waitForVisibleInput, isMobileViewport,
  hideDashboardRail,
} from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

/**
 * The composer row's picker and action button keep one width per layout, in
 * every state (ADR 0573). A thread with a pending change shows the Apply split
 * in the action slot; one typed character swaps it for Send. Neither control
 * may change width, and neither right edge may move.
 *
 * Every face's word fits its pill, at every UI scale the sweep reaches.
 *
 * Plan: `docs/plans/2026-10-25-composer-row-fixed-width-controls.md`.
 */

interface SlotReading {
  slot: { left: number; right: number; width: number; height: number };
  /** Null once the row folded the picker into the ⋯, which a narrow pane at a
   *  large UI scale does. Folding is the row's only answer to lack of room. */
  picker: { right: number; width: number } | null;
  /** Every visible slot word that overflows its box. */
  clipped: string[];
}

async function readSlot(page: Page): Promise<SlotReading | null> {
  return page.evaluate(() => {
    const row = Array.from(document.querySelectorAll<HTMLElement>('.prompt-actions-row'))
      .find((r) => r.getBoundingClientRect().width > 0);
    const slot = row?.querySelector<HTMLElement>('.prompt-actions-right > :last-child');
    const picker = row?.querySelector<HTMLElement>('.rung-control');
    if (!slot || picker?.hasAttribute('data-pending')) return null;
    const s = slot.getBoundingClientRect();
    const p = picker?.getBoundingClientRect();
    const labels = Array.from(slot.querySelectorAll<HTMLElement>('.action-slot-label'))
      .filter((l) => getComputedStyle(l).position !== 'absolute');
    return {
      slot: { left: s.left, right: s.right, width: s.width, height: s.height },
      picker: p ? { right: p.right, width: p.width } : null,
      clipped: labels.filter((l) => l.scrollWidth > l.clientWidth + 0.5).map((l) => l.textContent ?? ''),
    };
  });
}

/** A reading taken once two consecutive reads agree, so a fold still
 *  settling is never measured. */
async function settled(page: Page, label: string, ready: () => Promise<boolean>): Promise<SlotReading> {
  let previous: string | null = null;
  await expect.poll(async () => {
    if (!(await ready())) return false;
    const reading = await readSlot(page);
    if (!reading) return false;
    const signature = JSON.stringify(reading);
    const same = signature === previous;
    previous = signature;
    return same;
  }, { timeout: 15_000, message: `${label}: the row never settled` }).toBe(true);
  return (await readSlot(page))!;
}

const row = (page: Page) => page.locator('.prompt-actions-row:visible');

test.describe('The composer row keeps its widths', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  for (const scale of [100, 112.5, 137.5]) {
    test(`the action slot and the picker hold still from Apply to Send at ui-scale ${scale}`, async ({ page }) => {
      const suffix = uniqueMessage('slotwidth').replace(/[^a-z0-9-]/g, '');
      const { threadId, changeId, branch, file } = createCCThreadWithChange(
        'E2E Slot Width', suffix, { requiresRestart: true },
      );
      try {
        await page.addInitScript((tid: string) => {
          localStorage.setItem('lucidos-focused-thread', tid);
        }, threadId);
        // The rail would squeeze the thread pane until the picker folds, and a
        // narrow pane draws the compact faces (ADR 0576). The mobile project
        // keeps its emulated viewport.
        if (!isMobileViewport(page)) await page.setViewportSize({ width: 2600, height: 1000 });
        await hideDashboardRail(page);
        await navigateToApp(page);
        await ensureOnThreadPane(page);
        await expect(page.locator('.thread-action-buttons:visible')).toBeVisible({ timeout: 15_000 });
        await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', `${s}%`), scale);

        const apply = await settled(page, 'Apply', async () => (await row(page).locator('.split-button').count()) > 0);
        await (await waitForVisibleInput(page)).fill('x');
        const send = await settled(page, 'Send', async () => (await row(page).locator('.send-cancel-morph').count()) > 0);

        expect(send.slot.width, 'the action slot changed width from Apply to Send').toBeCloseTo(apply.slot.width, 0);
        expect(send.slot.right, 'the action slot moved').toBeCloseTo(apply.slot.right, 0);
        expect(!!send.picker, 'the picker folded or unfolded from Apply to Send').toBe(!!apply.picker);
        if (apply.picker && send.picker) {
          expect(send.picker.width, 'the picker changed width').toBeCloseTo(apply.picker.width, 0);
          expect(send.picker.right, 'the picker moved').toBeCloseTo(apply.picker.right, 0);
        }
        expect([...apply.clipped, ...send.clipped], 'a face word is clipped').toEqual([]);

        if (isMobileViewport(page)) {
          // The round button, Send's size, whatever it carries.
          expect(Math.abs(apply.slot.width - apply.slot.height)).toBeLessThan(1);
        } else {
          expect(apply.slot.width, 'the desktop slot is a pill, not a circle').toBeGreaterThan(apply.slot.height * 2);
        }
      } finally {
        cleanupCCThread(threadId, changeId, branch, file);
      }
    });
  }
});
