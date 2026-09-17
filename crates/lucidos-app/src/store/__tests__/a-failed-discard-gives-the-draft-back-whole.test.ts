/**
 * A discard that the engine refuses gives the draft back WHOLE.
 *
 * The rollback used to restore text, images and channel, and nothing else.
 * `forgetComposeState` had already dropped the draft's `composeSelections`
 * entry, so `resolveScope` answered Lucidos for a draft aimed at a repo. The
 * destination row and the drawer chip then said Lucidos source. A Send from
 * there bound `codingAgentKind: 'lucidos'` and ran the coding agent against
 * the wrong tree.
 *
 * Nothing else puts the override back: `stageDraftFromApi` returns early on an
 * unsent local draft, and the DELETE failed so no `ThreadComposeChanged` fires.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.hoisted(() => {
  if (typeof globalThis.document === 'undefined') (globalThis as any).document = {};
  const doc = globalThis.document as any;
  if (!doc.querySelector) doc.querySelector = () => null;
  if (!doc.querySelectorAll) doc.querySelectorAll = () => [];
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
});

vi.mock('../../api/threads', () => ({
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
}));

import { discardCompose } from '../actions/compose';
import { focusedThreadId, threadMap, toasts } from '../store';
import { _resetComposeDraftsForTesting, getDraft, setDraft } from '../composeDrafts';
import {
  _resetComposeSelectionsForTesting,
  resolveCodingAgent,
  resolveScope,
  seedComposeSelection,
} from '../composeSelections';
import type { ThreadState } from '../thread-events';

const T = 'draft-aimed-at-a-repo';
const REPO = { kind: 'external' as const, repoId: 'repo-7' };
const originalFetch = globalThis.fetch;

function composingThread(id: string): ThreadState {
  return {
    meta: {
      id,
      title: '',
      channel: 'claude_code',
      initiator: 'user',
      saved: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
      status: 'idle',
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      state: 'composing',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

describe('a discard the engine refuses', () => {
  beforeEach(() => {
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    toasts.value = [];
    focusedThreadId.value = null;
    threadMap.value = new Map([[T, composingThread(T)]]);
    setDraft(T, { text: 'rename the port', image_hashes: ['hash-1'], mode: 'claude_code' });
    seedComposeSelection(T, { scope: REPO, codingAgent: 'codex' });
    globalThis.fetch = vi.fn().mockResolvedValue(new Response(null, { status: 500 })) as unknown as typeof fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    threadMap.value = new Map();
    toasts.value = [];
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    vi.restoreAllMocks();
  });

  it('gives the text back', async () => {
    await discardCompose(T);

    expect(threadMap.value.get(T)!.meta.state).toBe('composing');
    expect(getDraft(T).text).toBe('rename the port');
    expect(getDraft(T).image_hashes).toEqual(['hash-1']);
  });

  it('gives the destination back with it', async () => {
    await discardCompose(T);

    expect(resolveScope(T)).toEqual(REPO);
    expect(resolveCodingAgent(T)).toBe('codex');
  });

  it('says the discard failed', async () => {
    await discardCompose(T);

    expect(toasts.value.map(t => t.message).join(' ')).toContain('Discard failed');
  });
});
