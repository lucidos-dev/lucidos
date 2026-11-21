/** Dispatch transport identity separately from event payloads. A system payload's
 * event_id can name another event (for example a notification's tap target). */
interface SSEReceivers {
  thread(data: Record<string, unknown>, eventId?: string): void;
  system(type: string, data: Record<string, unknown>, eventId?: string): void;
  references(type: string, data: Record<string, unknown>, eventId?: string): void;
}

export function dispatchSSEFrame(frame: Record<string, unknown>, receivers: SSEReceivers): void {
  const type = frame.type as string;
  const data = (frame.data ?? {}) as Record<string, unknown>;
  // Thread envelopes carried identity before system envelopes did. Never use
  // a system payload's event_id as an envelope identity or synthesize an id.
  const eventId = typeof frame.event_id === 'string' ? frame.event_id
    : type === 'ThreadEvent' && typeof data.event_id === 'string' ? data.event_id : undefined;
  if (type === 'ThreadEvent') receivers.thread(data, eventId);
  else receivers.system(type, data, eventId);
  receivers.references(type, data, eventId);
}
