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

/** One animation frame of a family toggle: where the parent ends, where the
 *  next family starts, and where the sub-threads (real or ghost) end. */
interface ToggleFrame {
    parentBottom: number;
    siblingTop: number;
    blockBottom: number | null;
    maskTop: number | null;
    ghosts: number;
    childLookups: number;
}

/** Click the family toggle, then let `frames` animation frames pass. */
async function toggleFor(page: import('@playwright/test').Page, parentId: string, frames: number): Promise<void> {
    await page.evaluate(async ({ parentId, frames }) => {
        document.querySelector<HTMLElement>(`.thread-row[data-thread-nav="${parentId}"] .family-disclosure`)!.click();
        for (let i = 0; i < frames; i++) await new Promise(resolve => requestAnimationFrame(resolve));
    }, { parentId, frames });
}

/** Click the family toggle and record every frame until the motion settles. */
async function recordToggle(
    page: import('@playwright/test').Page,
    ids: { parentId: string; childIds: string[]; siblingId: string },
): Promise<ToggleFrame[]> {
    return page.evaluate(async ({ parentId, childIds, siblingId }) => {
        const wrap = (id: string) => document.querySelector<HTMLElement>(`[data-flip-id="${id}"]`);
        const ghostSelector = '.drawer-section > .flip-disclosure-mask > *';
        document.querySelector<HTMLElement>(`.thread-row[data-thread-nav="${parentId}"] .family-disclosure`)!.click();
        const frames: ToggleFrame[] = [];
        const start = performance.now();
        while (performance.now() - start < 1000) {
            await new Promise(resolve => requestAnimationFrame(resolve));
            // While copies slide, the real rows wait invisible in place.
            const ghosts = [...document.querySelectorAll<HTMLElement>(ghostSelector)];
            const block = (ghosts.length > 0 ? ghosts : childIds.map(wrap).filter((el): el is HTMLElement => !!el))
                .map(el => el.getBoundingClientRect().bottom);
            const masks = [...document.querySelectorAll<HTMLElement>('.flip-disclosure-mask')]
                .map(el => el.getBoundingClientRect().top);
            frames.push({
                parentBottom: wrap(parentId)!.getBoundingClientRect().bottom,
                siblingTop: wrap(siblingId)!.getBoundingClientRect().top,
                blockBottom: block.length > 0 ? Math.max(...block) : null,
                maskTop: masks.length > 0 ? Math.min(...masks) : null,
                ghosts: document.querySelectorAll(ghostSelector).length,
                childLookups: childIds
                    .map(id => document.querySelectorAll(`[data-thread-nav="${id}"]`).length)
                    .reduce((a, b) => a + b, 0),
            });
        }
        return frames;
    }, ids);
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

        // The toggle sits on the parent row (one per family head), and its
        // aria-expanded reports the state.
        const toggle = parentRow.locator('.family-disclosure');
        await expect(toggle).toBeVisible();
        await expect(toggle).toHaveAttribute('aria-expanded', 'true');
        await expect(toggle).toHaveText('Hide 1 sub-thread');

        // Click the toggle: the child hides and the parent stays.
        await toggle.click();
        await expect(childRow.first()).toBeHidden();
        await expect(parentRow.first()).toBeVisible();
        await expect(toggle).toHaveAttribute('aria-expanded', 'false');
        await expect(toggle).toHaveText('Show 1 sub-thread');

        // Reload: the collapsed state survives via localStorage.
        await page.reload();
        await openThreadDrawer(page);
        await expect(parentRow.first()).toBeVisible();
        await expect(childRow.first()).toBeHidden();
        await expect(toggle).toHaveAttribute('aria-expanded', 'false');
        await expect(toggle).toHaveText('Show 1 sub-thread');

        // Re-expand: the child returns.
        await toggle.click();
        await expect(childRow.first()).toBeVisible();
        await expect(toggle).toHaveAttribute('aria-expanded', 'true');
        await expect(toggle).toHaveText('Hide 1 sub-thread');
    });

    test('sub-threads unroll and roll up in lockstep with the rows below', async ({ page }) => {
        const stamp = Date.now();
        const parentId = randomUUID();
        const childIds = [randomUUID(), randomUUID()];
        const siblingId = randomUUID();
        const now = new Date().toISOString();
        const earlier = new Date(stamp - 3_600_000).toISOString();
        psql([
            seedThreadRow({ id: parentId, title: `motion-parent-${stamp}`, totalChildren: 2, now }),
            ...childIds.map((id, i) => seedThreadRow({ id, title: `motion-child-${i}-${stamp}`, parentId, now })),
            seedThreadRow({ id: siblingId, title: `motion-sibling-${stamp}`, now: earlier }),
        ].join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        const childRow = page.locator(`.thread-row[data-thread-nav="${childIds[1]}"]`);
        await expect(childRow).toBeVisible();

        // The gap between the family's lowest row and the next family, at rest.
        const restGap = await page.evaluate(({ childIds, siblingId }) => {
            const rect = (id: string) => document.querySelector(`[data-flip-id="${id}"]`)!.getBoundingClientRect();
            return rect(siblingId).top - Math.max(...childIds.map(id => rect(id).bottom));
        }, { childIds, siblingId });

        // In both directions the next family never rides over the parent, it
        // tracks the bottom of the moving block, and it really did move.
        const expectLockstep = (frames: ToggleFrame[]) => {
            for (const f of frames) {
                expect(f.siblingTop).toBeGreaterThanOrEqual(f.parentBottom - 1);
                // The mask clips the copies, so none can paint over the parent.
                if (f.maskTop !== null) expect(f.maskTop).toBeGreaterThanOrEqual(f.parentBottom - 1);
                if (f.blockBottom !== null) {
                    expect(Math.abs(f.siblingTop - f.blockBottom - restGap)).toBeLessThanOrEqual(2);
                }
            }
            const first = frames[0].siblingTop;
            const last = frames[frames.length - 1].siblingTop;
            expect(Math.abs(first - last)).toBeGreaterThan(5);
        };

        // Collapse: the sub-threads roll up as ghosts, and no thread lookup
        // ever lands on a ghost.
        const collapse = await recordToggle(page, { parentId, childIds, siblingId });
        expectLockstep(collapse);
        expect(collapse.some(f => f.ghosts > 0)).toBe(true);
        expect(collapse.every(f => f.childLookups === 0)).toBe(true);
        expect(collapse[collapse.length - 1].ghosts).toBe(0);

        // Expand: the sub-threads unroll from under the parent.
        const expand = await recordToggle(page, { parentId, childIds, siblingId });
        expectLockstep(expand);
        expect(expand.some(f => f.ghosts > 0)).toBe(true);
        expect(expand[expand.length - 1].ghosts).toBe(0);
        await expect(childRow).toBeVisible();

        // Interrupted: a toggle landing mid-motion starts from where the rows
        // are, so the lockstep holds on every frame in both directions.
        await toggleFor(page, parentId, 60); // settled shut
        await toggleFor(page, parentId, 4); // showing
        expectLockstep(await recordToggle(page, { parentId, childIds, siblingId })); // hide mid-show
        await toggleFor(page, parentId, 60); // settled open
        await toggleFor(page, parentId, 4); // hiding
        expectLockstep(await recordToggle(page, { parentId, childIds, siblingId })); // show mid-hide
        await expect(childRow).toBeVisible();
    });

    test('toggling another family mid-motion never slides rows over their parent', async ({ page }) => {
        const stamp = Date.now();
        const parentA = randomUUID();
        const childrenA = [randomUUID(), randomUUID()];
        const parentB = randomUUID();
        const childB = randomUUID();
        const now = new Date().toISOString();
        const earlier = new Date(stamp - 3_600_000).toISOString();
        psql([
            seedThreadRow({ id: parentA, title: `family-a-${stamp}`, totalChildren: 2, now }),
            ...childrenA.map((id, i) => seedThreadRow({ id, title: `family-a-child-${i}-${stamp}`, parentId: parentA, now })),
            seedThreadRow({ id: parentB, title: `family-b-${stamp}`, totalChildren: 1, now: earlier }),
            seedThreadRow({ id: childB, title: `family-b-child-${stamp}`, parentId: parentB, now: earlier }),
        ].join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        await expect(page.locator(`.thread-row[data-thread-nav="${childB}"]`)).toBeVisible();

        // Shut family A, start showing it, then toggle family B mid-motion.
        await toggleFor(page, parentA, 60);
        await toggleFor(page, parentA, 4);
        const tops = await page.evaluate(async ({ parentA, childrenA, parentB }) => {
            const rect = (id: string) => document.querySelector(`[data-flip-id="${id}"]`)!.getBoundingClientRect();
            document.querySelector<HTMLElement>(`.thread-row[data-thread-nav="${parentB}"] .family-disclosure`)!.click();
            const frames: { parentBottom: number; childTop: number }[] = [];
            const start = performance.now();
            while (performance.now() - start < 800) {
                await new Promise(resolve => requestAnimationFrame(resolve));
                frames.push({ parentBottom: rect(parentA).bottom, childTop: Math.min(...childrenA.map(id => rect(id).top)) });
            }
            return frames;
        }, { parentA, childrenA, parentB });
        for (const f of tops) expect(f.childTop).toBeGreaterThanOrEqual(f.parentBottom - 1);
    });

    test('a long parent title never overlaps the toggle', async ({ page }) => {
        // Seed a deliberately long title so it wraps at every project width.
        const longTitle = `Diagnosing Interrupted Response and Memory Issues Across Long Running Sessions ${Date.now()}`;
        const childTitle = `child-${Date.now()}`;
        const { parentId } = seedParentChild(longTitle, childTitle);

        await navigateToApp(page);
        await openThreadDrawer(page);

        const parentRow = page.locator(`.thread-row[data-thread-nav="${parentId}"]`).first();
        await expect(parentRow).toBeVisible();

        const toggle = parentRow.locator('.family-disclosure');
        await toggle.click();
        await expect(toggle).toHaveAttribute('aria-expanded', 'false');

        const titleBox = await parentRow.locator('.thread-row-title').boundingBox();
        const toggleBox = await toggle.boundingBox();
        expect(titleBox).not.toBeNull();
        expect(toggleBox).not.toBeNull();

        // The title actually wrapped (more than one line) — otherwise the test
        // isn't exercising the overlap case.
        expect(titleBox!.height).toBeGreaterThan(30);

        // No overlap: the two rectangles must not intersect.
        const overlaps =
            titleBox!.x < toggleBox!.x + toggleBox!.width &&
            titleBox!.x + titleBox!.width > toggleBox!.x &&
            titleBox!.y < toggleBox!.y + toggleBox!.height &&
            titleBox!.y + titleBox!.height > toggleBox!.y;
        expect(overlaps).toBe(false);
    });

    test('toggle click does not focus the parent thread; row-body click does', async ({ page }) => {
        const parentTitle = `body-parent-${Date.now()}`;
        const childTitle = `body-child-${Date.now()}`;
        const { parentId } = seedParentChild(parentTitle, childTitle);

        await navigateToApp(page);
        await openThreadDrawer(page);

        const parentRow = page.locator(`.thread-row[data-thread-nav="${parentId}"]`).first();
        await expect(parentRow).toBeVisible();

        // A toggle click leaves focus alone: the button lives inside the parent
        // row but stopPropagation()s the click, so the row's own onClick
        // (focusThread) never fires.
        const toggle = parentRow.locator('.family-disclosure');
        await toggle.click();
        await expect(parentRow).not.toHaveClass(/thread-row-focused/);

        // Body click on the parent's title opens the thread → focused class.
        await parentRow.locator('.thread-row-title').click();
        await expect(parentRow).toHaveClass(/thread-row-focused/);
    });
});
