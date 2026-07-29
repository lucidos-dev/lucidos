/**
 * A **superseded draft** — one whose exact content has already been submitted to
 * the thread since this device last touched it — must be dropped, on every
 * inbound path.
 *
 * Regression: a draft typed on device A and then SUBMITTED from device B stayed
 * on A forever. `hasLocalDraftEdit` (now `hasUnsentLocalDraft`) vetoed every
 * inbound clear — the SSE `MessageReceived` echo, the empty
 * `ThreadComposeChanged`, and the `loadAllThreads` empty snapshot — and its
 * `composeEditedAt` stamp is never cleared and never expires, so the veto was
 * permanent. The reported case came in over the CUSTOM ANSWER path, where the
 * submitted text lands as `UserQuestionAnswered { FreeText }` and no
 * `MessageReceived` is ever emitted (chat/process/run.rs reroutes it), so there
 * was no live clear signal for peers at all.
 *
 * The discriminator is content match PLUS ordering PLUS server state — never
 * text similarity alone, because posting the same text many times must stay
 * possible. Ordering is compared in SERVER time only: the watermark stamped at
 * edit time (`composeEditWatermark`, from the thread's `meta.updatedAt`) against
 * the submitted event's `created`. Server state (`serverDraft`) covers what
 * ordering cannot see — a device that re-typed the text while still behind a
 * peer's submission has a stale watermark, but its own PUT re-filled the draft
 * server-side, and a server that still holds it proves the draft is new work.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
  if (typeof globalThis.document === 'undefined') (globalThis as any).document = {};
  if (!(globalThis.document as any).querySelector) (globalThis.document as any).querySelector = () => null;
  if (!(globalThis.document as any).querySelectorAll) (globalThis.document as any).querySelectorAll = () => [];
});

import { threadMap, focusedThreadId } from '../store';
import { handleThreadEvent, handleGlobalEvent } from '../actions/thread-sync';
import { composeEditedAt, composeEditWatermark, pendingComposePuts, serverDraft } from '../actions/compose';
import { refreshThreadEvents } from '../actions/thread-loading';
import { fetchThreadEvents } from '../../api/threads';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { getComposeSelectionOverride, seedComposeSelection, _resetComposeSelectionsForTesting } from '../composeSelections';
import type { StoredEvent, ThreadState } from '../thread-events';

vi.mock('../../api/threads', () => ({
  fetchThreads: vi.fn(),
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
  fetchThreadMessages: vi.fn(),
  fetchOlderThreads: vi.fn(),
  fetchFilterFacets: vi.fn(),
  fetchArchivedCount: vi.fn(),
  saveThread: vi.fn(),
  archiveThread: vi.fn(),
}));

vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

const T = 'thread-superseded';
/** The newest server activity this device has seen when the user types. */
const WATERMARK = '2026-07-28T05:00:00.000Z';
/** A submission that lands AFTER the local edit — the supersede evidence. */
const AFTER = '2026-07-28T06:32:00.000Z';
/** A submission that predates the local edit — NOT evidence. */
const BEFORE = '2026-07-28T04:00:00.000Z';
const DRAFT = 'night has passed by, any progress?';

function makeActiveThread(id: string, updatedAt: string): ThreadState {
  return {
    meta: {
      id,
      title: 'T',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-07-28T00:00:00.000Z',
      updatedAt,
      status: 'idle',
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentProposed: false,
      codingAgentRequiresRestart: false,
      codingAgentIsExternalRepo: false,
      codingAgentApplying: false,
      codingAgentHasDiff: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

/** Model a draft the user typed HERE and synced: the draft itself, the two
 *  stamps `markLocallyEdited` writes (local-edit marker + server-time
 *  watermark), and the acked PUT that leaves the server holding it. */
function typeDraftHere(id: string, text: string, watermark: string, imageHashes: string[] = []): void {
  setDraft(id, { text, image_hashes: imageHashes, mode: null });
  composeEditedAt.set(id, 1);
  composeEditWatermark.set(id, watermark);
  serverDraft.set(id, { text, imageHashes });
}

/** Put an already-known event into the thread's replayed history, without
 *  routing it through the live SSE handler (which does its own clearing). */
function seedEvent(id: string, seq: number, event: StoredEvent): void {
  threadMap.value.get(id)!.events.set(seq, event);
}

/** Model what a thread-summary snapshot tells this device: the server no longer
 *  holds a draft for the thread. */
function serverReportedNoDraft(id: string): void {
  serverDraft.set(id, { text: '', imageHashes: [] });
}

/** Model a peer's submission reaching this device live: the thread event, plus
 *  the empty `ThreadComposeChanged` that reports the server's new compose state
 *  (from the sending device's own compose clear, or — on the question-answer
 *  path, which has no such PUT — from the projection itself). BOTH frames
 *  matter: a thread event can be delivered long after it was written, so it is
 *  never taken as evidence of what the server holds now. */
function peerSubmitted(event: Record<string, unknown>, created: string, seq = 10): void {
  handleThreadEvent({ thread_id: T, seq, created, event });
  handleGlobalEvent('ThreadComposeChanged', {
    id: T, text: '', image_hashes: [], mode: null, origin_device_id: 'peer-device',
  });
}

describe('superseded drafts are dropped; unsent work is not', () => {
  beforeEach(() => {
    threadMap.value = new Map([[T, makeActiveThread(T, WATERMARK)]]);
    focusedThreadId.value = null;
    composeEditedAt.clear();
    composeEditWatermark.clear();
    serverDraft.clear();
    pendingComposePuts.clear();
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
    vi.mocked(fetchThreadEvents).mockResolvedValue({ events: [], currentAggregate: null });
  });

  afterEach(() => {
    threadMap.value = new Map();
    composeEditedAt.clear();
    composeEditWatermark.clear();
    serverDraft.clear();
    pendingComposePuts.clear();
    _resetComposeDraftsForTesting();
    _resetComposeSelectionsForTesting();
  });

  it('clears the draft when a peer sends the same text', () => {
    typeDraftHere(T, DRAFT, WATERMARK);

    peerSubmitted({ type: 'MessageReceived', text: DRAFT, device_id: 'peer-device' }, AFTER);

    expect(getDraft(T).text).toBe('');
  });

  it('clears the draft when a peer answers a question with the same text (CUSTOM ANSWER path)', () => {
    typeDraftHere(T, DRAFT, WATERMARK);

    peerSubmitted(
      { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'FreeText', text: DRAFT } },
      AFTER,
    );

    expect(getDraft(T).text).toBe('');
  });

  it('clears the draft when the multi-select answer folded in the same typed text', () => {
    typeDraftHere(T, DRAFT, WATERMARK);

    peerSubmitted(
      {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu-1',
        answer: { kind: 'MultiSelected', option_ids: ['a'], text: DRAFT },
      },
      AFTER,
    );

    expect(getDraft(T).text).toBe('');
  });

  it('clears the draft when the answer arrives BEFORE the peer compose clear', () => {
    // Frame order between the two is not guaranteed. Answer first: no server
    // report yet, so nothing may be concluded and the draft stands; the compose
    // clear that follows supplies the missing half.
    typeDraftHere(T, DRAFT, WATERMARK);

    handleThreadEvent({
      thread_id: T,
      seq: 10,
      created: AFTER,
      event: { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'FreeText', text: DRAFT } },
    });
    expect(getDraft(T).text).toBe(DRAFT);

    handleGlobalEvent('ThreadComposeChanged', {
      id: T, text: '', image_hashes: [], mode: null, origin_device_id: 'peer-device',
    });
    expect(getDraft(T).text).toBe('');
  });

  it('clears the draft on event replay — the wake path, where no live SSE arrives', async () => {
    typeDraftHere(T, DRAFT, WATERMARK);
    // The projection wiped compose_selection along with the text, so the draft's
    // dropdown picks must not survive locally either.
    seedComposeSelection(T, { model: 'claude-opus-5' });
    // loadAllThreads ran first and its snapshot came back empty — it just had no
    // evidence yet, because the missed messages arrive only with this replay.
    serverReportedNoDraft(T);
    vi.mocked(fetchThreadEvents).mockResolvedValue({
      events: [{
        sequence: 10,
        event_type: 'MessageReceived',
        payload: { text: DRAFT, device_id: 'peer-device' },
        created: AFTER,
        event_id: 'e-10',
      }],
      currentAggregate: null,
    });

    await refreshThreadEvents(T);

    expect(getDraft(T).text).toBe('');
    expect(getComposeSelectionOverride(T)).toEqual({});
  });

  it('accepts a peer empty compose sync once the submission is in history', () => {
    typeDraftHere(T, DRAFT, WATERMARK);
    seedEvent(T, 10, { type: 'MessageReceived', text: DRAFT, device_id: 'peer-device', created: AFTER });

    handleGlobalEvent('ThreadComposeChanged', {
      id: T,
      text: '',
      image_hashes: [],
      mode: null,
      origin_device_id: 'peer-device',
    });

    expect(getDraft(T).text).toBe('');
  });

  it('keeps the draft when the submitted text differs', () => {
    typeDraftHere(T, DRAFT, WATERMARK);

    peerSubmitted({ type: 'MessageReceived', text: 'something else entirely', device_id: 'peer-device' }, AFTER);

    expect(getDraft(T).text).toBe(DRAFT);
  });

  it('keeps the draft re-typed AFTER that same text was sent — posting twice must work', async () => {
    // The message is already in history and has advanced the thread's activity,
    // so the watermark captured when the user re-types sits at/after it. The
    // server-state half is deliberately NOT protecting this one (the server is
    // reported empty) — ordering alone must carry it.
    seedEvent(T, 10, { type: 'MessageReceived', text: DRAFT, device_id: 'peer-device', created: AFTER });
    threadMap.value.get(T)!.meta.updatedAt = AFTER;
    typeDraftHere(T, DRAFT, AFTER);
    serverReportedNoDraft(T);

    // A later refresh re-delivers the very same row (dedup drops it) and an
    // empty compose sync lands — neither is fresh evidence of a submission.
    await refreshThreadEvents(T);
    handleGlobalEvent('ThreadComposeChanged', {
      id: T, text: '', image_hashes: [], mode: null, origin_device_id: 'peer-device',
    });

    expect(getDraft(T).text).toBe(DRAFT);
  });

  it('keeps a draft re-typed while this device was still BEHIND the peer submission', async () => {
    // Ordering CANNOT see this case: the peer submitted at AFTER while this
    // device was asleep, so the watermark captured when the user then re-typed
    // is the stale pre-submission value and the replayed event looks newer.
    // What saves the draft is that our own PUT re-filled it server-side — a
    // server still holding our text proves the draft post-dates the submission.
    typeDraftHere(T, DRAFT, WATERMARK);
    vi.mocked(fetchThreadEvents).mockResolvedValue({
      events: [{
        sequence: 10,
        event_type: 'MessageReceived',
        payload: { text: DRAFT, device_id: 'peer-device' },
        created: AFTER,
        event_id: 'e-10',
      }],
      currentAggregate: null,
    });

    await refreshThreadEvents(T);

    expect(getDraft(T).text).toBe(DRAFT);
  });

  it('keeps the draft while a compose PUT of ours is still in flight', () => {
    // Mid-debounce the server has not seen this device's latest intent, so its
    // state is unknowable and nothing may be concluded from it.
    typeDraftHere(T, DRAFT, WATERMARK);
    serverReportedNoDraft(T);
    pendingComposePuts.add(T);
    seedEvent(T, 10, { type: 'MessageReceived', text: DRAFT, device_id: 'peer-device', created: AFTER });

    handleGlobalEvent('ThreadComposeChanged', {
      id: T, text: '', image_hashes: [], mode: null, origin_device_id: 'peer-device',
    });

    expect(getDraft(T).text).toBe(DRAFT);
  });

  it('keeps a locally-authored draft the server has never acknowledged', async () => {
    // Every compose PUT for this draft failed, so `serverDraft` is absent.
    // The text never reached the server, so the submission below cannot BE this
    // draft — it is a peer that typed the same thing — and the local copy is
    // work that exists nowhere else.
    setDraft(T, { text: DRAFT, image_hashes: [], mode: null });
    composeEditedAt.set(T, 1);
    composeEditWatermark.set(T, WATERMARK);
    vi.mocked(fetchThreadEvents).mockResolvedValue({
      events: [{
        sequence: 10,
        event_type: 'MessageReceived',
        payload: { text: DRAFT, device_id: 'peer-device' },
        created: AFTER,
        event_id: 'e-10',
      }],
      currentAggregate: null,
    });

    await refreshThreadEvents(T);

    expect(getDraft(T).text).toBe(DRAFT);
  });

  it('keeps the draft when the only matching submission predates the local edit', () => {
    typeDraftHere(T, DRAFT, WATERMARK);

    peerSubmitted({ type: 'MessageReceived', text: DRAFT, device_id: 'peer-device' }, BEFORE);

    expect(getDraft(T).text).toBe(DRAFT);
  });

  it('keeps a draft whose attached images the submission did not carry', () => {
    typeDraftHere(T, DRAFT, WATERMARK, ['hash-a']);

    peerSubmitted({ type: 'MessageReceived', text: DRAFT, device_id: 'peer-device' }, AFTER);

    expect(getDraft(T).text).toBe(DRAFT);
  });

  it('clears a draft whose attached images the submission carried verbatim', () => {
    typeDraftHere(T, DRAFT, WATERMARK, ['hash-a', 'hash-b']);

    peerSubmitted(
      { type: 'MessageReceived', text: DRAFT, user_image_hashes: ['hash-a', 'hash-b'], device_id: 'peer-device' },
      AFTER,
    );

    expect(getDraft(T).text).toBe('');
  });

  it('ignores whitespace-only differences — the answer path trims before submitting', () => {
    typeDraftHere(T, `${DRAFT}\n`, WATERMARK);

    peerSubmitted(
      { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'FreeText', text: DRAFT } },
      AFTER,
    );

    expect(getDraft(T).text).toBe('');
  });

  it('never supersedes on an agent-authored injection carrying the same text', () => {
    typeDraftHere(T, DRAFT, WATERMARK);

    peerSubmitted({ type: 'UserPromptInjected', text: DRAFT, mode: 'agent' }, AFTER);

    expect(getDraft(T).text).toBe(DRAFT);
  });
});
