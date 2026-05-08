import { describe, it, expect, beforeEach } from 'vitest';
import { composingTitle, threadDisplayTitle } from './threadTitle';
import { makeOptimisticThreadState, PENDING_TITLE_PLACEHOLDER, type ThreadState } from '../store/thread-events';
import { _resetComposeDraftsForTesting, setDraft } from '../store/composeDrafts';

beforeEach(() => {
    _resetComposeDraftsForTesting();
});

function makeThread(opts: Partial<{
    title: string;
    state: 'composing' | 'active';
    composeText: string;
    pendingText: string;
    firstMessageText: string;
}> = {}): ThreadState {
    const t = makeOptimisticThreadState({
        id: 'tid',
        title: opts.title ?? '',
        channel: 'chat',
        initiator: 'user',
        eventsLoaded: true,
        state: opts.state ?? 'active',
    });
    if (opts.composeText !== undefined) {
        setDraft('tid', { text: opts.composeText, image_hashes: [], mode: null });
    }
    if (opts.pendingText !== undefined) {
        t.pendingUserMessages.push({ text: opts.pendingText, eventId: 'p-1', created: new Date().toISOString() });
    }
    if (opts.firstMessageText !== undefined) {
        t.events.set(1, { type: 'MessageReceived', text: opts.firstMessageText });
    }
    return t;
}

describe('composingTitle', () => {
    it('returns "Empty draft" for empty / whitespace-only compose text', () => {
        expect(composingTitle('')).toBe('Empty draft');
        expect(composingTitle('   ')).toBe('Empty draft');
        expect(composingTitle('\n\t')).toBe('Empty draft');
    });

    it('returns trimmed text when short', () => {
        expect(composingTitle('Selecting a thread')).toBe('Selecting a thread');
        expect(composingTitle('  hi  ')).toBe('hi');
    });

    it('caps at 40 characters', () => {
        const long = 'a'.repeat(60);
        const out = composingTitle(long);
        expect(out.length).toBe(40);
    });

    it('appends an ellipsis when the compose text is truncated', () => {
        const long = 'a'.repeat(60);
        expect(composingTitle(long).endsWith('…')).toBe(true);
    });

    it('does not append an ellipsis when the compose text fits', () => {
        expect(composingTitle('Selecting a thread').endsWith('…')).toBe(false);
    });
});

describe('threadDisplayTitle', () => {
    it('uses composeText preview while composing', () => {
        const t = makeThread({ state: 'composing', composeText: 'Selecting a thread' });
        expect(threadDisplayTitle(t)).toBe('Selecting a thread');
    });

    it('returns generated title when present', () => {
        const t = makeThread({ state: 'active', title: 'My LLM-named thread' });
        expect(threadDisplayTitle(t)).toBe('My LLM-named thread');
    });

    it('falls back to first pending message preview when title is empty (post-send, pre-SSE)', () => {
        const t = makeThread({ state: 'active', title: '', pendingText: 'Selecting a thread' });
        expect(threadDisplayTitle(t)).toBe('Selecting a thread');
    });

    it('falls back to first MessageReceived event preview when title is empty (post-SSE, pre-title-generation)', () => {
        const t = makeThread({ state: 'active', title: '', firstMessageText: 'Selecting a thread' });
        expect(threadDisplayTitle(t)).toBe('Selecting a thread');
    });

    it('treats PENDING_TITLE_PLACEHOLDER as no title and falls back to message preview', () => {
        const t = makeThread({ state: 'active', title: PENDING_TITLE_PLACEHOLDER, firstMessageText: 'Hello world' });
        expect(threadDisplayTitle(t)).toBe('Hello world');
    });

    it('caps message-derived titles at 40 chars', () => {
        const long = 'b'.repeat(60);
        const t = makeThread({ state: 'active', title: '', firstMessageText: long });
        expect(threadDisplayTitle(t).length).toBe(40);
    });

    it('appends an ellipsis to message-derived titles when truncated', () => {
        const long = 'b'.repeat(60);
        const t = makeThread({ state: 'active', title: '', firstMessageText: long });
        expect(threadDisplayTitle(t).endsWith('…')).toBe(true);
    });

    it('returns "Untitled Thread" only when no title and no message text exists', () => {
        const t = makeThread({ state: 'active', title: '' });
        expect(threadDisplayTitle(t)).toBe('Untitled Thread');
    });

    it('treats whitespace-only meta.title as empty (defends against bad backend data)', () => {
        const t = makeThread({ state: 'active', title: '   ', firstMessageText: 'Real first message' });
        expect(threadDisplayTitle(t)).toBe('Real first message');
    });

    it('drawer/header parity: same input always produces same output (regression: header showed "Untitled Thread" briefly between send and ThreadTitleGenerated)', () => {
        const t = makeThread({ state: 'active', title: '', pendingText: 'Selecting a thread' });
        const drawerTitle = threadDisplayTitle(t);
        const headerTitle = threadDisplayTitle(t);
        expect(drawerTitle).toBe(headerTitle);
        expect(drawerTitle).toBe('Selecting a thread');
        expect(drawerTitle).not.toBe('Untitled Thread');
    });
});
