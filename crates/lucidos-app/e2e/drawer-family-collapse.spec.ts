import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** Seed a parent thread with one child thread directly in `thread_summaries`. */
function seedParentChild(parentTitle: string, childTitle: string): { parentId: string; childId: string } {
    const parentId = randomUUID();
    const childId = randomUUID();
    const now = new Date().toISOString();
    psql([
        seedThreadRow({ id: parentId, title: parentTitle, totalChildren: 1, now }),
        seedThreadRow({ id: childId, title: childTitle, parentId, now }),
    ].join(';\n'));
    return { parentId, childId };
}

test.describe('Drawer family collapse', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        // Each spec starts with a clean drawer state — collapsed-families is
        // localStorage-backed, so a survivor from another test would mask the
        // collapse interaction we're verifying here.
        await context.clearCookies();
        // Section collapse is always cleared — Archive being collapsed from a
        // prior test would hide our seeded rows entirely. Family collapse is
        // NOT cleared via addInitScript (which re-runs on every reload and
        // would defeat the cross-reload persistence check); per-thread UUIDs
        // are random so stale family entries from prior tests are inert.
        await context.addInitScript(() => {
            localStorage.removeItem('lucidos-drawer-collapsed');
        });
        clearAllThreads();
    });

    test('toggle row hides family, persists across reload, and re-expands', async ({ page }) => {
        const parentTitle = `parent-${Date.now()}`;
        const childTitle = `child-${Date.now()}`;
        const { parentId, childId } = seedParentChild(parentTitle, childTitle);

        await navigateToApp(page);
        await openThreadDrawer(page);

        // Both rows visible initially.
        const parentRow = page.locator(`.thread-row[data-thread-nav="${parentId}"]`);
        const childRow = page.locator(`.thread-row[data-thread-nav="${childId}"]`);
        await expect(parentRow.first()).toBeVisible();
        await expect(childRow.first()).toBeVisible();

        // The disclosure control sits on the parent row (one per family head).
        // The button carries aria-expanded so we can probe state.
        const toggle = page.locator(`.family-disclosure[aria-label*="sub-thread"]`).first();
        await expect(toggle).toBeVisible();
        await expect(toggle).toHaveAttribute('aria-expanded', 'true');

        // Expanded = chevron only: the ▾ glyph shows, the count badge is absent
        // (children are inline, so the number would be redundant).
        await expect(toggle.locator('.family-disclosure-glyph')).toBeVisible();
        await expect(page.locator(`.family-disclosure .collapse-count-badge:visible`)).toHaveCount(0);

        // Click the control — child hides; parent stays.
        await toggle.click();
        await expect(childRow.first()).toBeHidden();
        await expect(parentRow.first()).toBeVisible();
        await expect(toggle).toHaveAttribute('aria-expanded', 'false');

        // Collapsed = badge only: the count badge reports the one hidden
        // sub-thread, and the chevron glyph is gone.
        const badge = page.locator(`.family-disclosure .collapse-count-badge:visible`).first();
        await expect(badge).toBeVisible();
        await expect(badge).toHaveText('1');
        await expect(toggle.locator('.family-disclosure-glyph')).toHaveCount(0);

        // Reload — collapsed state survives via localStorage.
        await page.reload();
        await openThreadDrawer(page);
        await expect(parentRow.first()).toBeVisible();
        await expect(childRow.first()).toBeHidden();
        const toggleAfterReload = page.locator(`.family-disclosure[aria-label*="sub-thread"]`).first();
        await expect(toggleAfterReload).toHaveAttribute('aria-expanded', 'false');
        // Still badge-only after reload: badge present, chevron absent.
        await expect(toggleAfterReload.locator('.collapse-count-badge')).toBeVisible();
        await expect(toggleAfterReload.locator('.family-disclosure-glyph')).toHaveCount(0);

        // Re-expand via the badge — child returns, badge disappears, chevron back.
        await toggleAfterReload.click();
        await expect(childRow.first()).toBeVisible();
        await expect(toggleAfterReload).toHaveAttribute('aria-expanded', 'true');
        await expect(page.locator(`.family-disclosure .collapse-count-badge:visible`)).toHaveCount(0);
        await expect(toggleAfterReload.locator('.family-disclosure-glyph')).toBeVisible();
    });

    test('long parent title pushes the disclosure badge clear of the title', async ({ page }) => {
        // A multi-line title used to grow DOWN into the bottom-centered
        // disclosure badge (absolutely positioned), so the count badge / chevron
        // overlapped the title's last line. The fix reserves bottom room in the
        // title column. Seed a deliberately long title so it wraps at every
        // project width.
        const longTitle = `Diagnosing Interrupted Response and Memory Issues Across Long Running Sessions ${Date.now()}`;
        const childTitle = `child-${Date.now()}`;
        const { parentId } = seedParentChild(longTitle, childTitle);

        await navigateToApp(page);
        await openThreadDrawer(page);

        const parentRow = page.locator(`.thread-row[data-thread-nav="${parentId}"]`).first();
        await expect(parentRow).toBeVisible();

        // Collapse to the count badge — the taller of the two disclosure states
        // and the one in the bug report.
        const toggle = parentRow.locator('.family-disclosure');
        await toggle.click();
        await expect(parentRow.locator('.family-disclosure .collapse-count-badge')).toBeVisible();

        const titleBox = await parentRow.locator('.thread-row-title').boundingBox();
        const badgeBox = await toggle.boundingBox();
        expect(titleBox).not.toBeNull();
        expect(badgeBox).not.toBeNull();

        // The title actually wrapped (more than one line) — otherwise the test
        // isn't exercising the overlap case.
        expect(titleBox!.height).toBeGreaterThan(30);

        // No overlap: the two rectangles must not intersect.
        const overlaps =
            titleBox!.x < badgeBox!.x + badgeBox!.width &&
            titleBox!.x + titleBox!.width > badgeBox!.x &&
            titleBox!.y < badgeBox!.y + badgeBox!.height &&
            titleBox!.y + titleBox!.height > badgeBox!.y;
        expect(overlaps).toBe(false);
    });

    test('chevron click does not focus the parent thread; row-body click does', async ({ page }) => {
        const parentTitle = `body-parent-${Date.now()}`;
        const childTitle = `body-child-${Date.now()}`;
        const { parentId } = seedParentChild(parentTitle, childTitle);

        await navigateToApp(page);
        await openThreadDrawer(page);

        const parentRow = page.locator(`.thread-row[data-thread-nav="${parentId}"]`).first();
        await expect(parentRow).toBeVisible();

        // Chevron click leaves focus alone — the disclosure button lives inside
        // the parent row but stopPropagation()s the click, so the row's own
        // onClick (focusThread) never fires.
        const toggle = page.locator(`.family-disclosure[aria-label*="sub-thread"]`).first();
        await toggle.click();
        await expect(parentRow).not.toHaveClass(/thread-row-focused/);

        // Body click on the parent's title opens the thread → focused class.
        await parentRow.locator('.thread-row-title').click();
        await expect(parentRow).toHaveClass(/thread-row-focused/);
    });
});
