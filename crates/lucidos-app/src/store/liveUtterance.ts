import { type Signal, effect } from '@preact/signals';
import { threadMap } from './store';
import { bumpThreadEvents } from './threadActivity';
import { voiceCall } from './voiceCall';
import { claimUtteranceRows } from './thread-events';
import type { CallState } from '../voice/callState';
import type { LiveReply, LiveUtterance } from './thread-events';

export interface LiveUtteranceDeps {
  call: Signal<CallState>;
  draw(threadId: string, row: LiveUtterance, fresh: boolean): void;
  erase(threadId: string, count: number): void;
  drawReply(threadId: string, row: LiveReply, fresh: boolean): boolean;
  now(): string;
}

export interface LiveUtteranceBridge {
  dispose(): void;
}

export function createLiveUtteranceBridge(deps: LiveUtteranceDeps): LiveUtteranceBridge {
  let drawn: { threadId: string; count: number; text?: string; partial?: string } | null = null;

  let reply: {
    threadId: string;
    count: number;
    created: string;
    stretch: number;
    before: string;
    said: string;
  } | null = null;

  const paint = (threadId: string, call: CallState): void => {
    const text = call.heard ?? undefined;
    const partial = text === undefined ? call.hearing ?? undefined : undefined;
    const prior = drawn && drawn.threadId === threadId && drawn.count === call.utteranceCount
      ? drawn
      : null;
    if (prior) {
      if (prior.text === text && prior.partial === partial) return;
      drawn = { ...prior, text, partial };
    } else {
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

  const paintReply = (threadId: string, call: CallState): void => {
    const standing = reply && reply.threadId === threadId ? reply : null;
    const row = standing ?? {
      threadId,
      count: call.replyCount,
      created: deps.now(),
      stretch: call.replyCount,
      before: '',
      said: '',
    };
    const before = row.stretch === call.replyCount ? row.before : row.before + row.said;
    const text = before + call.said;
    reply = { ...row, stretch: call.replyCount, before, said: call.said };
    const held = deps.drawReply(threadId, {
      eventId: liveReplyId(threadId, row.count),
      created: row.created,
      text,
    }, standing === null);
    if (held) return;
    reply = null;
    paintReply(threadId, call);
  };

  const dispose = effect(() => {
    const call = deps.call.value;
    const threadId = call.threadId;
    if (call.utterance === 'none' || threadId === null) {
      if (drawn && drawn.text === undefined) deps.erase(drawn.threadId, drawn.count);
      drawn = null;
    } else {
      paint(threadId, call);
    }
    if (threadId === null) {
      reply = null;
      return;
    }
    if (call.said !== '') paintReply(threadId, call);
  });

  return { dispose };
}

export function liveUtteranceId(threadId: string, count: number): string {
  return `live-utterance:${threadId}:${count}`;
}

export function liveReplyId(threadId: string, count: number): string {
  return `live-reply:${threadId}:${count}`;
}

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
    if (fresh && count === 1) {
      rows = [];
      thread.unclaimedUtterances = [];
      thread.liveReply = undefined;
    }
    const at = rows.findIndex(r => r.count === count);
    if (!fresh && at === -1) return;
    if (!fresh && sameRow(rows[at], row)) return;
    const next = !fresh && at !== -1 ? { ...row, created: rows[at].created } : row;
    thread.liveUtterances = at === -1 ? [...rows, next] : rows.map(r => (r.count === count ? next : r));
    if (next.text !== undefined) claimUtteranceRows(thread);
  }
  threadMap.value = new Map(map);
  bumpThreadEvents(threadId);
}

function writeReply(threadId: string, row: LiveReply, fresh: boolean): boolean {
  const map = threadMap.peek();
  const thread = map.get(threadId);
  if (!thread) return true;
  const standing = thread.liveReply;
  if (!fresh && !standing) return false;
  if (standing && standing.eventId === row.eventId && standing.text === row.text) return true;
  thread.liveReply = row;
  threadMap.value = new Map(map);
  bumpThreadEvents(threadId);
  return true;
}

function sameRow(a: LiveUtterance, b: LiveUtterance): boolean {
  return a.eventId === b.eventId && a.text === b.text && a.partial === b.partial;
}

let live: LiveUtteranceBridge | null = null;

export function installLiveUtteranceRow(): void {
  live ??= createLiveUtteranceBridge({
    call: voiceCall,
    draw: (threadId, row, fresh) => writeRow(threadId, row.count, row, fresh),
    erase: (threadId, count) => writeRow(threadId, count, null),
    drawReply: writeReply,
    now: () => new Date().toISOString(),
  });
}
