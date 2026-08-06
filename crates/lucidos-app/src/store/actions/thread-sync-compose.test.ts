/** Guards in handleThreadEvent that preserve composeText when the user is
 *  typing during MessageReceived / ThreadDiscarded SSE arrival. */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { focusedThreadId, threadMap } from '../store';
import type { ThreadMeta, ThreadState } from '../thread-events';
import { handleThreadEvent, handleGlobalEvent } from './thread-sync';
import { composeEditedAt } from './compose';
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
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
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

describe('SSE MessageReceived preserves user-typed compose text', () => {
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
    composeEditedAt.clear();
    (globalThis as any).document = originalDocument;
    vi.restoreAllMocks();
  });

  it('keeps composeText when MessageReceived arrives while user is genuinely typing here (locally edited)', () => {
    focusPromptOnThread('t-1');
    // "User is typing" == a draft THIS device authored — stamp composeEditedAt,
    // as every local mutation (updateCompose) does. This is what
    // hasUnsentLocalDraft keys off, NOT DOM focus.
    composeEditedAt.set('t-1', Date.now());

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: { type: 'MessageReceived', text: 'previously sent message', device_id: 'peer-device' },
    });

    expect(getDraft('t-1').text).toBe('follow up I was typing');
  });

  it('clears a server-originated (synced) draft on a peer MessageReceived even when the textarea is focused here', () => {
    // Regression: a follow-up drafted on another device syncs here as a draft
    // (applyRemoteCompose / stageDraftFromApi → no composeEditedAt stamp). When
    // the peer SENDS it, this device must clear the stale synced draft. It was
    // being PRESERVED because the compose-clear was gated on isComposeFocusedHere
    // rather than authorship (hasUnsentLocalDraft) — so a focused textarea kept a
    // draft the user never typed. The draft from beforeEach is setDraft-created
    // (not locally edited), i.e. server-originated.
    focusPromptOnThread('t-1');

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: { type: 'MessageReceived', text: 'peer-sent follow-up', device_id: 'peer-device' },
    });

    expect(getDraft('t-1').text).toBe('');
  });

  it('still mirrors state=active on MessageReceived even when a locally-edited compose is preserved', () => {
    focusPromptOnThread('t-1');
    composeEditedAt.set('t-1', Date.now());
    threadMap.value.get('t-1')!.meta.state = 'composing';

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: { type: 'MessageReceived', text: 'previously sent message', device_id: 'peer-device' },
    });

    expect(threadMap.value.get('t-1')!.meta.state).toBe('active');
    // Locally-edited draft survives (authorship guard), independent of the state flip.
    expect(getDraft('t-1').text).toBe('follow up I was typing');
  });

  it('clears composeText on MessageReceived when textarea is NOT focused (peer send, unattended thread)', () => {
    unfocusPrompt();

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: { type: 'MessageReceived', text: 'peer-sent message', device_id: 'peer-device' },
    });

    expect(getDraft('t-1').text).toBe('');
  });

  it('keeps composeText when MessageReceived originated from THIS device, even when textarea unfocused', () => {
    localStorage.setItem('lucidos-device-id', 'me-device');
    unfocusPrompt();

    handleThreadEvent({
      thread_id: 't-1',
      seq: 42,
      created: '2026-05-04T10:00:00Z',
      event: { type: 'MessageReceived', text: 'previously sent message', device_id: 'me-device' },
    });

    expect(getDraft('t-1').text).toBe('follow up I was typing');
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
        actor: { kind: 'device', device_id: 'me-device', label: 'Me' },
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
        actor: { kind: 'device', device_id: 'peer-device', label: 'Peer' },
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
        actor: { kind: 'device', device_id: 'peer-device', label: 'Peer' },
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
        actor: { kind: 'device', device_id: 'peer-device', label: 'Peer' },
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
        actor: { kind: 'device', device_id: 'me-device', label: 'Me' },
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
        actor: { kind: 'device', device_id: 'peer-device', label: 'Peer' },
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
        actor: { kind: 'device', device_id: 'peer-device', label: 'Peer' },
      },
    });

    expect(focusedThreadId.value).toBe('t-1');
    expect(getDraft('t-1').text).toBe('half-typed draft');
  });
});

describe('SSE ThreadComposeChanged empty-clear from a peer (send/discard elsewhere)', () => {
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
    composeEditedAt.clear();
    (globalThis as any).document = originalDocument;
    vi.restoreAllMocks();
  });

  it('clears a server-originated draft even when the textarea is focused here', () => {
    // The peer's compose PUT cleared the shared draft (they sent/discarded it);
    // its empty ThreadComposeChanged must clear this device's synced mirror. The
    // focus guard used to drop the WHOLE inbound update — including an empty
    // clear — leaving the peer's follow-up preserved here. The draft from
    // beforeEach is setDraft-created (server-originated, no composeEditedAt).
    focusPromptOnThread('t-1');

    handleGlobalEvent('ThreadComposeChanged', {
      id: 't-1',
      text: '',
      image_hashes: [],
      mode: null,
      origin_device_id: 'peer-device',
    });

    expect(getDraft('t-1').text).toBe('');
  });

  it('keeps a locally-edited draft on an empty peer clear even when focused (authorship guard)', () => {
    focusPromptOnThread('t-1');
    composeEditedAt.set('t-1', Date.now());

    handleGlobalEvent('ThreadComposeChanged', {
      id: 't-1',
      text: '',
      image_hashes: [],
      mode: null,
      origin_device_id: 'peer-device',
    });

    expect(getDraft('t-1').text).toBe('synced-from-peer draft');
  });

  it('still drops a peer NON-empty update while the user is focused here (in-flight typing protected)', () => {
    focusPromptOnThread('t-1');

    handleGlobalEvent('ThreadComposeChanged', {
      id: 't-1',
      text: 'peer is typing something new',
      image_hashes: [],
      mode: 'lucidos',
      origin_device_id: 'peer-device',
    });

    // Focused → a non-empty peer update is not applied (would move the cursor).
    expect(getDraft('t-1').text).toBe('synced-from-peer draft');
  });
});
