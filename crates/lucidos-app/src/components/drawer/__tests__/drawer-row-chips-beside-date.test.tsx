// @vitest-environment jsdom
/**
 * Every drawer row carries its chips beside the date. A row with sub-threads
 * puts its "Show N sub-threads" link and archived toggle on a line below them.
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
    codingAgentChangeState: { kind: 'none' },
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
  it('keeps them beside the date when the row has sub-threads', () => {
    mount(3);
    expect(host.querySelector('.thread-row-meta .thread-row-created')).not.toBeNull();
    expect(host.querySelector('.thread-row-meta .message-channel-tag')).not.toBeNull();
  });

  it('puts only the sub-thread controls on the line below', () => {
    mount(3);
    const familyLine = host.querySelector('.thread-row-family-line');
    expect(familyLine, 'no sub-thread line rendered').not.toBeNull();
    expect(familyLine!.querySelector(':scope > .family-disclosure')).not.toBeNull();
    expect(familyLine!.querySelector('.label')).toBeNull();
    // Below the chips, not above them.
    expect(host.querySelector('.thread-row-meta + .thread-row-family-line')).not.toBeNull();
  });

  it('keeps them beside the date when the row has no sub-threads', () => {
    mount(0);
    expect(host.querySelector('.thread-row-family-line')).toBeNull();
    expect(host.querySelector('.thread-row-meta .message-channel-tag')).not.toBeNull();
  });
});
