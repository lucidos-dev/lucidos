/**
 * The row window keys each row's measured height on its layout props. These
 * tests pin that every prop that can change a row's height is in the key, and
 * that nothing which only paints is.
 */

import { describe, it, expect } from 'vitest';
import {
    THREAD_ROW_PROP_ROLES, threadRowLayoutKey, threadRowLayoutProps, setFamilyCollapsed,
    type ThreadRowLayoutProps,
} from './ThreadDrawer';
import type { ThreadState, ThreadMeta } from '../../store/thread-events';

function makeThread(overrides: Partial<ThreadMeta> = {}): ThreadState {
    const meta: ThreadMeta = {
        id: 't1',
        title: 'A thread',
        channel: 'chat',
        initiator: 'user',
        saved: false,
        createdAt: '2026-06-30T00:00:00Z',
        updatedAt: '2026-06-30T00:00:00Z',
        status: 'idle',
        summaryVersion: 0,
        messageCount: 1,
        section: 'inbox',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentChangeState: { kind: 'none' },
        codingAgentIsExternalRepo: false,
        lastRevivedAt: '',
        parentThreadId: undefined,
        state: 'active',
        latestTodoList: null,
        liveEventWaitCount: 0,
        liveEventWaits: [],
        ...overrides,
    };
    return {
        meta, events: new Map(), streamingBuffer: '', eventsLoaded: false,
        eventsLoadFailed: false, lastDbSeq: 0, pendingUserMessages: [],
    };
}

const BASE: ThreadRowLayoutProps = {
    depth: 0, title: 'A', createdLabel: 'Today 10:00:00', channel: 'chat', codingAgent: undefined,
    contextName: undefined, totalChildren: 0, hasDraft: false, isHome: false, collapsible: false,
    isCollapsed: false, hiddenArchivedCount: 0, isArchivedRevealed: false,
};

/** A different value of the same type, for each layout prop. */
const CHANGED: ThreadRowLayoutProps = {
    depth: 1, title: 'B', createdLabel: 'Oct 10 10:00:00', channel: 'claude_code', codingAgent: 'codex',
    contextName: 'example-repo', totalChildren: 3, hasDraft: true, isHome: true, collapsible: true,
    isCollapsed: true, hiddenArchivedCount: 2, isArchivedRevealed: true,
};

describe('thread row layout key', () => {
    it('changes when any layout prop changes', () => {
        const layoutNames = Object.keys(BASE) as (keyof ThreadRowLayoutProps)[];
        for (const name of layoutNames) {
            const changed = { ...BASE, [name]: CHANGED[name] };
            expect(threadRowLayoutKey(changed), name).not.toBe(threadRowLayoutKey(BASE));
        }
    });

    it('names exactly the props the roles table calls layout', () => {
        const layoutRoles = Object.entries(THREAD_ROW_PROP_ROLES)
            .filter(([, role]) => role === 'layout').map(([name]) => name).sort();
        const thread = makeThread();
        const props = threadRowLayoutProps(thread, { depth: 0, hiddenArchivedCount: 0, enableFamilyToggle: true });
        expect(Object.keys(props).sort()).toEqual(layoutRoles);
    });

    it('follows the family collapse state for a row with live sub-threads', () => {
        const thread = makeThread({ id: 'parent', totalChildrenCount: 2 });
        const input = { depth: 0, hiddenArchivedCount: 0, enableFamilyToggle: true };
        setFamilyCollapsed('parent', false);
        const open = threadRowLayoutKey(threadRowLayoutProps(thread, input));
        setFamilyCollapsed('parent', true);
        const shut = threadRowLayoutKey(threadRowLayoutProps(thread, input));
        setFamilyCollapsed('parent', false);
        expect(shut).not.toBe(open);
    });

    it('stays the same when only the thread status changes, which only paints', () => {
        const input = { depth: 0, hiddenArchivedCount: 0, enableFamilyToggle: true };
        const idle = threadRowLayoutKey(threadRowLayoutProps(makeThread({ status: 'idle' }), input));
        const running = threadRowLayoutKey(threadRowLayoutProps(makeThread({ status: 'running' }), input));
        expect(running).toBe(idle);
    });
});
