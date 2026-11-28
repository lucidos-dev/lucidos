import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

test.use({ reducedMotion: 'no-preference' });

interface ToggleFrame {
    headerBottom: number;
    nextHeaderTop: number;
    blockBottom: number | null;
    maskTop: number | null;
    ghosts: number;
    viewportBottom: number;
    divider: 'none' | 'hidden' | 'visible';
    carriedBottom: number | null;
}

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
        const pinnedIds = Array.from({ length: 30 }, () => randomUUID());
        const archivedId = randomUUID();
        const now = new Date().toISOString();
        psql([
            ...pinnedIds.map((id, i) => seedThreadRow({ id, title: `pinned-${i}-${stamp}`, now })),
            seedThreadRow({ id: archivedId, title: `archived-${stamp}`, now }),
            `UPDATE thread_summaries SET is_saved = true, archive_state = 'inbox' WHERE thread_id IN ('${pinnedIds.join("','")}')`,
        ].join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        const pinnedRow = page.locator('[data-flip-id="__section_saved"] ~ .drawer-section-rows .thread-row').first();
        await expect(pinnedRow).toBeVisible();
        await expect(page.locator('[data-flip-id="__section_archive"]')).toBeVisible();

        const nextHeaderGap = () => page.evaluate(() => {
            const header = document.querySelector('[data-flip-id="__section_saved"]')!;
            const rows = header.parentElement!.querySelector('.drawer-section-rows');
            const blockBottom = (rows ?? header).getBoundingClientRect().bottom;
            return document.querySelector('[data-flip-id="__section_archive"]')!.getBoundingClientRect().top - blockBottom;
        });
        const openGap = await nextHeaderGap();

        const collapse = await recordToggle(page, pinnedIds);
        await expect(pinnedRow).toBeHidden();
        expect(collapse[0].nextHeaderTop).toBeLessThan(collapse[0].viewportBottom + 150);
        expect(Math.max(...collapse.map(f => f.ghosts))).toBeLessThan(pinnedIds.length);
        for (const f of collapse.filter(f => f.ghosts > 0)) expect(f.divider).toBe('hidden');
        expect(collapse[collapse.length - 1].divider).toBe('visible');
        const shutGap = await nextHeaderGap();
        const expand = await recordToggle(page, pinnedIds);
        await expect(pinnedRow).toBeVisible();
        for (const f of expand.filter(f => f.ghosts > 0)) expect(f.divider).toBe('none');

        const slack = (frames: ToggleFrame[], i: number) => {
            const cur = frames[i].blockBottom;
            const step = (other: number | null | undefined) => (other != null && cur != null ? Math.abs(cur - other) : 0);
            return 2 + Math.max(step(frames[i - 1]?.blockBottom), step(frames[i + 1]?.blockBottom));
        };
        const tracked = (f: ToggleFrame) =>
            f.ghosts > 0 && f.blockBottom !== null && f.nextHeaderTop < f.viewportBottom;

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
            `UPDATE thread_summaries SET is_saved = true, archive_state = 'inbox' WHERE thread_id IN ('${parentId}','${childId}')`,
        ].join(';\n'));

        await navigateToApp(page);
        await openThreadDrawer(page);
        const child = page.locator(`.thread-row[data-thread-nav="${childId}"]`);
        await expect(child).toBeVisible();
        const lineLeft = (el: Element) => el.getBoundingClientRect().left + parseFloat(getComputedStyle(el, '::after').left);
        const rowLineLeft = await child.evaluate(lineLeft);

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
