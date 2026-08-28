/**
 * The shell's live event-stream transport, held in one place.
 *
 * `thread-sync.ts` owns the connection lifecycle and writes it here.
 * `presence-pong.ts` reads it to submit a pong. A module between them is what
 * keeps the two from importing each other: thread-sync already imports the
 * PresenceCheck handler, so presence-pong cannot import back.
 */

import type { EventStream, PongAnswer } from '@lucidos/event-stream';

let current: EventStream | null = null;

export function getEventStream(): EventStream | null {
  return current;
}

export function setEventStream(stream: EventStream | null): void {
  current = stream;
}

/** Hand this document's `PresenceCheck` answer to the live transport.
 *
 *  Where that lands depends on the transport, and the caller must not care.
 *  Direct: POSTed straight away. Shared: sent to the worker, which ORs it with
 *  its other ports' answers and POSTs exactly one pong for the workspace.
 *
 *  That indirection is what holds the engine's `expected_pong_count` equal to
 *  its open-connection count. It waits for one pong per open SSE stream
 *  (`crates/lucidos-engine/src/scheduler/push.rs`), so N documents behind one
 *  connection owe it one answer, not N. */
export function submitPong(notificationId: string, answer: PongAnswer): void {
  if (!current) {
    // Telemetry carve-out (.claude/rules/frontend.md): a PresenceCheck can only
    // have arrived over a stream, so this is unreachable in practice and names
    // no user-facing operation. The engine's deadline-then-push fallback covers
    // the missing pong, so the user gets an OS notification rather than none.
    console.warn('[PresencePong] no live event stream to answer through');
    return;
  }
  current.submitPong(notificationId, answer);
}
