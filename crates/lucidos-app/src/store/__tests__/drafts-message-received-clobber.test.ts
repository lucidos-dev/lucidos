/**
 * Regression: drafts:65 value='' — a locally-typed, unsent follow-up draft on an
 * ACTIVE thread is blanked when a `!isFromThisDevice` MessageReceived echo for
 * that thread is processed while the user has navigated away (e.g. to the
 * compose view), then the user switches back and the textarea is empty.
 *
 * Root cause: the MessageReceived compose-clear in handleThreadEvent
 * (thread-sync.ts) called `clearComposeIfUnfocused` UNCONDITIONALLY — it was the
 * ONE inbound compose-clear path missing the "never blank a non-empty,
 * locally-edited draft" invariant that `stageDraftFromApi` (thread-loading.ts),
 * `applyRemoteCompose` (compose.ts), and `upsertThread`'s compose overwrite all
 * enforce via `hasUnsentLocalDraft`. The handler's own comment claims a "layer 2:
 * when this user is mid-keystroke, drop the inbound clear" but only checked
 * `isComposeFocusedHere` (a *composing* thread), so an active thread with a typed
 * follow-up draft was unprotected.
 *
 * Reproduced live at retries:0 on a release-engine mobile-webkit run
 * (docs/plans/2026-06-27-mobile-webkit-shard-contention.md session 6). In e2e /
 * cross-device the user's own echoed MessageReceived can carry a device_id that
 * does not match getDeviceId(), so isFromThisDevice is false and the echo wiped
 * the draft.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap, focusedThreadId } from '../store';
import { handleThreadEvent } from '../actions/thread-sync';
import { composeEditedAt } from '../actions/compose';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from '../composeDrafts';
import type { ThreadState } from '../thread-events';

// handleThreadEvent batches its threadMap flush via requestAnimationFrame; the
// compose-clear side effect itself is synchronous (clearDraft writes the signal
// immediately), so the draft assertions need no rAF wait.
vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(cb, 0));
vi.stubGlobal('cancelAnimationFrame', (id: number) => clearTimeout(id));

function makeActiveThread(id: string): ThreadState {
  return {
    meta: {
      id,
      title: 'T',
      channel: 'chat',
      initiator: 'user',
      saved: false,
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-01T00:00:00Z',
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

describe("drafts:65 — MessageReceived echo must not blank a locally-edited draft", () => {
  beforeEach(() => {
    threadMap.value = new Map();
    focusedThreadId.value = null;
    composeEditedAt.clear();
    _resetComposeDraftsForTesting();
  });

  it('keeps a non-empty, locally-typed follow-up draft when a !isFromThisDevice MessageReceived for the active thread is processed while the user is off the thread', () => {
    const T = 'thread-T';
    threadMap.value = new Map([[T, makeActiveThread(T)]]);

    // Model the post-typing state: the user typed a follow-up draft on the active
    // thread (non-empty draft + composeEditedAt stamped), then navigated to the
    // compose view (focusedThreadId = null → T is not compose-focused-here).
    setDraft(T, { text: 'thread draft text', image_hashes: [], mode: null });
    composeEditedAt.set(T, 1);
    expect(getDraft(T).text).toBe('thread draft text');

    // The user's own MessageReceived for T is echoed back over SSE with a
    // device_id that does NOT match getDeviceId() (the e2e / cross-device case),
    // so isFromThisDevice is false and the unconditional clear used to fire.
    handleThreadEvent({
      thread_id: T,
      seq: 1,
      event: { type: 'MessageReceived', text: 'an earlier message', device_id: 'peer-device' },
      created: '2026-06-28T00:00:00Z',
    });

    // The locally-edited, unsent draft must survive — the value='' face.
    expect(getDraft(T).text).toBe('thread draft text');
  });

  it('still clears a non-empty draft that was NOT locally edited (server-originated) on an inbound MessageReceived', () => {
    const T = 'thread-T2';
    threadMap.value = new Map([[T, makeActiveThread(T)]]);

    // A draft present but never edited on THIS device (no composeEditedAt entry) —
    // e.g. hydrated from the server. This stays clearable, mirroring
    // applyRemoteCompose's "server-originated draft is still clearable" boundary.
    setDraft(T, { text: 'server draft', image_hashes: [], mode: null });
    focusedThreadId.value = null;

    handleThreadEvent({
      thread_id: T,
      seq: 1,
      event: { type: 'MessageReceived', text: 'msg', device_id: 'peer-device' },
      created: '2026-06-28T00:00:00Z',
    });

    expect(getDraft(T).text).toBe('');
  });
});
