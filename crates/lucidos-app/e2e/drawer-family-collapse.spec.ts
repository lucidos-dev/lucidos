import { test, expect } from '@playwright/test';
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

/** Seed a 3-level chain (grandparent → parent → child) directly in
 *  `thread_summaries`. Renders two family-toggle headers (one under each
 *  ancestor) — the canonical fixture for depth-related geometry tests. */
function seedGrandparentParentChild(): { grandparentId: string; parentId: string; childId: string } {
    const grandparentId = randomUUID();
    const parentId = randomUUID();
    const childId = randomUUID();
    const now = new Date().toISOString();
    const stamp = Date.now();
    psql([
        seedThreadRow({ id: grandparentId, title: `grand-${stamp}`, totalChildren: 1, now }),
        seedThreadRow({ id: parentId, title: `parent-${stamp}`, parentId: grandparentId, totalChildren: 1, now }),
        seedThreadRow({ id: childId, title: `child-${stamp}`, parentId, now }),
    ].join(';\n'));
    return { grandparentId, parentId, childId };
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

        // Family toggle row appears under the parent (one per family head).
        // The toggle button carries aria-expanded so we can probe state.
        const toggle = page.locator(`.family-toggle[aria-label*="sub-thread"]`).first();
        await expect(toggle).toBeVisible();
        await expect(toggle).toHaveAttribute('aria-expanded', 'true');

        // Click the toggle — child hides; parent stays.
        await toggle.click();
        await expect(childRow.first()).toBeHidden();
        await expect(parentRow.first()).toBeVisible();
        await expect(toggle).toHaveAttribute('aria-expanded', 'false');

        // Reload — collapsed state survives via localStorage.
        await page.reload();
        await openThreadDrawer(page);
        await expect(parentRow.first()).toBeVisible();
        await expect(childRow.first()).toBeHidden();
        const toggleAfterReload = page.locator(`.family-toggle[aria-label*="sub-thread"]`).first();
        await expect(toggleAfterReload).toHaveAttribute('aria-expanded', 'false');

        // Re-expand — child returns.
        await toggleAfterReload.click();
        await expect(childRow.first()).toBeVisible();
        await expect(toggleAfterReload).toHaveAttribute('aria-expanded', 'true');
    });

    test('family-toggle row indents to match its children', async ({ page }) => {
        // Header's wrap starts at the same x as the children's visible (clip-
        // path) left edge below — the tinted band reads as the children's
        // header, not the parent's. Anchored to the children themselves (via
        // their rail-x ::before) so no rem-to-px hardcoding is needed.
        // Seed 3 levels so both depth=1 and depth=2 toggles are present.
        const { childId } = seedGrandparentParentChild();

        await navigateToApp(page);
        await openThreadDrawer(page);

        // Anchor on the deepest descendant — if depth=2 rendered, every
        // ancestor + both family-toggle rows are guaranteed in the DOM too.
        // `:visible` scopes to the active layout (desktop or mobile copy).
        const childRow = page.locator(`.thread-row[data-thread-nav="${childId}"]:visible`).first();
        await expect(childRow).toBeVisible();

        const wraps = page.locator(`.family-toggle-wrap:visible`);
        await expect(wraps).toHaveCount(2);

        // For each header, read its viewport-left and the absolute x of its
        // OWN first child's rail (`::before` left on the row inside the next
        // sibling wrap) — they must match. Walking via nextElementSibling
        // pairs each header with the wrap it actually heads, regardless of
        // DOM order between sections.
        const readPair = (el: ReturnType<typeof page.locator>) =>
            el.evaluate(node => {
                const nextWrap = node.nextElementSibling as HTMLElement | null;
                const firstChild = nextWrap?.querySelector<HTMLElement>('.thread-row');
                if (!firstChild) throw new Error('header has no following child row');
                const rowBefore = getComputedStyle(firstChild, '::before');
                return {
                    wrapLeft: node.getBoundingClientRect().left,
                    childRailLeft: firstChild.getBoundingClientRect().left + parseFloat(rowBefore.left),
                };
            });

        const [first, second] = await Promise.all([readPair(wraps.nth(0)), readPair(wraps.nth(1))]);

        // Tolerance 0 (0.5px) accommodates cross-viewport sub-pixel rounding
        // — the bug we guard against is whole-step offsets (8–16px), not
        // fractional pixel drift.
        expect(first.wrapLeft).toBeCloseTo(first.childRailLeft, 0);
        expect(second.wrapLeft).toBeCloseTo(second.childRailLeft, 0);
    });

    test('family-toggle header has no vertical rail and the bottom divider matches the header width', async ({ page }) => {
        // Two asserted properties of the family-toggle row's geometry:
        //  1. The header has NO vertical rail on its left edge — the spine
        //     starts at the first child row below, not on the header itself.
        //  2. The horizontal divider below the header starts at the header's
        //     own left edge (not full-width drawer-left).
        const { parentId, childId } = seedGrandparentParentChild();

        await navigateToApp(page);
        await openThreadDrawer(page);

        const childRow = page.locator(`.thread-row[data-thread-nav="${childId}"]:visible`).first();
        const parentRow = page.locator(`.thread-row[data-thread-nav="${parentId}"]:visible`).first();
        await expect(childRow).toBeVisible();

        // Top-level header is the first .family-toggle-wrap (under grandparent
        // at depth 0); nested header is the second (under parent at depth 1).
        const headers = page.locator(`.family-toggle-wrap:visible`);
        await expect(headers).toHaveCount(2);
        const topHeader = headers.nth(0);
        const nestedHeader = headers.nth(1);

        // One CDP round-trip per header reads ::after's computed `content`
        // and the wrap's viewport-left.
        const readHeader = (el: ReturnType<typeof page.locator>) =>
            el.evaluate(node => {
                const after = getComputedStyle(node, '::after');
                return {
                    railContent: after.content,
                    wrapLeft: node.getBoundingClientRect().left,
                };
            });
        // One CDP round-trip per child row reads the divider above its wrap
        // (::before on the wrap) — the wrap divider is what visually delimits
        // the toggle header above.
        const readChildWrap = (el: ReturnType<typeof page.locator>) =>
            el.evaluate(node => {
                const wrap = node.parentElement!;
                const wrapBefore = getComputedStyle(wrap, '::before');
                return {
                    wrapLeft: wrap.getBoundingClientRect().left,
                    dividerLeftPx: parseFloat(wrapBefore.left),
                };
            });

        const [top, nested, parent, child] = await Promise.all([
            readHeader(topHeader),
            readHeader(nestedHeader),
            readChildWrap(parentRow),
            readChildWrap(childRow),
        ]);

        // (1) No rail on the header: assert on `content` (not width/left) so
        // a stray `content: ''` declaration fails here even if width happens
        // to be 0. `none` and `normal` both mean the pseudo did not render.
        expect(['none', 'normal']).toContain(top.railContent);
        expect(['none', 'normal']).toContain(nested.railContent);

        // (2) Divider below header = header width: absolute divider start of
        // the first child wrap equals the header wrap's left.
        expect(parent.wrapLeft + parent.dividerLeftPx).toBeCloseTo(top.wrapLeft, 1);
        expect(child.wrapLeft + child.dividerLeftPx).toBeCloseTo(nested.wrapLeft, 1);
    });

    test('shallower-nested row after deeper family draws its own top divider (no horizontal gap)', async ({ page }) => {
        // Repro of the divider-gap bug: a depth-1 parent that itself has
        // children, followed by a depth-1 sibling. With the previous
        // "wrapper draws bottom divider at its own rail-x" model, the
        // divider between the deeper (depth-2) last child and the depth-1
        // sibling was inset to depth-2's rail-x, leaving a horizontal gap
        // between depth-1's content edge and the divider. Each row owning
        // the divider ABOVE itself, drawn at its own depth's rail-x, kills
        // the gap.
        const grandparentId = randomUUID();
        const parent1Id = randomUUID();
        const childId = randomUUID();
        const parent2Id = randomUUID();
        const now = new Date().toISOString();
        const stamp = Date.now();
        psql([
            seedThreadRow({ id: grandparentId, title: `grand-${stamp}`, totalChildren: 2, now }),
            seedThreadRow({ id: parent1Id, title: `p1-${stamp}`, parentId: grandparentId, totalChildren: 1, now }),
            seedThreadRow({ id: childId, title: `child-${stamp}`, parentId: parent1Id, now }),
            seedThreadRow({ id: parent2Id, title: `p2-${stamp}`, parentId: grandparentId, now }),
        ].join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);

        const parent2Row = page.locator(`.thread-row[data-thread-nav="${parent2Id}"]:visible`).first();
        await expect(parent2Row).toBeVisible();

        // parent2's wrapper owns the divider above itself via ::before. Read
        // the pseudo's geometry and parent2's own padding-left to verify the
        // divider doesn't begin past parent2's content edge.
        const divider = await parent2Row.evaluate(rowEl => {
            const wrap = rowEl.parentElement!;
            const before = getComputedStyle(wrap, '::before');
            return {
                content: before.content,
                heightPx: parseFloat(before.height),
                topPx: parseFloat(before.top),
                leftPx: parseFloat(before.left),
                rowPaddingLeftPx: parseFloat(getComputedStyle(rowEl).paddingLeft),
            };
        });

        // ::before exists and is 1px tall at the wrapper's top edge.
        expect(divider.content).not.toBe('none');
        expect(divider.content).not.toBe('normal');
        expect(divider.heightPx).toBe(1);
        expect(divider.topPx).toBe(0);
        // Critical: the divider must start at or before parent2's content
        // edge. Before the fix there was no ::before on parent2's wrapper at
        // all — the previous (depth-2) wrapper's divider, inset to depth-2's
        // rail-x, was the only line between the rows.
        expect(divider.leftPx).toBeLessThanOrEqual(divider.rowPaddingLeftPx);
    });

    test('toggle-row click does not focus the parent thread; row-body click does', async ({ page }) => {
        const parentTitle = `body-parent-${Date.now()}`;
        const childTitle = `body-child-${Date.now()}`;
        const { parentId } = seedParentChild(parentTitle, childTitle);

        await navigateToApp(page);
        await openThreadDrawer(page);

        const parentRow = page.locator(`.thread-row[data-thread-nav="${parentId}"]`).first();
        await expect(parentRow).toBeVisible();

        // Toggle-row click leaves focus alone — the parent doesn't gain the
        // focused class because the toggle row is a sibling, not part of the
        // parent's onClick.
        const toggle = page.locator(`.family-toggle[aria-label*="sub-thread"]`).first();
        await toggle.click();
        await expect(parentRow).not.toHaveClass(/thread-row-focused/);

        // Body click on the parent's title opens the thread → focused class.
        await parentRow.locator('.thread-row-title').click();
        await expect(parentRow).toHaveClass(/thread-row-focused/);
    });
});
