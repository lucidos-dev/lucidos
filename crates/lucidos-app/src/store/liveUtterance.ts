/**
 * The caller's bubble, from the moment they start speaking.
 *
 * A call and a transcript know nothing about each other, so this is the one
 * seam between them. It reads the call's own state and writes a single row onto
 * the thread the call is running on. Nothing here decides whether the caller is
 * speaking: `voice/callState.ts` does, and this only draws the answer.
 *
 * The row goes two ways, and the other one is not here. `handleEvent` drops it
 * the moment the caller's real words land, because that is the swap the reader
 * sees. This end handles every way an utterance ends with no words at all.
 *
 * Built through a factory so a test supplies its own call and its own clock.
 * The live instance is the last few lines.
 */
import { type Signal, effect } from '@preact/signals';
import { threadMap } from './store';
import { bumpThreadEvents } from './threadActivity';
import { voiceCall } from './voice';
import type { CallState } from '../voice/callState';
import type { LiveUtterance } from './thread-events';

export interface LiveUtteranceDeps {
  call: Signal<CallState>;
  draw(threadId: string, row: LiveUtterance): void;
  erase(threadId: string): void;
  /** When the row was drawn, as the bubble header will show it. */
  now(): string;
}

export interface LiveUtteranceBridge {
  /** Stop watching the call. For a test, and for nothing else. */
  dispose(): void;
}

export function createLiveUtteranceBridge(deps: LiveUtteranceDeps): LiveUtteranceBridge {
  /**
   * The utterance this bridge has already drawn a row for.
   *
   * What makes it necessary is that the row is erased from the other end. An
   * utterance stays `transcribed` for a moment after its words land. Anything
   * else the call does in that moment would redraw a row for words the reader
   * can already see.
   */
  let drawn: { threadId: string; count: number } | null = null;

  const dispose = effect(() => {
    const call = deps.call.value;
    const threadId = call.threadId;
    if (call.utterance === 'none' || threadId === null) {
      if (drawn) deps.erase(drawn.threadId);
      drawn = null;
      return;
    }
    if (drawn && drawn.threadId === threadId && drawn.count === call.utteranceCount) return;
    if (drawn) deps.erase(drawn.threadId);
    drawn = { threadId, count: call.utteranceCount };
    deps.draw(threadId, {
      eventId: liveUtteranceId(threadId, call.utteranceCount),
      count: call.utteranceCount,
      created: deps.now(),
    });
  });

  return { dispose };
}

/** The row's render key. Not a `uuid`: no event ever carries this id, and one
 *  that reads as a real event id would be the harder thing to trace. */
export function liveUtteranceId(threadId: string, count: number): string {
  return `live-utterance:${threadId}:${count}`;
}

/** Put the row on the thread, or take it off. Paired with the per-thread bump
 *  for the same reason `addPendingMessage` is: `activeExchanges` subscribes to
 *  the events bell rather than to `threadMap`, so a map write alone leaves the
 *  focused transcript painting its cached exchanges. */
function writeRow(threadId: string, row: LiveUtterance | null): void {
  const map = threadMap.peek();
  const thread = map.get(threadId);
  if (!thread) return;
  if (thread.liveUtterance === row) return;
  // A new call counts its utterances from one again, so the tally of landed
  // words starts over with it. Without this, a thread carrying a long previous
  // call would clear the next call's first row on any utterance at all.
  if (row?.count === 1) thread.settledUtterances = 0;
  thread.liveUtterance = row;
  threadMap.value = new Map(map);
  bumpThreadEvents(threadId);
}

let live: LiveUtteranceBridge | null = null;

/** Start drawing the caller's bubble. Called once, from `store/effects.ts`. */
export function installLiveUtteranceRow(): void {
  live ??= createLiveUtteranceBridge({
    call: voiceCall,
    draw: (threadId, row) => writeRow(threadId, row),
    erase: (threadId) => writeRow(threadId, null),
    now: () => new Date().toISOString(),
  });
}
