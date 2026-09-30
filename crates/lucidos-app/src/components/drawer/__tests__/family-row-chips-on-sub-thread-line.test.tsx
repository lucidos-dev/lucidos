// @vitest-environment jsdom
/**
 * A row with sub-threads carries its chips on the "Show N sub-threads" line,
 * leaving the date alone on its own line. A row without sub-threads keeps the
 * chips beside the date.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { ThreadRow } from '../ThreadDrawer';
import { threadMap } from '../../../store/store';
import type { ThreadState, ThreadMeta } from '../../../store/thread-events';

vi.mock('../../../store/actions/threads', () => ({
  focusThread: vi.fn(),
  handleSaveThread: vi.fn(),
  handleUnsaveThread: vi.fn(),
}));
vi.mock('../../../store/actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  loadOlderThreads: vi.fn(),
  reloadAfterFilterChange: vi.fn(),
  filterChangedSinceLoad: () => false,
  ensureThreadInMap: vi.fn(),
}));

const THREAD_ID = 'row-1';

function makeThread(totalChildrenCount: number): ThreadState {
  const meta: ThreadMeta = {
    id: THREAD_ID,
    title: 'A thread',
    channel: 'claude_code',
    codingAgent: 'claude-code',
    initiator: 'user',
    saved: false,
    createdAt: '2026-05-01T00:00:00Z',
    updatedAt: '2026-05-01T00:00:00Z',
    status: 'idle',
    summaryVersion: 0,
    messageCount: 1,
    section: 'inbox',
    activeChildrenCount: 0,
    totalChildrenCount,
    blockingDescendantCount: 0,
    attentionDescendantCount: 0,
    codingAgentHasDiff: false,
    codingAgentProposed: false,
    codingAgentRequiresRestart: false,
    codingAgentIsExternalRepo: false,
    lastRevivedAt: '',
    state: 'active',
    latestTodoList: null,
    liveEventWaitCount: 0,
    liveEventWaits: [],
  };
  return {
    meta,
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

let host: HTMLDivElement;

function mount(totalChildrenCount: number): void {
  threadMap.value = new Map([[THREAD_ID, makeThread(totalChildrenCount)]]);
  render(<ThreadRow threadId={THREAD_ID} status="idle" enableFamilyToggle />, host);
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('where a drawer row puts its chips', () => {
  it('puts them on the sub-thread line when the row has sub-threads', () => {
    mount(3);
    const familyLine = host.querySelector('.thread-row-family-line');
    expect(familyLine, 'no sub-thread line rendered').not.toBeNull();
    expect(familyLine!.querySelector(':scope > .family-disclosure')).not.toBeNull();
    // In their own box, so the link aligns with the bottom of all of them.
    expect(familyLine!.querySelector(':scope > .thread-row-family-chips > .message-channel-tag')).not.toBeNull();
  });

  it('leaves the date alone on the meta line when the row has sub-threads', () => {
    mount(3);
    const meta = host.querySelector('.thread-row-meta');
    expect(meta!.querySelector('.thread-row-created')).not.toBeNull();
    expect(meta!.querySelector('.message-channel-tag')).toBeNull();
  });

  it('keeps them beside the date when the row has no sub-threads', () => {
    mount(0);
    expect(host.querySelector('.thread-row-family-line')).toBeNull();
    expect(host.querySelector('.thread-row-meta .message-channel-tag')).not.toBeNull();
  });
});
