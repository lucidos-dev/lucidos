// @vitest-environment jsdom
/**
 * Sub-threads can hold Archive and Delete back. The thread ⋯ menu then lists
 * each one by title and state, strongest blocker first, and a tap opens it
 * (ADR 0378). Past `LISTED_SUB_THREADS` it counts the rest.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/threads', async (importOriginal) => ({
    ...(await importOriginal<typeof import('../../../store/actions/threads')>()),
    focusThread: vi.fn(),
}));

import { LISTED_SUB_THREADS, ThreadOverflowMenu } from '../ThreadOverflowMenu';
import { focusThread } from '../../../store/actions/threads';
import { threadMap } from '../../../store/store';
import type { ThreadMeta } from '../../../store/thread-events';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { BLOCKER_REASON, SUB_THREAD_STATE, blockedBySubThreads, moreSubThreads } from '../../../store/actions/blockerCopy';

let host: HTMLElement | null = null;

function openMenu(): HTMLElement {
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<ThreadOverflowMenu threadId="t" title="T" />, host!); });
    const trigger = host.querySelector<HTMLElement>('button[aria-haspopup="menu"]');
    act(() => { trigger?.click(); });
    return document.querySelector<HTMLElement>('.thread-overflow-menu')!;
}

const pending: Partial<ThreadMeta> = { channel: 'claude_code', codingAgentProposed: true };

function sub(id: string, title: string, meta: Partial<ThreadMeta>) {
    return makeThreadState(id, { meta: { parentThreadId: 't', title, section: 'inbox', ...meta } });
}

afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    document.querySelectorAll('.thread-overflow-menu').forEach(el => el.remove());
    threadMap.value = new Map();
    vi.clearAllMocks();
});

describe('Blocking sub-threads in the thread overflow menu', () => {
    it('lists each blocking sub-thread with its state, strongest first, and opens it on a tap', () => {
        threadMap.value = new Map([
            ['t', makeThreadState('t', { meta: { section: 'inbox' } })],
            ['c1', sub('c1', 'Bump the SDK', pending)],
            ['c2', sub('c2', 'Fix login redirect', { status: 'running' })],
        ]);
        const menu = openMenu();

        expect(menu.querySelector('.thread-overflow-note')?.textContent).toBe(blockedBySubThreads(2));
        const rows = [...menu.querySelectorAll<HTMLElement>('.thread-overflow-sub-thread')].map(r => r.closest('button')!);
        expect(rows.map(r => r.textContent)).toEqual([
            `Fix login redirect${SUB_THREAD_STATE.running}`,
            `Bump the SDK${SUB_THREAD_STATE.pending_change}`,
        ]);
        for (const exit of menu.querySelectorAll('.thread-overflow-item-blocked')) {
            expect(exit.querySelector('.visually-hidden')?.textContent?.trim()).toBe(blockedBySubThreads(2));
        }

        act(() => { rows[1].click(); });
        expect(focusThread).toHaveBeenCalledWith('c1');
    });

    it('names the first few and counts the rest', () => {
        const total = LISTED_SUB_THREADS + 2;
        const ids = Array.from({ length: total }, (_, i) => `c${i}`);
        threadMap.value = new Map([
            ['t', makeThreadState('t', { meta: { section: 'inbox' } })],
            ...ids.map(id => [id, sub(id, `Sub ${id}`, pending)] as const),
        ]);
        const menu = openMenu();

        expect(menu.querySelectorAll('.thread-overflow-sub-thread')).toHaveLength(LISTED_SUB_THREADS);
        const notes = [...menu.querySelectorAll('.thread-overflow-note')].map(n => n.textContent);
        expect(notes).toEqual([blockedBySubThreads(total), moreSubThreads(2)]);
    });

    it("shows the thread's own reason with no sub-thread rows", () => {
        threadMap.value = new Map([
            ['t', makeThreadState('t', { meta: { section: 'inbox', status: 'running' } })],
            ['c', sub('c', 'Child', pending)],
        ]);
        const menu = openMenu();

        expect(menu.querySelector('.thread-overflow-note')?.textContent).toBe(BLOCKER_REASON.running);
        expect(menu.querySelectorAll('.thread-overflow-sub-thread')).toHaveLength(0);
    });
});
