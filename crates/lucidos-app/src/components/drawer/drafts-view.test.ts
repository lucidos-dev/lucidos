
import { describe, it, expect, beforeEach } from 'vitest';
import { composingThreads, draftThreads } from './ThreadDrawer';
import type { ThreadState, ThreadMeta } from '../../store/thread-events';
import { _resetComposeDraftsForTesting, setDraft, type ComposeDraft } from '../../store/composeDrafts';
import { threadMap } from '../../store/store';

interface MakeThreadOpts extends Partial<ThreadMeta> {
  composeText?: string;
  composeImages?: string[];
  composeMode?: ComposeDraft['mode'];
}

beforeEach(() => {
  _resetComposeDraftsForTesting();
  threadMap.value = new Map();
});

function makeThread(id: string, overrides: MakeThreadOpts = {}): ThreadState {
  const { composeText, composeImages, composeMode, ...metaOverrides } = overrides;
  if (composeText !== undefined || composeImages !== undefined || composeMode !== undefined) {
    setDraft(id, {
      text: composeText ?? '',
      image_hashes: composeImages ?? [],
      mode: composeMode ?? null,
    });
  }
  return {
    meta: {
      id,
      title: 'Test Thread',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-05-01T00:00:00Z',
      updatedAt: '2026-05-01T00:00:00Z',
      status: 'idle',
      summaryVersion: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      messageCount: 1,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0, archivableSubThreadCount: 0,
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      ...metaOverrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function asMap(threads: ThreadState[]): Map<string, ThreadState> {
  return new Map(threads.map(t => [t.meta.id, t]));
}

describe('draftThreads', () => {
  it('excludes composing threads with no text and no images', () => {
    const composing = makeThread('a', { state: 'composing' });
    const result = draftThreads(asMap([composing]));
    expect(result).toEqual([]);
  });

  it('includes composing threads with composeText', () => {
    const composing = makeThread('a', { state: 'composing', composeText: 'hi' });
    const result = draftThreads(asMap([composing]));
    expect(result.map(t => t.meta.id)).toEqual(['a']);
  });

  it('includes composing threads with only images', () => {
    const composing = makeThread('a', {
      state: 'composing',
      composeImages: ['data:image/png;base64,xxx'],
    });
    const result = draftThreads(asMap([composing]));
    expect(result.map(t => t.meta.id)).toEqual(['a']);
  });

  it('includes active threads with composeText', () => {
    const active = makeThread('a', { state: 'active', composeText: 'hello' });
    const result = draftThreads(asMap([active]));
    expect(result.map(t => t.meta.id)).toEqual(['a']);
  });

  it('includes active threads with pending compose images', () => {
    const active = makeThread('a', {
      state: 'active',
      composeImages: ['data:image/png;base64,xxx'],
    });
    const result = draftThreads(asMap([active]));
    expect(result.map(t => t.meta.id)).toEqual(['a']);
  });

  it('excludes active threads with no draft content', () => {
    const idle = makeThread('a', { state: 'active' });
    const result = draftThreads(asMap([idle]));
    expect(result).toEqual([]);
  });

  it('excludes discarded threads even with stale composeText', () => {
    const discarded = makeThread('a', { state: 'discarded', composeText: 'orphan' });
    const result = draftThreads(asMap([discarded]));
    expect(result).toEqual([]);
  });

  it('puts composing threads first, then drafts on existing threads, each group by updatedAt desc', () => {
    const oldNew = makeThread('old-new', { state: 'composing', composeText: 'a', updatedAt: '2026-05-01T00:00:00Z' });
    const freshDraft = makeThread('fresh-draft', { state: 'active', composeText: 'x', updatedAt: '2026-05-04T00:00:00Z' });
    const oldDraft = makeThread('old-draft', { state: 'active', composeText: 'y', updatedAt: '2026-05-02T00:00:00Z' });
    const freshNew = makeThread('fresh-new', { state: 'composing', composeText: 'b', updatedAt: '2026-05-03T00:00:00Z' });
    const result = draftThreads(asMap([oldNew, freshDraft, oldDraft, freshNew]));
    expect(result.map(t => t.meta.id)).toEqual(['fresh-new', 'old-new', 'fresh-draft', 'old-draft']);
  });

  it('returns empty list when no threads have drafts', () => {
    const a = makeThread('a', { state: 'active' });
    const b = makeThread('b', { state: 'active' });
    expect(draftThreads(asMap([a, b]))).toEqual([]);
  });
});

describe('composingThreads', () => {
  it('excludes composing threads with no text and no images', () => {
    const composing = makeThread('a', { state: 'composing' });
    expect(composingThreads(asMap([composing]))).toEqual([]);
  });

  it('includes composing threads with composeText', () => {
    const composing = makeThread('a', { state: 'composing', composeText: 'hi' });
    const result = composingThreads(asMap([composing]));
    expect(result.map(t => t.meta.id)).toEqual(['a']);
  });

  it('includes composing threads with only images', () => {
    const composing = makeThread('a', {
      state: 'composing',
      composeImages: ['data:image/png;base64,xxx'],
    });
    const result = composingThreads(asMap([composing]));
    expect(result.map(t => t.meta.id)).toEqual(['a']);
  });

  it('excludes non-composing threads', () => {
    const active = makeThread('a', { state: 'active', composeText: 'follow-up' });
    const discarded = makeThread('b', { state: 'discarded', composeText: 'orphan' });
    expect(composingThreads(asMap([active, discarded]))).toEqual([]);
  });

  it('sorts composing threads by updatedAt desc', () => {
    const old = makeThread('old', { state: 'composing', composeText: 'a', updatedAt: '2026-05-01T00:00:00Z' });
    const fresh = makeThread('fresh', { state: 'composing', composeText: 'b', updatedAt: '2026-05-04T00:00:00Z' });
    const result = composingThreads(asMap([old, fresh]));
    expect(result.map(t => t.meta.id)).toEqual(['fresh', 'old']);
  });

  it('excludes composing threads whose only text is whitespace', () => {
    const composing = makeThread('a', { state: 'composing', composeText: '   \n\t' });
    expect(composingThreads(asMap([composing]))).toEqual([]);
  });
});

describe('the Drafts group count', () => {
  it('counts only the draft rows the group renders', () => {
    threadMap.value = asMap([
      makeThread('new-draft', { state: 'composing', composeText: 'hi' }),
      makeThread('follow-up-draft', { state: 'active', composeText: 'follow-up' }),
      makeThread('image-draft', { state: 'active', composeImages: ['data:image/png;base64,xxx'] }),
      makeThread('ghost-composing', { state: 'composing' }),
      makeThread('discarded-stale', { state: 'discarded', composeText: 'orphan' }),
      makeThread('idle', { state: 'active' }),
    ]);

    expect(draftThreads(threadMap.value)).toHaveLength(3);
  });

  it('returns zero when no thread has a visible draft', () => {
    threadMap.value = asMap([
      makeThread('ghost-composing', { state: 'composing' }),
      makeThread('discarded-stale', { state: 'discarded', composeText: 'orphan' }),
      makeThread('idle', { state: 'active' }),
    ]);

    expect(draftThreads(threadMap.value)).toHaveLength(0);
  });
});
