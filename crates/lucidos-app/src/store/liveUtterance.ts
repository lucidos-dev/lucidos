/**
 * The caller's bubble, from the moment they start speaking to the moment the
 * engine's own row replaces it.
 *
 * A call and a transcript know nothing about each other, so this is the one
 * seam between them. It reads the call's own state and writes rows onto the
 * thread the call is running on. Nothing here decides whether the caller is
 * speaking: `voice/callState.ts` does, and this only draws the answer.
 *
 * **A row carrying words is never withdrawn from this end** (ADR 0174). The
 * engine holds a finished utterance across the talker's decision, and writes it
 * down for every way a call can end. So the words are always owed a row, and
 * only `handleEvent` retires one, when the engine's row for it lands. This end
 * handles the other case: an utterance that ended with no words at all.
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
  /** Put this row on the thread.
   *
   *  `fresh` says the row BEGINS an utterance, rather than rewriting one this
   *  bridge already drew. Only the writer can act on the difference: a rewrite
   *  keeps the moment the row went up, and a fresh row at count one means a
   *  new call, whose tally starts over. */
  draw(threadId: string, row: LiveUtterance, fresh: boolean): void;
  /** Take the row with this count off the thread. */
  erase(threadId: string, count: number): void;
  /** When the row was drawn, as the bubble header will show it. */
  now(): string;
}

export interface LiveUtteranceBridge {
  /** Stop watching the call. For a test, and for nothing else. */
  dispose(): void;
}

export function createLiveUtteranceBridge(deps: LiveUtteranceDeps): LiveUtteranceBridge {
  /**
   * The utterance this bridge has already drawn a row for, and what that row
   * says.
   *
   * The text is held here as well as on the row because the call forgets it:
   * `heard` belongs to the CURRENT utterance, and the row outlives that. What
   * this remembers is whether the row it drew is still a promise (withdraw it)
   * or already the caller's words (leave it standing).
   */
  let drawn: { threadId: string; count: number; text?: string } | null = null;

  /** Draw or rewrite the row for the call's current utterance. */
  const paint = (threadId: string, call: CallState): void => {
    const text = call.heard ?? undefined;
    const prior = drawn && drawn.threadId === threadId && drawn.count === call.utteranceCount
      ? drawn
      : null;
    if (prior) {
      // Same row. Only the words can have changed, and a revision rewrites it
      // in place rather than adding a second bubble.
      if (prior.text === text) return;
      drawn = { ...prior, text };
    } else {
      // A NEW utterance. The row before it stays exactly when it has words:
      // those are owed a bubble until the engine writes them down.
      if (drawn && drawn.text === undefined) deps.erase(drawn.threadId, drawn.count);
      drawn = { threadId, count: call.utteranceCount, text };
    }
    deps.draw(threadId, {
      eventId: liveUtteranceId(threadId, call.utteranceCount),
      count: call.utteranceCount,
      created: deps.now(),
      ...(text === undefined ? {} : { text }),
    }, !prior);
  };

  const dispose = effect(() => {
    const call = deps.call.value;
    const threadId = call.threadId;
    if (call.utterance === 'none' || threadId === null) {
      // The utterance is over. A wordless row promised words that will never
      // come, so it goes. A row with words has already delivered on itself.
      if (drawn && drawn.text === undefined) deps.erase(drawn.threadId, drawn.count);
      drawn = null;
      return;
    }
    paint(threadId, call);
  });

  return { dispose };
}

/** The row's render key. Not a `uuid`: no event ever carries this id, and one
 *  that reads as a real event id would be the harder thing to trace. */
export function liveUtteranceId(threadId: string, count: number): string {
  return `live-utterance:${threadId}:${count}`;
}

/** Put a row on the thread, or take one off. Paired with the per-thread bump
 *  for the same reason `addPendingMessage` is: `activeExchanges` subscribes to
 *  the events bell rather than to `threadMap`, so a map write alone leaves the
 *  focused transcript painting its cached exchanges. */
function writeRow(
  threadId: string,
  count: number,
  row: LiveUtterance | null,
  fresh = false,
): void {
  const map = threadMap.peek();
  const thread = map.get(threadId);
  if (!thread) return;
  let rows = thread.liveUtterances ?? [];
  if (row === null) {
    const left = rows.filter(r => r.count !== count);
    if (left.length === rows.length) return;
    thread.liveUtterances = left;
  } else {
    // A new call counts its utterances from one again, so both tallies start
    // over. Anything the LAST call left un-landed goes with it. `call.rs`
    // writes down whatever it holds however a call ends. So a row still here
    // outlived its own words, and its count is about to be reused.
    if (fresh && count === 1) {
      rows = [];
      thread.settledUtterances = 0;
    }
    const at = rows.findIndex(r => r.count === count);
    if (!fresh && at !== -1 && sameRow(rows[at], row)) return;
    // A rewrite keeps the moment the row first went up, so better words do not
    // move the bubble's timestamp.
    const next = !fresh && at !== -1 ? { ...row, created: rows[at].created } : row;
    // Oldest first, and a row is only ever appended or rewritten in place, so
    // the order holds without a sort.
    thread.liveUtterances = at === -1 ? [...rows, next] : rows.map(r => (r.count === count ? next : r));
  }
  threadMap.value = new Map(map);
  bumpThreadEvents(threadId);
}

/** Would redrawing change anything the reader can see? `created` is excluded:
 *  a row rewritten with better words keeps the moment it first went up, so the
 *  bubble's timestamp does not jump. */
function sameRow(a: LiveUtterance, b: LiveUtterance): boolean {
  return a.eventId === b.eventId && a.text === b.text;
}

let live: LiveUtteranceBridge | null = null;

/** Start drawing the caller's bubble. Called once, from `store/effects.ts`. */
export function installLiveUtteranceRow(): void {
  live ??= createLiveUtteranceBridge({
    call: voiceCall,
    draw: (threadId, row, fresh) => writeRow(threadId, row.count, row, fresh),
    erase: (threadId, count) => writeRow(threadId, count, null),
    now: () => new Date().toISOString(),
  });
}
