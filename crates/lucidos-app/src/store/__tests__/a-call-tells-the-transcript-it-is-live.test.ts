/** A call says it is LIVE, so the transcript's follow carries the reader.
 *
 *  The follow asks the thread whether anything is arriving, and a call answers
 *  no by every measure the scroll module has: every event a call writes is
 *  `Metadata` (ADR 0165), so no turn runs and the projection stays quiescent.
 *  `watchCallLiveness` is the sentence that fixes that, and this pins what it
 *  says and when.
 *
 *  The behaviour it unlocks is pinned where the follow lives:
 *  `components/chat/__tests__/scroll-follow-the-live-edge.test.ts`.
 *
 *  Plan: `docs/plans/2026-09-16-a-call-carries-the-reader-to-the-live-edge.md`.
 */
import { describe, expect, it } from 'vitest';
import { signal } from '@preact/signals';
import { watchCallLiveness } from '../voice';
import { CALL_IDLE, type CallPhase, type CallState } from '../../voice/callState';

const THREAD = 'thread-1';
const OTHER = 'thread-2';

function callOn(threadId: string | null, phase: CallPhase = 'listening'): CallState {
  return { ...CALL_IDLE, phase, threadId };
}

/** The watcher, its inputs, and every answer it has given so far. */
function watching(initial: CallState, focused: string | null) {
  const call = signal<CallState>(initial);
  const focus = signal<string | null>(focused);
  const said: boolean[] = [];
  const dispose = watchCallLiveness({
    call,
    focused: focus,
    setLive: (live) => { said.push(live); },
  });
  return { call, focus, said, dispose, latest: () => said[said.length - 1] };
}

describe('a call tells the transcript it is live', () => {
  it('says nothing is live before a call is placed', () => {
    const w = watching(CALL_IDLE, THREAD);
    expect(w.latest()).toBe(false);
    w.dispose();
  });

  it('says live for every phase a call is actually up in', () => {
    // `isOnCall` is the one definition, so the toggle and the follow cannot
    // disagree about whether a call is running. Connecting counts: the reader
    // has pressed, and the first row can land before the socket settles.
    const w = watching(CALL_IDLE, THREAD);
    for (const phase of ['connecting', 'listening', 'speaking', 'ending'] as CallPhase[]) {
      w.call.value = callOn(THREAD, phase);
      expect(w.latest()).toBe(true);
    }
    w.dispose();
  });

  it('says nothing is live for a call on a thread the reader is not looking at', () => {
    // The follow moves the transcript ON SCREEN. The store ends a call whose
    // thread is left, so this window is short, but it is real: that exit runs
    // after the socket closes.
    const w = watching(callOn(OTHER), THREAD);
    expect(w.latest()).toBe(false);
    w.dispose();
  });

  it('follows the reader off the thread and back onto it', () => {
    const w = watching(callOn(THREAD), THREAD);
    expect(w.latest()).toBe(true);

    w.focus.value = OTHER;
    expect(w.latest()).toBe(false);

    w.focus.value = THREAD;
    expect(w.latest()).toBe(true);
    w.dispose();
  });

  it('says nothing is live once the call rings off', () => {
    const w = watching(callOn(THREAD), THREAD);
    expect(w.latest()).toBe(true);

    w.call.value = CALL_IDLE;

    expect(w.latest()).toBe(false);
    w.dispose();
  });

  it('stops answering at all once disposed', () => {
    // A test's own watcher must not outlive it. The live one is installed for
    // the life of the app and drops its dispose, like the store's departure
    // watcher beside it.
    const w = watching(CALL_IDLE, THREAD);
    w.dispose();
    const before = w.said.length;

    w.call.value = callOn(THREAD);

    expect(w.said.length).toBe(before);
  });
});
