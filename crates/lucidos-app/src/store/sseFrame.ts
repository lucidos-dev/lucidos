interface SSEReceivers {
  thread(data: Record<string, unknown>, eventId?: string): void;
  system(type: string, data: Record<string, unknown>, eventId?: string): void;
  references(type: string, data: Record<string, unknown>, eventId?: string): void;
}

export function dispatchSSEFrame(frame: Record<string, unknown>, receivers: SSEReceivers): void {
  const type = frame.type as string;
  const data = (frame.data ?? {}) as Record<string, unknown>;
  const eventId = typeof frame.event_id === 'string' ? frame.event_id
    : type === 'ThreadEvent' && typeof data.event_id === 'string' ? data.event_id : undefined;
  if (type === 'ThreadEvent') receivers.thread(data, eventId);
  else receivers.system(type, data, eventId);
  receivers.references(type, data, eventId);
}
