import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

const ARCHIVE_WINDOW = 30;

const INSIDE_WINDOW = 2;

const BELOW_WINDOW = 3;

function seedFilteredArchive(): { newest: string; oldest: string } {
    const titles: string[] = [];
    const inserts: string[] = [];
    let minute = 0;
    const stamp = () => new Date(Date.now() - (minute++) * 60_000).toISOString();

    for (let i = 0; i < INSIDE_WINDOW; i++) {
        const title = `chat-in-window-${i}-${randomUUID().slice(0, 8)}`;
        titles.push(title);
        inserts.push(seedThreadRow({ id: randomUUID(), title, now: stamp() }));
    }
    for (let i = 0; i < ARCHIVE_WINDOW; i++) {
        inserts.push(seedThreadRow({
            id: randomUUID(), title: `trigger-filler-${i}`, now: stamp(), source: 'trigger',
        }));
    }
    for (let i = 0; i < BELOW_WINDOW; i++) {
        const title = `chat-below-window-${i}-${randomUUID().slice(0, 8)}`;
        titles.push(title);
        inserts.push(seedThreadRow({ id: randomUUID(), title, now: stamp() }));
    }
    psql(inserts.join(';\n'));
    return { newest: titles[0], oldest: titles[titles.length - 1] };
}

test.describe('Drawer archive pagination', () => {
    test.beforeEach(async ({ page, context }) => {
        await assertHealthy(page);
        await context.clearCookies();
        await context.addInitScript(() => {
            localStorage.setItem('lucidos-drawer-collapsed', JSON.stringify(['archive']));
            localStorage.setItem('lucidos-thread-channel-filter', JSON.stringify(['chat']));
        });
        clearAllThreads();
    });

    test('expanding Archive reaches past the initial window on a list too short to scroll', async ({ page }) => {
        const { newest, oldest } = seedFilteredArchive();

        await navigateToApp(page);
        await openThreadDrawer(page);

        const header = page.locator('.thread-drawer .list-section-title-collapsible')
            .filter({ hasText: 'Archive' }).first();
        await expect(header).toBeVisible();
        await expect(header).toHaveAttribute('aria-expanded', 'false');
        await expect(header.locator('.section-count-badge'))
            .toHaveText(String(INSIDE_WINDOW + BELOW_WINDOW));

        await header.click();
        await expect(header).toHaveAttribute('aria-expanded', 'true');

        await expect(page.locator('.thread-row-title', { hasText: newest }).first()).toBeVisible();

        await expect(page.locator('.thread-row-title', { hasText: oldest }).first())
            .toBeVisible({ timeout: 15_000 });
        await expect(page.locator('.thread-drawer .thread-row')).toHaveCount(INSIDE_WINDOW + BELOW_WINDOW);
    });
});
