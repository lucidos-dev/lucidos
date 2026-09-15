/**
 * Both sides of a call on the transcript, from the first word to the moment
 * the engine's own row replaces it.
 *
 * A call and a transcript know nothing about each other, so this is the one
 * seam between them. It reads the call's own state and writes rows onto the
 * thread the call is running on. Nothing here decides who is speaking:
 * `voice/callState.ts` does, and this only draws the answer.
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
import { claimUtteranceRows } from './thread-events';
import type { CallState } from '../voice/callState';
import type { LiveReply, LiveUtterance } from './thread-events';

export interface LiveUtteranceDeps {
  call: Signal<CallState>;
  /** Put this row on the thread.
   *
   *  `fresh` says the row BEGINS an utterance, rather than rewriting one this
   *  bridge already drew. Only the writer can act on the difference: a rewrite
   *  keeps the moment the row went up, and a fresh row at count one means a
   *  new call, whose ledger starts over. */
  draw(threadId: string, row: LiveUtterance, fresh: boolean): void;
  /** Take the row with this count off the thread. */
  erase(threadId: string, count: number): void;
  /** Put the talker's row on the thread, or rewrite the one standing. */
  drawReply(threadId: string, row: LiveReply): void;
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
   * The words are held here as well as on the row because the call forgets
   * them: `heard` belongs to the CURRENT utterance, and the row outlives that.
   * What this remembers is whether the row it drew is still a promise
   * (withdraw it) or already the caller's finished words (leave it standing).
   * A partial is a promise: it is the sentence still being said.
   */
  let drawn: { threadId: string; count: number; text?: string; partial?: string } | null = null;

  /** The reply row's identity, so a rewrite keeps the moment it went up. */
  let reply: { threadId: string; count: number; created: string } | null = null;

  /** Draw or rewrite the row for the call's current utterance. */
  const paint = (threadId: string, call: CallState): void => {
    const text = call.heard ?? undefined;
    const partial = text === undefined ? call.hearing ?? undefined : undefined;
    const prior = drawn && drawn.threadId === threadId && drawn.count === call.utteranceCount
      ? drawn
      : null;
    if (prior) {
      // Same row. Only the words can have changed, and a revision rewrites it
      // in place rather than adding a second bubble.
      if (prior.text === text && prior.partial === partial) return;
      drawn = { ...prior, text, partial };
    } else {
      // A NEW utterance. The row before it stays exactly when it has FINAL
      // words: those are owed a bubble until the engine writes them down.
      if (drawn && drawn.text === undefined) deps.erase(drawn.threadId, drawn.count);
      drawn = { threadId, count: call.utteranceCount, text, partial };
    }
    deps.draw(threadId, {
      eventId: liveUtteranceId(threadId, call.utteranceCount),
      count: call.utteranceCount,
      created: deps.now(),
      ...(text === undefined ? {} : { text }),
      ...(partial === undefined ? {} : { partial }),
    }, !prior);
  };

  /**
   * Draw or rewrite the talker's row.
   *
   * Called on every delta, and deliberately NOT memoized against the store. A
   * persisted row for the PREVIOUS reply can clear the slot mid-reply, and the
   * next delta is what puts this one back.
   */
  const paintReply = (threadId: string, call: CallState): void => {
    const standing = reply && reply.threadId === threadId && reply.count === call.replyCount
      ? reply
      : { threadId, count: call.replyCount, created: deps.now() };
    reply = standing;
    deps.drawReply(threadId, {
      eventId: liveReplyId(threadId, call.replyCount),
      created: standing.created,
      text: call.said,
    });
  };

  const dispose = effect(() => {
    const call = deps.call.value;
    const threadId = call.threadId;
    if (call.utterance === 'none' || threadId === null) {
      // The utterance is over. A row with no final words promised some that
      // will never come, so it goes. One with them has delivered on itself.
      if (drawn && drawn.text === undefined) deps.erase(drawn.threadId, drawn.count);
      drawn = null;
    } else {
      paint(threadId, call);
    }
    if (threadId === null) {
      reply = null;
      return;
    }
    // The reply's row outlives `said`, so an empty one withdraws nothing. Only
    // `SpokenReplyGenerated` and the session's end retire it.
    if (call.said !== '') paintReply(threadId, call);
  });

  return { dispose };
}

/** The row's render key. Not a `uuid`: no event ever carries this id, and one
 *  that reads as a real event id would be the harder thing to trace. */
export function liveUtteranceId(threadId: string, count: number): string {
  return `live-utterance:${threadId}:${count}`;
}

/** The same for the talker's row, and for the same reason. */
export function liveReplyId(threadId: string, count: number): string {
  return `live-reply:${threadId}:${count}`;
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
    // A new call counts its utterances from one again, so the ledger starts
    // over with them. Anything the LAST call left un-landed goes with it.
    // `call.rs` writes down whatever it holds however a call ends. So a row
    // still here outlived its own words, and its count is about to be reused.
    if (fresh && count === 1) {
      rows = [];
      thread.unclaimedUtterances = [];
      thread.liveReply = undefined;
    }
    const at = rows.findIndex(r => r.count === count);
    // A REWRITE of a row that is no longer here means it has been claimed: the
    // engine wrote those words down while the bridge still held them. Putting
    // it back would paint the sentence twice, which is the defect the ledger
    // exists to end. A provider revising its own transcript is how you get
    // here, since that is the one thing that rewrites a row with words.
    if (!fresh && at === -1) return;
    if (!fresh && sameRow(rows[at], row)) return;
    // A rewrite keeps the moment the row first went up, so better words do not
    // move the bubble's timestamp.
    const next = !fresh && at !== -1 ? { ...row, created: rows[at].created } : row;
    // Oldest first, and a row is only ever appended or rewritten in place, so
    // the order holds without a sort.
    thread.liveUtterances = at === -1 ? [...rows, next] : rows.map(r => (r.count === count ? next : r));
    // A row that just gained its FINAL words may be one a persisted row is
    // already waiting to claim. `call.rs` emits that row before it sends the
    // frame carrying these words, so the debt is often owed before they land.
    if (next.text !== undefined) claimUtteranceRows(thread);
  }
  threadMap.value = new Map(map);
  bumpThreadEvents(threadId);
}

/** Put the talker's row on the thread, or rewrite the one standing. */
function writeReply(threadId: string, row: LiveReply): void {
  const map = threadMap.peek();
  const thread = map.get(threadId);
  if (!thread) return;
  const standing = thread.liveReply;
  if (standing && standing.eventId === row.eventId && standing.text === row.text) return;
  thread.liveReply = row;
  threadMap.value = new Map(map);
  bumpThreadEvents(threadId);
}

/** Would redrawing change anything the reader can see? `created` is excluded:
 *  a row rewritten with better words keeps the moment it first went up, so the
 *  bubble's timestamp does not jump. */
function sameRow(a: LiveUtterance, b: LiveUtterance): boolean {
  return a.eventId === b.eventId && a.text === b.text && a.partial === b.partial;
}

let live: LiveUtteranceBridge | null = null;

/** Start drawing the call's rows. Called once, from `store/effects.ts`. */
export function installLiveUtteranceRow(): void {
  live ??= createLiveUtteranceBridge({
    call: voiceCall,
    draw: (threadId, row, fresh) => writeRow(threadId, row.count, row, fresh),
    erase: (threadId, count) => writeRow(threadId, count, null),
    drawReply: writeReply,
    now: () => new Date().toISOString(),
  });
}
