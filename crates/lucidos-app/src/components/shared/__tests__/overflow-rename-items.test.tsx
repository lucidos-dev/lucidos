// @vitest-environment jsdom
/**
 * The thread ⋯ menu is the one place to rename, since the title is
 * display-only. It offers Rename… on a sent thread, home included. A draft is
 * titled by its compose text, so it offers none. Suggesting a name lives in
 * the rename dialog, never as a menu item of its own.
 */
import { beforeAll, describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/threadRename', async (importOriginal) => ({
    ...(await importOriginal<typeof import('../../../store/actions/threadRename')>()),
    promptRenameThread: vi.fn(async () => {}),
}));

import { ThreadOverflowItemsChunk, ThreadOverflowMenu } from '../ThreadOverflowMenu';
import { promptRenameThread } from '../../../store/actions/threadRename';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

// The menu's rows are code-split. Warm them, so a click opens it at once.
beforeAll(() => ThreadOverflowItemsChunk.preload());

let host: HTMLElement | null = null;

function openMenu(): HTMLElement[] {
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<ThreadOverflowMenu threadId="t" title="T" />, host!); });
    const trigger = host.querySelector<HTMLElement>('button[aria-haspopup="menu"]');
    act(() => { trigger?.click(); });
    return [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')];
}

const item = (items: HTMLElement[], label: string) => items.find(el => el.textContent === label);

beforeEach(() => {
    vi.clearAllMocks();
});

afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    document.querySelectorAll('.thread-overflow-menu').forEach(el => el.remove());
    threadMap.value = new Map();
});

describe('Rename items in the thread overflow menu', () => {
    it('offers Rename… on a sent thread and runs it, with no Suggest item', () => {
        threadMap.value = new Map([['t', makeThreadState('t')]]);
        const items = openMenu();
        expect(items.map(el => el.textContent)).not.toContain('Suggest name');
        const rename = item(items, 'Rename…');
        expect(rename).toBeTruthy();
        act(() => { rename!.click(); });
        expect(promptRenameThread).toHaveBeenCalledWith('t');
    });

    it('offers Rename… on the home thread', () => {
        threadMap.value = new Map([['t', makeThreadState('t', { meta: { home: true } })]]);
        expect(openMenu().map(el => el.textContent)).toContain('Rename…');
    });

    it('offers no rename on a draft', () => {
        threadMap.value = new Map([['t', makeThreadState('t', { meta: { state: 'composing' } })]]);
        const labels = openMenu().map(el => el.textContent);
        expect(labels).not.toContain('Rename…');
    });

    it('offers no rename for a thread that is not loaded (a search hit)', () => {
        const labels = openMenu().map(el => el.textContent);
        expect(labels).not.toContain('Rename…');
    });
});
