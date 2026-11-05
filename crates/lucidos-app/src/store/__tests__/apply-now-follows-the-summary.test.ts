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
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
});

vi.mock('../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/client')>()),
  applyNow: vi.fn(),
  applyChange: vi.fn(),
  fetchChanges: vi.fn(),
}));

import { makeThreadState } from '../actions/threads-test-helpers';
import type { ThreadAggregate, ThreadEvent, ThreadState } from '../thread-events';
import { applyingNowThreadIds, applyNowRequests, threadMap } from '../store';
import { flushThreadMap, handleThreadEvent } from '../actions/thread-sync';
import { endClaudeCodeAndApply } from '../actions/chat-claude-code';
import { applyNow, ApiError } from '../../api/client';

type Mock = ReturnType<typeof vi.fn>;

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

/** A coding-agent thread waiting with a change, at summary version 3. */
function seed(): void {
  const map = new Map<string, ThreadState>();
  map.set('t', makeThreadState('t', {
    meta: {
      id: 't', title: 'CC thread', channel: 'claude_code', saved: false, createdAt: '',
      updatedAt: '2026-01-01T00:00:01Z', status: 'waiting',
      codingAgentChangeState: { kind: 'proposed', requires_restart: false },
      codingAgentIsExternalRepo: false, lastRevivedAt: '', messageCount: 1, section: 'inbox',
      activeChildrenCount: 0, summaryVersion: 3,
    },
  }));
  threadMap.value = map;
}

/** An SSE frame for `t` at `version`, with the engine's apply flag. */
function frame(version: number, event: ThreadEvent, applyInFlight: boolean, status = 'waiting'): void {
  handleThreadEvent({
    thread_id: 't',
    seq: 100 + version,
    created: '2026-01-01T00:00:02Z',
    event,
    aggregate: {
      threadId: 't', title: 'CC thread', channel: 'claude_code', initiator: 'user', createdAt: '',
      lastActivity: '2026-01-01T00:00:02Z', messageCount: 1, section: 'inbox', status,
      activeChildrenCount: 0, totalChildrenCount: 0, blockingDescendantCount: 0,
      attentionDescendantCount: 0, codingAgentChangeState: { kind: 'proposed', requires_restart: false },
      codingAgentIsExternalRepo: false, isSaved: false, hasResponse: true, lastRevivedAt: null,
      parentThreadId: null, parentThreadTitle: null, state: 'active',
      summaryVersion: version, applyInFlight,
    } as unknown as ThreadAggregate,
  });
  flushThreadMap();
}

const applying = () => applyingNowThreadIds.value.has('t');

beforeEach(() => {
  applyNowRequests.value = new Map();
  seed();
  vi.resetAllMocks();
  // Timers only: the thread map flushes on an animation frame, which must run.
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] });
});

afterEach(() => {
  vi.useRealTimers();
});

// "Applying" is the engine's own word, projected on the thread summary from
// ApplyNowStarted to ApplyNowEnded. Nothing else ends it: not a message from
// another device, not a late proposal, not a timer (ADR 0437).
describe('Apply Now follows the summary', () => {
  it('holds from the click until the engine ends the apply, whatever arrives meanwhile', async () => {
    const post = deferred<{ status: string; summary_version: number }>();
    (applyNow as Mock).mockReturnValueOnce(post.promise);

    const requesting = endClaudeCodeAndApply('t');
    expect(applying()).toBe(true);
    post.resolve({ status: 'applying', summary_version: 5 });
    await requesting;
    expect(applying()).toBe(true);

    frame(5, { type: 'ApplyNowStarted' } as unknown as ThreadEvent, true);
    frame(6, { type: 'MessageReceived', text: 'from my phone', mode: 'human', event_id: 'm1' } as unknown as ThreadEvent, true, 'running');
    expect(applying()).toBe(true);

    frame(7, { type: 'SessionEnded' } as unknown as ThreadEvent, true);
    vi.advanceTimersByTime(31_000);
    frame(8, { type: 'ChangeProposed', change_id: 'c1' } as unknown as ThreadEvent, true);
    expect(applying()).toBe(true);

    frame(9, { type: 'ApplyNowEnded', outcome: 'finished' } as unknown as ThreadEvent, false);
    expect(applying()).toBe(false);
  });

  it('a refusal for a running apply arms no timer: the summary ends it', async () => {
    const message = 'An apply is already in progress for this thread';
    (applyNow as Mock).mockRejectedValueOnce(new ApiError(409, message, { error: message, reason: 'apply_in_progress' }));

    // Another device started the apply, and its frame lands after the refusal.
    await endClaudeCodeAndApply('t');
    expect(applyNow).toHaveBeenCalledOnce();
    frame(4, { type: 'ApplyNowStarted' } as unknown as ThreadEvent, true);
    vi.advanceTimersByTime(61_000);
    expect(applying()).toBe(true);

    frame(5, { type: 'ApplyNowEnded', outcome: 'finished' } as unknown as ThreadEvent, false);
    expect(applying()).toBe(false);
  });

  it('an unversioned answer ends the bridge when a frame beat the response', async () => {
    const post = deferred<{ status: string }>();
    (applyNow as Mock).mockReturnValueOnce(post.promise);

    const requesting = endClaudeCodeAndApply('t');
    frame(20, { type: 'SessionEnded' } as unknown as ThreadEvent, false);
    post.resolve({ status: 'applying' });
    await requesting;

    expect(applying()).toBe(false);
  });

  it('a failed request ends the bridge at once', async () => {
    (applyNow as Mock).mockRejectedValueOnce(new ApiError(500, 'boom'));

    await endClaudeCodeAndApply('t');

    expect(applying()).toBe(false);
  });
});
