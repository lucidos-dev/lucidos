// @vitest-environment jsdom
/**
 * Every drawer row carries its chips beside the date, and the agent mark last.
 * A row with sub-threads puts its "Show N sub-threads" link and archived toggle
 * on a line below them.
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

function makeThread(totalChildrenCount: number, overrides: Partial<ThreadMeta> = {}): ThreadState {
  const meta: ThreadMeta = {
    id: THREAD_ID,
    title: 'A thread',
    channel: 'claude_code',
    codingAgent: 'claude-code',
    repoName: 'example-repo',
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
    ...overrides,
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

function mount(totalChildrenCount: number, overrides: Partial<ThreadMeta> = {}): void {
  threadMap.value = new Map([[THREAD_ID, makeThread(totalChildrenCount, overrides)]]);
  render(<ThreadRow threadId={THREAD_ID} status="idle" enableFamilyToggle />, host);
}

beforeEach(() => {
  // jsdom has no layout. The name chip measures its lines on mount, and an
  // empty list reads as one line, which needs no width pin.
  Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
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
    expect(host.querySelector('.thread-row-meta .thread-row-agent')).not.toBeNull();
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
    expect(host.querySelector('.thread-row-meta .thread-row-agent')).not.toBeNull();
  });
});

describe('the agent mark', () => {
  it('sits last on the meta line, after the name chip', () => {
    mount(0);
    const meta = host.querySelector('.thread-row-meta')!;
    expect(meta.lastElementChild!.classList.contains('thread-row-agent')).toBe(true);
    expect(meta.querySelector('.thread-row-context + .thread-row-agent')).not.toBeNull();
  });

  it.each([
    [{ channel: 'claude_code', codingAgent: 'claude-code' }, '.claude-icon', 'Claude Code'],
    [{ channel: 'claude_code', codingAgent: 'codex' }, '.codex-icon', 'Codex'],
    [{ channel: 'chat', codingAgent: undefined }, '.lucidos-mark-icon', 'Lucidos Agent'],
    [{ channel: 'trigger', codingAgent: undefined }, 'svg', 'Trigger'],
  ] as const)('draws the mark for %o', (overrides, iconSelector, label) => {
    mount(0, overrides);
    const mark = host.querySelector('.thread-row-agent')!;
    expect(mark.querySelector(iconSelector)).not.toBeNull();
    expect(mark.getAttribute('aria-label')).toBe(label);
    expect(host.querySelector('.message-channel-tag')).toBeNull();
  });

  it('keeps the red ERROR tag for an unknown channel', () => {
    mount(0, { channel: 'error_unknown_channel' });
    expect(host.querySelector('.thread-row-agent')).toBeNull();
    expect(host.querySelector('.message-channel-tag.channel-error')?.textContent).toBe('ERROR');
  });

  it('names a channel it has no mark for, rather than guessing one', () => {
    mount(0, { channel: 'future_channel' as ThreadMeta['channel'] });
    expect(host.querySelector('.thread-row-agent')).toBeNull();
    expect(host.querySelector('.message-channel-tag')?.textContent).toBe('future_channel');
  });
});

describe('the agent marks of a thread that ran on several backends', () => {
  const marks = () => [...host.querySelectorAll('.thread-row-meta .thread-row-agent')];
  const glyphs = () => marks().map(m => m.getAttribute('data-glyph'));

  it('draws one mark for a single backend, with no stack around it', () => {
    mount(0, { codingAgents: ['codex'], codingAgent: 'codex' });
    expect(glyphs()).toEqual(['codex']);
    expect(host.querySelector('.thread-row-agents')).toBeNull();
  });

  it('draws the marks first-used on top, under a label naming both', () => {
    mount(0, { codingAgents: ['claude-code', 'codex'], codingAgent: 'codex' });
    expect(glyphs()).toEqual(['claude', 'codex']);
    expect(host.querySelector('.thread-row-meta > .thread-row-agents')!.getAttribute('aria-label'))
      .toBe('Claude Code, then Codex');
  });

  it('keeps the first-use order when Codex came first', () => {
    mount(0, { codingAgents: ['codex', 'claude-code'], codingAgent: 'claude-code' });
    expect(glyphs()).toEqual(['codex', 'claude']);
    expect(host.querySelector('.thread-row-agents')!.getAttribute('aria-label')).toBe('Codex, then Claude Code');
  });
});
