import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, openThreadDrawer, assertHealthy } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** How many archived rows the initial `GET /threads` window carries (the
 *  `get_recent_threads(30)` call in `api/threads/list.rs`). Anything older is
 *  reachable only through `/threads/older` pagination. */
const ARCHIVE_WINDOW = 30;

/** Rows kept inside the window so the Archive section has something to render
 *  while collapsed. A section with no matching row renders nothing at all, and
 *  there would be no header to click. */
const INSIDE_WINDOW = 2;

/** Rows seeded BELOW the window. Only pagination can reach them. */
const BELOW_WINDOW = 3;

/** Seed an archive the drawer's chat filter narrows to a handful of rows. Chat
 *  threads top and bottom, with a wall of trigger threads between them filling
 *  out the initial window.
 *
 *  The trigger wall is what makes the reproduction faithful. The rendered list
 *  has to be SHORTER than the drawer, because that is the case the bug bites
 *  in: no overflow means no scroll, and no scroll means the sentinel never
 *  transitions back into view. */
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
        // Boot with Archive SHUT and the list narrowed to chat. Both are
        // localStorage-backed, and seeding them before the app loads is what
        // makes this a cold boot rather than a filter change: a filter change
        // would fetch a page of its own and hide the stall being tested.
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
        // The badge is the server's own filter-scoped total, so it states up
        // front how many rows the section owes the reader.
        await expect(header.locator('.section-count-badge'))
            .toHaveText(String(INSIDE_WINDOW + BELOW_WINDOW));

        await header.click();
        await expect(header).toHaveAttribute('aria-expanded', 'true');

        // The window's own rows arrive with the expand.
        await expect(page.locator('.thread-row-title', { hasText: newest }).first()).toBeVisible();

        // The rest can only come from pagination, and nothing else is going to
        // ask for it: the list is far shorter than the drawer, so there is no
        // scroll to fire the sentinel with.
        await expect(page.locator('.thread-row-title', { hasText: oldest }).first())
            .toBeVisible({ timeout: 15_000 });
        await expect(page.locator('.thread-drawer .thread-row')).toHaveCount(INSIDE_WINDOW + BELOW_WINDOW);
    });
});
