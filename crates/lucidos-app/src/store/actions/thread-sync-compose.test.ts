/** How SSE thread events and compose reports reach the draft. */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { focusedThreadId, threadMap } from '../store';
import type { ThreadMeta, ThreadState } from '../thread-events';
import { handleThreadEvent, handleGlobalEvent } from './thread-sync';
import { noteLocalComposeEdit, _resetComposeSyncForTesting } from '../composeSync';
import {
  _resetThreadNavForTesting,
  _threadNavStateForTesting,
  pushThreadNavState,
} from './thread-navigation';
import { _resetComposeDraftsForTesting, getDraft, setDraft, type ComposeDraft } from '../composeDrafts';

interface MakeThreadOpts extends Partial<ThreadMeta> {
  composeText?: string;
  composeImages?: string[];
  composeMode?: ComposeDraft['mode'];
}

function makeActiveThread(overrides: MakeThreadOpts = {}): ThreadState {
  const { composeText, composeImages, composeMode, ...metaOverrides } = overrides;
  const id = metaOverrides.id ?? 't-1';
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
      title: '',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '',
      updatedAt: '',
      status: 'idle',
      summaryVersion: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      messageCount: 0,
      section: 'archived',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0,
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      ...metaOverrides,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

const originalDocument = globalThis.document;

/** Plant a fake textarea matching `[data-role="prompt-input"]` and mark it as
 *  document.activeElement so isComposeFocusedHere() returns true. The `role`
 *  field on dataset must match the real DOM marker — isComposeFocusedHere
 *  reads document.activeElement directly and verifies dataset.role. */
function focusPromptOnThread(threadId: string): void {
  const el = {
    dataset: { role: 'prompt-input', threadId },
  };
  (globalThis as any).document = {
    ...originalDocument,
    activeElement: el,
  };
}

function unfocusPrompt(): void {
  (globalThis as any).document = {
    ...originalDocument,
    activeElement: null,
  };
}

describe('SSE thread events and the draft', () => {
  beforeEach(() => {
    focusedThreadId.value = 't-1';
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread({ composeText: 'follow up I was typing' }));
    threadMap.value = map;
  });

  afterEach(() => {
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetComposeSyncForTesting();
    (globalThis as any).document = originalDocument;
    vi.restoreAllMocks();
  });

  /** A send consumes the compose slot at accept, and the engine reports what
   *  that left (ADR 0450). The thread event itself never touches the draft,
   *  whoever sent it and wherever focus is. */
  it('a MessageReceived leaves the draft alone, focused or not, from any device', () => {
    focusPromptOnThread('t-1');
    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: { type: 'MessageReceived', text: 'peer-sent follow-up', device_id: 'peer-device' },
    });
    unfocusPrompt();
    handleThreadEvent({
      thread_id: 't-1',
      seq: 43,
      created: '2026-05-04T10:00:01Z',
      event: { type: 'MessageReceived', text: 'another', device_id: 'peer-device' },
    });

    expect(getDraft('t-1').text).toBe('follow up I was typing');
  });

  it('still mirrors state=active on MessageReceived', () => {
    threadMap.value.get('t-1')!.meta.state = 'composing';

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: { type: 'MessageReceived', text: 'previously sent message', device_id: 'peer-device' },
    });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('active');
  });

  it('keeps composeText when ThreadDiscarded arrives while user is typing', () => {
    focusPromptOnThread('t-1');
    threadMap.value.get('t-1')!.meta.state = 'composing';

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: { type: 'ThreadDiscarded' },
    });

    expect(getDraft('t-1').text).toBe('follow up I was typing');
  });

  it('keeps composeText when ThreadDiscarded originated from THIS device — discardCompose already mutated state locally', () => {
    localStorage.setItem('lucidos-device-id', 'me-device');
    unfocusPrompt();
    threadMap.value.get('t-1')!.meta.state = 'composing';

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: {
        type: 'ThreadDiscarded',
        actor: { kind: 'device', device_id: 'me-device' },
      },
    });

    expect(getDraft('t-1').text).toBe('follow up I was typing');
  });

  it('clears composeText on ThreadDiscarded from a peer device when textarea unfocused', () => {
    localStorage.setItem('lucidos-device-id', 'me-device');
    unfocusPrompt();
    threadMap.value.get('t-1')!.meta.state = 'composing';

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: {
        type: 'ThreadDiscarded',
        actor: { kind: 'device', device_id: 'peer-device' },
      },
    });

    expect(getDraft('t-1').text).toBe('');
  });
});

describe('SSE ThreadDiscarded from peer returns this device to compose view', () => {
  beforeEach(() => {
    _resetThreadNavForTesting();
    localStorage.setItem('lucidos-device-id', 'me-device');
    focusedThreadId.value = 't-1';
    pushThreadNavState({ type: 'thread', id: 't-1' });
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread({ state: 'composing', composeText: 'half-typed draft' }));
    threadMap.value = map;
  });

  afterEach(() => {
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetThreadNavForTesting();
    (globalThis as any).document = originalDocument;
    vi.restoreAllMocks();
  });

  it('releases focusedThreadId when the focused thread is discarded by a peer', () => {
    unfocusPrompt();

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: {
        type: 'ThreadDiscarded',
        actor: { kind: 'device', device_id: 'peer-device' },
      },
    });

    expect(focusedThreadId.value).toBeNull();
  });

  it('removes the discarded thread from the nav stack so Back/Forward cannot restore it', () => {
    unfocusPrompt();

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: {
        type: 'ThreadDiscarded',
        actor: { kind: 'device', device_id: 'peer-device' },
      },
    });

    const { stack } = _threadNavStateForTesting();
    expect(stack.find((e) => e.id === 't-1')).toBeUndefined();
  });

  it('keeps focus + nav when this device originated the discard (local discardCompose already handled it)', () => {
    unfocusPrompt();

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: {
        type: 'ThreadDiscarded',
        actor: { kind: 'device', device_id: 'me-device' },
      },
    });

    expect(focusedThreadId.value).toBe('t-1');
    expect(_threadNavStateForTesting().stack.find((e) => e.id === 't-1')).toBeDefined();
  });

  it('keeps focus when peer discards a thread that is NOT the focused one', () => {
    unfocusPrompt();
    focusedThreadId.value = 't-other';

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: {
        type: 'ThreadDiscarded',
        actor: { kind: 'device', device_id: 'peer-device' },
      },
    });

    expect(focusedThreadId.value).toBe('t-other');
  });

  it('keeps focus when the user is mid-keystroke on this thread (preserve work over auto-redirect)', () => {
    focusPromptOnThread('t-1');

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: {
        type: 'ThreadDiscarded',
        actor: { kind: 'device', device_id: 'peer-device' },
      },
    });

    expect(focusedThreadId.value).toBe('t-1');
    expect(getDraft('t-1').text).toBe('half-typed draft');
  });
});

describe('SSE ThreadComposeChanged is a compose report', () => {
  beforeEach(() => {
    localStorage.setItem('lucidos-device-id', 'me-device');
    focusedThreadId.value = 't-1';
    const map = new Map<string, ThreadState>();
    map.set('t-1', makeActiveThread({ composeText: 'synced-from-peer draft' }));
    threadMap.value = map;
  });

  afterEach(() => {
    focusedThreadId.value = null;
    threadMap.value = new Map();
    _resetComposeDraftsForTesting();
    _resetComposeSyncForTesting();
    (globalThis as any).document = originalDocument;
    vi.restoreAllMocks();
  });

  function report(revision: number, text: string): void {
    handleGlobalEvent('ThreadComposeChanged', {
      id: 't-1',
      summary_version: revision,
      compose_epoch: 0,
      text,
      image_hashes: [],
      origin_device_id: 'peer-device',
    });
  }

  it('an empty report clears a synced draft even when the textarea is focused here', () => {
    focusPromptOnThread('t-1');
    report(3, '');
    expect(getDraft('t-1').text).toBe('');
  });

  it('a pending local edit keeps its text over an empty report', () => {
    focusPromptOnThread('t-1');
    noteLocalComposeEdit('t-1');
    report(3, '');
    expect(getDraft('t-1').text).toBe('synced-from-peer draft');
  });

  /** Dropping it would lose the peer's write: the next keystroke here would
   *  PUT the older text plus the key over it. */
  it('a newer non-empty report applies while focused, when nothing is pending', () => {
    focusPromptOnThread('t-1');
    report(3, 'peer is typing something new');
    expect(getDraft('t-1').text).toBe('peer is typing something new');
  });

  it('a report at or below the held revision changes nothing', () => {
    report(5, 'newer');
    report(4, 'older, delivered late');
    expect(getDraft('t-1').text).toBe('newer');
  });
});
