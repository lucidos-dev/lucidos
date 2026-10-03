// @vitest-environment jsdom
/**
 * A row with hidden archived children shows a "N archived" reveal toggle on
 * the sub-thread line, on its own when there are no live children. A row
 * with no hidden archived children shows no such toggle, even when
 * collapsible. Covers the UI half of
 * slack-20261002-hide-archived-child-threads; the hiding logic itself is
 * `hidden-archived-children.test.ts`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { ThreadRow, setArchivedRevealed } from '../ThreadDrawer';
import { focusThread } from '../../../store/actions/threads';
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
    channel: 'chat',
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
    codingAgentIncomplete: false,
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

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('the archived-reveal toggle', () => {
  it('renders nothing when there are no hidden archived children', () => {
    threadMap.value = new Map([[THREAD_ID, makeThread(2)]]);
    render(<ThreadRow threadId={THREAD_ID} status="idle" enableFamilyToggle hiddenArchivedCount={0} />, host);

    expect(host.querySelector('.archived-reveal-toggle')).toBeNull();
  });

  it('shows the toggle, unchecked, labelled with the count, when hidden archived children exist', () => {
    threadMap.value = new Map([[THREAD_ID, makeThread(3)]]);
    render(
      <ThreadRow threadId={THREAD_ID} status="idle" enableFamilyToggle hiddenArchivedCount={2} isArchivedRevealed={false} />,
      host,
    );

    const toggle = host.querySelector('.archived-reveal-toggle input') as HTMLInputElement | null;
    expect(toggle).not.toBeNull();
    expect(toggle!.checked).toBe(false);
    expect(toggle!.getAttribute('aria-label')).toBe('Show 2 archived sub-threads');
    expect(host.querySelector('.archived-reveal-label')?.textContent).toBe('2 archived');
  });

  it('shows the toggle as checked and relabels it when revealed', () => {
    threadMap.value = new Map([[THREAD_ID, makeThread(1)]]);
    render(
      <ThreadRow threadId={THREAD_ID} status="idle" enableFamilyToggle hiddenArchivedCount={1} isArchivedRevealed={true} />,
      host,
    );

    const toggle = host.querySelector('.archived-reveal-toggle input') as HTMLInputElement | null;
    expect(toggle!.checked).toBe(true);
    expect(toggle!.getAttribute('aria-label')).toBe('Hide 1 archived sub-thread');
  });

  it('flips once, and opens no thread, on a click on its "N archived" text', () => {
    threadMap.value = new Map([[THREAD_ID, makeThread(3)]]);
    render(
      <ThreadRow threadId={THREAD_ID} status="idle" enableFamilyToggle hiddenArchivedCount={2} isArchivedRevealed={false} />,
      host,
    );
    const input = host.querySelector('.archived-reveal-toggle input') as HTMLInputElement;
    let changes = 0;
    input.addEventListener('change', () => { changes += 1; });

    (host.querySelector('.archived-reveal-label') as HTMLElement).click();

    expect(changes).toBe(1);
    expect(focusThread).not.toHaveBeenCalled();
    setArchivedRevealed(THREAD_ID, false);
  });

  it('renders both controls on one line, with no separator, when live and archived children coexist', () => {
    // totalChildrenCount counts both; the live chevron's own count subtracts
    // the hidden ones (see visibleChildrenCount / hidden-archived-children.test.ts).
    threadMap.value = new Map([[THREAD_ID, makeThread(3)]]);
    render(
      <ThreadRow threadId={THREAD_ID} status="idle" enableFamilyToggle hiddenArchivedCount={1} isArchivedRevealed={false} />,
      host,
    );

    const controls = host.querySelector('.thread-row-family-line');
    expect(controls?.querySelector('.family-disclosure')?.textContent).toContain('2 sub-threads');
    expect(controls?.textContent).not.toContain('·');
    // The switch and its count wrap together as one unit.
    expect(controls?.querySelector('.archived-reveal > .archived-reveal-toggle + .archived-reveal-label')).not.toBeNull();
  });

  it('shows only the toggle, with no sub-thread chevron, when every child is hidden archived', () => {
    threadMap.value = new Map([[THREAD_ID, makeThread(2)]]);
    render(
      <ThreadRow threadId={THREAD_ID} status="idle" enableFamilyToggle hiddenArchivedCount={2} isArchivedRevealed={false} />,
      host,
    );

    const controls = host.querySelector('.thread-row-family-line');
    expect(controls?.querySelector('.family-disclosure')).toBeNull();
    expect(controls?.querySelector('.archived-reveal-toggle')).not.toBeNull();
  });

  it('is absent from a non-collapsible context even with hidden archived children', () => {
    // Mirrors the existing sub-thread toggle's search/drafts carve-out: only
    // the nested ThreadList passes `enableFamilyToggle`.
    threadMap.value = new Map([[THREAD_ID, makeThread(2)]]);
    render(<ThreadRow threadId={THREAD_ID} status="idle" hiddenArchivedCount={2} isArchivedRevealed={false} />, host);

    expect(host.querySelector('.archived-reveal-toggle')).toBeNull();
  });
});
