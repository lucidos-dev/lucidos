// @vitest-environment jsdom
/**
 * The thread ⋯ menu is the one place to rename, since the title is
 * display-only. It offers Rename… and Suggest name on a sent thread, and runs
 * each action. A draft is titled by its compose text, so it offers neither.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/threadRename', async (importOriginal) => ({
    ...(await importOriginal<typeof import('../../../store/actions/threadRename')>()),
    promptRenameThread: vi.fn(async () => {}),
    suggestThreadName: vi.fn(async () => {}),
}));

import { ThreadOverflowMenu } from '../ThreadOverflowMenu';
import { promptRenameThread, suggestThreadName } from '../../../store/actions/threadRename';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

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
    it('offers Rename… and Suggest name on a sent thread and runs each', () => {
        threadMap.value = new Map([['t', makeThreadState('t')]]);
        const rename = item(openMenu(), 'Rename…');
        expect(rename).toBeTruthy();
        act(() => { rename!.click(); });
        expect(promptRenameThread).toHaveBeenCalledWith('t');

        const suggest = item(openMenu(), 'Suggest name');
        expect(suggest).toBeTruthy();
        act(() => { suggest!.click(); });
        expect(suggestThreadName).toHaveBeenCalledWith('t');
    });

    it('offers neither on a draft', () => {
        threadMap.value = new Map([['t', makeThreadState('t', { meta: { state: 'composing' } })]]);
        const labels = openMenu().map(el => el.textContent);
        expect(labels).not.toContain('Rename…');
        expect(labels).not.toContain('Suggest name');
    });

    it('offers neither for a thread that is not loaded (a search hit)', () => {
        const labels = openMenu().map(el => el.textContent);
        expect(labels).not.toContain('Rename…');
        expect(labels).not.toContain('Suggest name');
    });
});
