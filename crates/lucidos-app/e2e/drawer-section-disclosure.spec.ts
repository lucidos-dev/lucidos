import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** One animation frame of a section toggle. */
interface ToggleFrame {
    headerBottom: number;
    nextHeaderTop: number;
    blockBottom: number | null;
    maskTop: number | null;
    ghosts: number;
    viewportBottom: number;
    /** The collapsed divider under the header: absent, waiting, or drawn. */
    divider: 'none' | 'hidden' | 'visible';
    /** Bottom edge of the line carried on the rolling block, while it exists. */
    carriedBottom: number | null;
}

/** Click the Pinned header and record every frame until the motion settles. */
async function recordToggle(page: import('@playwright/test').Page, threadIds: string[]): Promise<ToggleFrame[]> {
    return page.evaluate(async (threadIds) => {
        const wrap = (id: string) => document.querySelector<HTMLElement>(`[data-flip-id="${id}"]`);
        const ghostSelector = '.drawer-section > .flip-disclosure-mask > *';
        wrap('__section_saved')!.click();
        const frames: ToggleFrame[] = [];
        const start = performance.now();
        while (performance.now() - start < 1000) {
            await new Promise(resolve => requestAnimationFrame(resolve));
            const ghosts = [...document.querySelectorAll<HTMLElement>(ghostSelector)];
            const block = (ghosts.length > 0 ? ghosts : threadIds.map(wrap).filter((el): el is HTMLElement => !!el))
                .map(el => el.getBoundingClientRect().bottom);
            const masks = [...document.querySelectorAll<HTMLElement>('.flip-disclosure-mask')]
                .map(el => el.getBoundingClientRect().top);
            frames.push({
                headerBottom: wrap('__section_saved')!.getBoundingClientRect().bottom,
                nextHeaderTop: wrap('__section_archive')!.getBoundingClientRect().top,
                blockBottom: block.length > 0 ? Math.max(...block) : null,
                maskTop: masks.length > 0 ? Math.min(...masks) : null,
                ghosts: ghosts.length,
                viewportBottom: window.innerHeight,
                carriedBottom: document.querySelector('.drawer-section > .flip-disclosure-hairline')
                    ?.getBoundingClientRect().bottom ?? null,
                // Read last: a pseudo-element style read can move WebKit's
                // animations on, and every box above must share one moment.
                divider: (() => {
                    const after = getComputedStyle(wrap('__section_saved')!, '::after');
                    if (after.content === 'none') return 'none' as const;
                    return after.visibility === 'hidden' ? 'hidden' as const : 'visible' as const;
                })(),
            });
        }
        return frames;
    }, threadIds);
}

test.describe('Drawer section disclosure', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.clearCookies();
        await context.addInitScript(() => {
            localStorage.removeItem('lucidos-drawer-collapsed');
        });
        clearAllThreads();
    });

    test('a section\'s threads unroll and roll up under its header, like sub-threads', async ({ page }) => {
        const stamp = Date.now();
        // Taller than the viewport, so the roll is capped at the screen and
        // the rows past the cut stay out of the motion.
        const pinnedIds = Array.from({ length: 30 }, () => randomUUID());
        const archivedId = randomUUID();
        const now = new Date().toISOString();
        psql([
            ...pinnedIds.map((id, i) => seedThreadRow({ id, title: `pinned-${i}-${stamp}`, now })),
            seedThreadRow({ id: archivedId, title: `archived-${stamp}`, now }),
            `UPDATE thread_summaries SET is_saved = true WHERE thread_id IN ('${pinnedIds.join("','")}')`,
        ].join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        const pinnedRow = page.locator(`.thread-row[data-thread-nav="${pinnedIds[1]}"]`);
        await expect(pinnedRow).toBeVisible();
        await expect(page.locator(`.thread-row[data-thread-nav="${archivedId}"]`)).toBeVisible();

        // The gap under the moving block, open and shut. The next header must
        // stay inside that range on every frame.
        const nextHeaderGap = () => page.evaluate((pinnedIds) => {
            const rect = (id: string) => document.querySelector(`[data-flip-id="${id}"]`)?.getBoundingClientRect();
            const rows = pinnedIds.map(rect).filter((r): r is DOMRect => !!r);
            const blockBottom = rows.length > 0 ? Math.max(...rows.map(r => r.bottom)) : rect('__section_saved')!.bottom;
            return rect('__section_archive')!.top - blockBottom;
        }, pinnedIds);
        const openGap = await nextHeaderGap();

        const collapse = await recordToggle(page, pinnedIds);
        await expect(pinnedRow).toBeHidden();
        // The section is taller than the screen, so it rolls only one screen:
        // the next header starts just below the fold, and rows past the cut
        // get no copy.
        expect(collapse[0].nextHeaderTop).toBeLessThan(collapse[0].viewportBottom + 150);
        expect(Math.max(...collapse.map(f => f.ghosts))).toBeLessThan(pinnedIds.length);
        // While the rows roll up, a carried line stands in for the divider,
        // and the divider takes over once it lands.
        for (const f of collapse.filter(f => f.ghosts > 0)) expect(f.divider).toBe('hidden');
        expect(collapse[collapse.length - 1].divider).toBe('visible');
        const shutGap = await nextHeaderGap();
        const expand = await recordToggle(page, pinnedIds);
        await expect(pinnedRow).toBeVisible();
        // Unrolling, the carried line takes the divider's place and rides down.
        for (const f of expand.filter(f => f.ghosts > 0)) expect(f.divider).toBe('none');

        // Mobile WebKit can read one element's box a frame apart from the
        // rest, early or late, so a check allows the block's larger step to
        // either neighbouring frame. Real drift grows past that; a one-frame
        // read skew does not.
        const slack = (frames: ToggleFrame[], i: number) => {
            const cur = frames[i].blockBottom;
            const step = (other: number | null | undefined) => (other != null && cur != null ? Math.abs(cur - other) : 0);
            return 2 + Math.max(step(frames[i - 1]?.blockBottom), step(frames[i + 1]?.blockBottom));
        };
        const tracked = (f: ToggleFrame) =>
            f.ghosts > 0 && f.blockBottom !== null && f.nextHeaderTop < f.viewportBottom;

        // The carried line rides the block's bottom edge and never rises above
        // the header, where the divider sits.
        for (const frames of [collapse, expand]) {
            expect(frames.some(f => f.carriedBottom !== null)).toBe(true);
            expect(frames[frames.length - 1].carriedBottom).toBeNull();
            frames.forEach((f, i) => {
                if (f.carriedBottom === null) return;
                expect(f.carriedBottom).toBeGreaterThanOrEqual(f.headerBottom - 1);
                if (tracked(f)) {
                    expect(Math.abs(f.carriedBottom - f.blockBottom!), JSON.stringify({ i, f }))
                        .toBeLessThanOrEqual(slack(frames, i));
                }
            });
        }

        // The next header never rides over the section header, and while it
        // is on screen it tracks the bottom of the moving block. Below the
        // fold there is nothing to see, so no copy exists to measure.
        for (const frames of [collapse, expand]) {
            frames.forEach((f, i) => {
                expect(f.nextHeaderTop).toBeGreaterThanOrEqual(f.headerBottom - 1);
                if (f.maskTop !== null) expect(f.maskTop).toBeGreaterThanOrEqual(f.headerBottom - 1);
                if (tracked(f)) {
                    const gap = f.nextHeaderTop - f.blockBottom!;
                    expect(gap, JSON.stringify({ i, f })).toBeGreaterThanOrEqual(Math.min(openGap, shutGap) - slack(frames, i));
                    expect(gap, JSON.stringify({ i, f })).toBeLessThanOrEqual(Math.max(openGap, shutGap) + slack(frames, i));
                }
            });
            expect(frames.some(f => f.ghosts > 0)).toBe(true);
            expect(frames[frames.length - 1].ghosts).toBe(0);
        }
    });

    test('the carried line narrows to a nested closing row\'s indent', async ({ page }) => {
        const stamp = Date.now();
        const parentId = randomUUID();
        const childId = randomUUID();
        const now = new Date().toISOString();
        psql([
            seedThreadRow({ id: parentId, title: `nest-parent-${stamp}`, totalChildren: 1, now }),
            seedThreadRow({ id: childId, title: `nest-child-${stamp}`, parentId, now }),
            seedThreadRow({ id: randomUUID(), title: `nest-archived-${stamp}`, now }),
            `UPDATE thread_summaries SET is_saved = true WHERE thread_id IN ('${parentId}','${childId}')`,
        ].join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        // The sub-thread closes the Pinned section, so its line is indented.
        const child = page.locator(`.thread-row[data-thread-nav="${childId}"]`);
        await expect(child).toBeVisible();
        const lineLeft = (el: Element) => el.getBoundingClientRect().left + parseFloat(getComputedStyle(el, '::after').left);
        const rowLineLeft = await child.evaluate(lineLeft);

        // The carried line's left edge on every frame it exists.
        const recordLefts = () => page.evaluate(async () => {
            document.querySelector<HTMLElement>('[data-flip-id="__section_saved"]')!.click();
            const lefts: number[] = [];
            const start = performance.now();
            while (performance.now() - start < 1000) {
                await new Promise(resolve => requestAnimationFrame(resolve));
                const line = document.querySelector('.drawer-section > .flip-disclosure-hairline');
                if (line) lefts.push(line.getBoundingClientRect().left);
            }
            return lefts;
        });

        const collapse = await recordLefts();
        await expect(child).toBeHidden();
        const dividerLeft = await page.locator('[data-flip-id="__section_saved"]').evaluate(lineLeft);
        expect(dividerLeft).toBeLessThan(rowLineLeft - 5);
        const expand = await recordLefts();
        await expect(child).toBeVisible();

        // The left edge travels one way between the row's indent and the
        // divider's inset, and finishes on the far end. The first frame read
        // can land well into the motion, so the path is what gets checked.
        const expectPath = (lefts: number[], from: number, to: number) => {
            expect(lefts.length).toBeGreaterThan(0);
            const sign = Math.sign(to - from);
            lefts.forEach((left, i) => {
                expect(left).toBeGreaterThanOrEqual(Math.min(from, to) - 3);
                expect(left).toBeLessThanOrEqual(Math.max(from, to) + 3);
                if (i > 0) expect((left - lefts[i - 1]) * sign).toBeGreaterThanOrEqual(-1);
            });
            expect(Math.abs(lefts[lefts.length - 1] - to)).toBeLessThanOrEqual(3);
        };
        expectPath(collapse, rowLineLeft, dividerLeft);
        expectPath(expand, dividerLeft, rowLineLeft);
    });

    test('the section count badge shows whether the section is open or shut', async ({ page }) => {
        const stamp = Date.now();
        const ids = [randomUUID(), randomUUID()];
        const now = new Date().toISOString();
        psql(ids.map((id, i) => seedThreadRow({ id, title: `counted-${i}-${stamp}`, now })).join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        const header = page.locator('.thread-drawer [data-flip-id="__section_archive"]');
        const badge = header.locator('.section-count-badge');
        const open = header.locator('.section-count-open');

        // One copy of the number shows at a time: the bare, larger one while
        // open, and the pill's own while shut.
        await expect(header).toHaveAttribute('aria-expanded', 'true');
        await expect(open).toBeVisible();
        await expect(open).toHaveText('2');
        await expect(badge).toBeHidden();
        await header.click();
        await expect(header).toHaveAttribute('aria-expanded', 'false');
        await expect(badge).toBeVisible();
        await expect(badge).toHaveText('2');
        await expect(open).toBeHidden();
    });
});
