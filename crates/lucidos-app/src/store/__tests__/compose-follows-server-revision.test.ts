/**
 * The compose draft follows the server's *compose revision* (ADR 0437,
 * `docs/plans/2026-10-12-compose-draft-follows-a-server-revision.md`).
 *
 * Every case replays one interleaving with explicit synchronization points:
 * a PUT is held until the test resolves it, and each report lands exactly
 * where the test delivers it. No sleeps.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readdirSync, readFileSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join, resolve } from 'node:path';

vi.hoisted(() => {
  if (typeof globalThis.document === 'undefined') (globalThis as any).document = {};
  const doc = globalThis.document as any;
  if (!doc.querySelector) doc.querySelector = () => null;
  if (!doc.querySelectorAll) doc.querySelectorAll = () => [];
});

vi.mock('../../api/threads', () => ({
  fetchThreadEvents: vi.fn().mockResolvedValue({ events: [], currentAggregate: null }),
}));

import { connectionStatus, focusedThreadId, threadMap } from '../store';
import { handleGlobalEvent } from '../actions/thread-sync';
import { serverComposeOf } from '../composeSync';
import {
  discardCompose,
  ensureFocusedComposeThread,
  updateCompose,
  _flushAllPendingForTesting,
  _resetUndeliveredComposeDraftsForTesting,
  _undeliveredComposeDraftsForTesting,
} from '../actions/compose';
import { upsertThread } from '../actions/thread-loading';
import { getDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import { _resetComposeSelectionsForTesting, getComposeSelectionOverride, patchComposeSelection } from '../composeSelections';
import { _resetRowIntentsForTesting } from '../rowIntents';
import type { ThreadState } from '../thread-events';
import type { ThreadSummary } from '../../api/threads';

const T = 't-rev';
const originalFetch = globalThis.fetch;

/** A PUT the test holds open until it answers. */
interface HeldPut {
  body: { text: string; compose_epoch?: number };
  answer: (res: Response) => void;
}

let puts: HeldPut[] = [];
/** What the next DELETE answers. */
let deleteAnswer: () => Response = () => new Response(JSON.stringify({ summary_version: 6 }), { status: 200 });

function report(revision: number, text: string, extra: Record<string, unknown> = {}) {
  return {
    summary_version: revision,
    compose_epoch: 0,
    text,
    image_hashes: [],
    ...extra,
  };
}

function ackPut(index: number, revision: number): void {
  const put = puts[index];
  put.answer(new Response(JSON.stringify(report(revision, put.body.text)), { status: 200 }));
}

function peerFrame(revision: number, text: string, extra: Record<string, unknown> = {}): void {
  handleGlobalEvent('ThreadComposeChanged', {
    id: T,
    ...report(revision, text, extra),
    origin_device_id: 'peer-device',
  });
}

function summary(revision: number, text: string): ThreadSummary {
  return {
    thread_id: T,
    title: 'T',
    channel: 'chat',
    initiator: 'user',
    created_at: '2026-10-12T08:00:00.000Z',
    last_activity: '2026-10-12T08:00:00.000Z',
    message_count: 1,
    section: 'inbox',
    active_children_count: 0,
    total_children_count: 0,
    blocking_descendant_count: 0,
    attention_descendant_count: 0,
    live_event_wait_count: 0,
    status: 'idle',
    summary_version: revision,
    coding_agent_change_state: { kind: 'none' },
    coding_agent_is_external_repo: false,
    last_revived_at: null,
    state: 'active',
    compose_text: text,
    compose_images: [],
    compose_epoch: 0,
  } as ThreadSummary;
}

function activeThread(): ThreadState {
  return {
    meta: {
      id: T,
      title: 'T',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-10-12T08:00:00.000Z',
      updatedAt: '2026-10-12T08:00:00.000Z',
      status: 'idle',
      summaryVersion: 0,
      messageCount: 1,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
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

/** Type `text` and let the debounce fire, so its PUT is in flight. */
async function typeAndFlush(text: string): Promise<void> {
  updateCompose(T, { text });
  await vi.advanceTimersByTimeAsync(300);
}

beforeEach(() => {
  vi.useFakeTimers();
  puts = [];
  deleteAnswer = () => new Response(JSON.stringify({ summary_version: 6 }), { status: 200 });
  globalThis.fetch = vi.fn((_url: unknown, init?: RequestInit) => {
    if (init?.method === 'PUT') {
      return new Promise<Response>((answer) => {
        puts.push({ body: JSON.parse(init.body as string), answer });
      });
    }
    if (init?.method === 'DELETE') return Promise.resolve(deleteAnswer());
    return Promise.resolve(new Response('{}', { status: 200 }));
  }) as unknown as typeof fetch;
  connectionStatus.value = 'connected';
  focusedThreadId.value = T;
  threadMap.value = new Map([[T, activeThread()]]);
});

afterEach(() => {
  globalThis.fetch = originalFetch;
  vi.useRealTimers();
  connectionStatus.value = 'disconnected';
  focusedThreadId.value = null;
  threadMap.value = new Map();
  _resetComposeDraftsForTesting();
  _resetComposeSelectionsForTesting();
  _resetUndeliveredComposeDraftsForTesting();
  _resetRowIntentsForTesting();
});

describe('a report at or below the known revision changes nothing (C1, C6)', () => {
  it('a peer frame delayed past our newer ack leaves the draft and the server record alone', async () => {
    await typeAndFlush('ours');
    ackPut(0, 7);
    await vi.advanceTimersByTimeAsync(0);

    peerFrame(6, 'an older peer draft', { image_hashes: ['h'.repeat(64)] });

    expect(getDraft(T).text).toBe('ours');
    expect(serverComposeOf(T)?.text).toBe('ours');
    expect(serverComposeOf(T)?.imageHashes).toEqual([]);
  });

  it('a summary read before our write commits does not apply, whenever its GET went out', async () => {
    await typeAndFlush('ours');
    ackPut(0, 7);
    await vi.advanceTimersByTimeAsync(0);

    upsertThread(threadMap.value, summary(5, 'the pre-write draft'), false);

    expect(getDraft(T).text).toBe('ours');
  });

  it('our own echo landing after the ack is a no-op', async () => {
    await typeAndFlush('ours');
    ackPut(0, 7);
    await vi.advanceTimersByTimeAsync(0);
    updateCompose(T, { text: 'ours, edited' });
    await vi.advanceTimersByTimeAsync(300);
    ackPut(1, 8);
    await vi.advanceTimersByTimeAsync(0);

    handleGlobalEvent('ThreadComposeChanged', {
      id: T, ...report(7, 'ours'), origin_device_id: null,
    });

    expect(getDraft(T).text).toBe('ours, edited');
  });
});

describe('a pending edit holds until an ack covers it (C2)', () => {
  it('a newer report cannot replace text typed during the PUT', async () => {
    await typeAndFlush('first');
    updateCompose(T, { text: 'first and more' });
    peerFrame(9, 'a peer draft');
    ackPut(0, 8);
    await vi.advanceTimersByTimeAsync(0);

    expect(getDraft(T).text).toBe('first and more');
  });
});

describe('when the intent drops, the newest server state shows (C3)', () => {
  it('a peer write that landed after ours replaces our draft once our ack arrives', async () => {
    await typeAndFlush('ours');
    peerFrame(8, 'the peer wrote after us');
    ackPut(0, 7);
    await vi.advanceTimersByTimeAsync(0);

    expect(getDraft(T).text).toBe('the peer wrote after us');
  });

  it('with nothing pending, a newer peer draft applies, and the next PUT builds on it', async () => {
    upsertThread(threadMap.value, summary(3, 'synced'), false);
    peerFrame(4, 'synced, peer edit');
    expect(getDraft(T).text).toBe('synced, peer edit');

    await typeAndFlush('synced, peer edit!');
    expect(puts[0].body.text).toBe('synced, peer edit!');
  });
});

describe('delivery after an unload, and an older engine (C12, C13)', () => {
  it('the keepalive flush leaves its threads owed a re-send', async () => {
    updateCompose(T, { text: 'typed as the page hid' });
    _flushAllPendingForTesting();
    expect(_undeliveredComposeDraftsForTesting()).toEqual([T]);
  });

  it('a frame without summary_version applies when nothing is pending', () => {
    peerFrame(5, 'versioned');
    handleGlobalEvent('ThreadComposeChanged', { id: T, compose_epoch: 0, text: 'from an older engine', image_hashes: [] });
    expect(getDraft(T).text).toBe('from an older engine');
  });

  it('a 204 without a report still acknowledges the write', async () => {
    await typeAndFlush('ours');
    puts[0].answer(new Response(null, { status: 204 }));
    await vi.advanceTimersByTimeAsync(0);

    peerFrame(9, 'a peer draft');
    expect(getDraft(T).text).toBe('a peer draft');
  });
});

describe('a discard holds until the row reaches its version (C9)', () => {
  it('a read from before the discard cannot bring the draft back', async () => {
    const composing = activeThread();
    composing.meta = { ...composing.meta, state: 'composing', summaryVersion: 4 };
    threadMap.value = new Map([[T, composing]]);
    updateCompose(T, { text: 'the draft' });

    await discardCompose(T);
    upsertThread(threadMap.value, { ...summary(4, 'the draft'), state: 'composing' }, false);

    expect(threadMap.value.get(T)!.meta.state).toBe('discarded');
    expect(getDraft(T).text).toBe('');
  });
});

describe('a failed discard keeps the edits it interrupted', () => {
  it('a PUT answered after the rollback cannot replace text typed since', async () => {
    const composing = activeThread();
    composing.meta = { ...composing.meta, state: 'composing' };
    threadMap.value = new Map([[T, composing]]);
    await typeAndFlush('abc');
    updateCompose(T, { text: 'abcd' });
    deleteAnswer = () => new Response(JSON.stringify({ error: 'boom' }), { status: 500 });

    await discardCompose(T);
    expect(getDraft(T).text).toBe('abcd');
    ackPut(0, 7);
    await vi.advanceTimersByTimeAsync(0);

    expect(getDraft(T).text).toBe('abcd');
  });
});

describe('a new draft keeps the picks it was seeded with', () => {
  it('the thread start report cannot wipe them', async () => {
    focusedThreadId.value = null;
    patchComposeSelection(null, { model: 'picked-model' });
    globalThis.fetch = vi.fn((url: unknown, init?: RequestInit) => {
      if (String(url).endsWith('/threads') && init?.method === 'POST') {
        return Promise.resolve(new Response(JSON.stringify({
          summary_version: 1, compose_epoch: 0, text: '', image_hashes: [], mode: 'lucidos',
        }), { status: 201 }));
      }
      return new Promise<Response>(() => {});
    }) as unknown as typeof fetch;

    const id = ensureFocusedComposeThread();
    await vi.advanceTimersByTimeAsync(0);

    expect(getComposeSelectionOverride(id).model).toBe('picked-model');
  });
});

/** The fences this change replaced must not come back under their old names
 *  (C10 in the plan). */
describe('the retired compose fences stay gone', () => {
  const RETIRED = [
    'composeEditedAt', 'composeSentAt', 'composePutSettledAt', 'composeEditWatermark',
    'pendingComposePuts', 'hasUnsentLocalDraft', 'draftIsSuperseded', 'clearSupersededDraft',
    'applyRemoteCompose', 'resolveEmptyDraftSync', 'shouldSkipSyncWhileEditing', 'resolveComposerText',
  ];
  const src = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
  function sources(dir: string): string[] {
    return readdirSync(dir).flatMap((name: string) => {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) return name === 'node_modules' ? [] : sources(path);
      return /\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
    });
  }

  it('no shipping source names one', () => {
    const hits = sources(src).flatMap((path) => {
      const text = readFileSync(path, 'utf-8');
      return RETIRED.filter((name) => text.includes(name)).map((name) => `${path}: ${name}`);
    });
    expect(hits).toEqual([]);
  });
});
