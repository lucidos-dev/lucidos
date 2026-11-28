
import type { EventStream, PongAnswer } from '@lucidos/event-stream';

let current: EventStream | null = null;
let framesReceived = 0;

export function getEventStream(): EventStream | null {
  return current;
}

export function setEventStream(stream: EventStream | null): void {
  current = stream;
}

export function noteEventFrame(): void {
  framesReceived += 1;
}

export function eventFramesReceived(): number {
  return framesReceived;
}

export function restartEventStream(): 'direct' | 'shared' | null {
  if (!current) return null;
  const transport = current.ownsReconnect ? 'shared' : 'direct';
  current.restart();
  return transport;
}

export function submitPong(notificationId: string, answer: PongAnswer): void {
  if (!current) {
    // BEST-EFFORT: telemetry carve-out (.claude/rules/frontend.md). A
    // PresenceCheck can only have arrived over a stream, so this is
    // unreachable in practice and names
    // no user-facing operation. The engine's deadline-then-push fallback covers
    // the missing pong, so the user gets an OS notification rather than none.
    console.warn('[PresencePong] no live event stream to answer through');
    return;
  }
  current.submitPong(notificationId, answer);
}
