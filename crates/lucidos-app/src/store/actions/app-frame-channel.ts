import { BRIDGE_PUSH_TYPE, HOST_REQUEST_TYPE, HOST_REPLY_TYPE } from '../../../../../packages/lucidos-sdk/src/_bridge';


interface SseSubscriber {
  frame: HTMLIFrameElement;
  types: ReadonlySet<string> | null;
}

const sseSubscribers = new Map<MessageEventSource, SseSubscriber>();

function listenedTypes(args: unknown): ReadonlySet<string> | null {
  const types = (args as { types?: unknown } | null)?.types;
  if (!Array.isArray(types)) return null;
  return new Set(types.filter((t): t is string => typeof t === 'string'));
}

export function frameEventTypes(data: string): string[] {
  let parsed: { type?: unknown; data?: { event?: { type?: unknown } } } | null;
  try {
    parsed = JSON.parse(data);
  } catch {
    return [];
  }
  const outer = parsed?.type;
  if (typeof outer !== 'string') return [];
  if (outer !== 'ThreadEvent') return [outer];
  const inner = parsed?.data?.event?.type;
  return typeof inner === 'string' ? [inner, outer] : [outer];
}

function wants(subscriber: SseSubscriber, types: () => readonly string[]): boolean {
  if (!subscriber.types || subscriber.types.has('*')) return true;
  return types().some((t) => subscriber.types!.has(t));
}

let hostStreamOpen = false;

export function postToFrame(target: MessageEventSource, message: unknown): void {
  (target as Window).postMessage(message, '*');
}

function fanOut(payload: unknown, accepts: (subscriber: SseSubscriber) => boolean = () => true): void {
  for (const [target, subscriber] of [...sseSubscribers]) {
    if (!subscriber.frame.isConnected) {
      sseSubscribers.delete(target);
      continue;
    }
    if (accepts(subscriber)) postStreamTo(target, payload);
  }
}

function postStreamTo(target: MessageEventSource, payload: unknown): void {
  postToFrame(target, { type: BRIDGE_PUSH_TYPE, channel: 'sse', data: payload });
}

export function postToAppFrame(
  frame: HTMLIFrameElement,
  channel: string,
  data: unknown,
): boolean {
  const target = frame.contentWindow;
  if (!target) return false;
  postToFrame(target, { type: BRIDGE_PUSH_TYPE, channel, data });
  return true;
}

export function fanOutEventFrame(data: string): void {
  if (sseSubscribers.size === 0) return;
  let types: string[] | null = null;
  const frameTypes = () => (types ??= frameEventTypes(data));
  fanOut({ kind: 'frame', data }, (subscriber) => wants(subscriber, frameTypes));
}

export function fanOutEventStreamStatus(kind: 'open' | 'error'): void {
  hostStreamOpen = kind === 'open';
  if (sseSubscribers.size === 0) return;
  fanOut({ kind });
}


let askCounter = 0;

export function tellFrame(frame: HTMLIFrameElement, op: string, args: unknown): boolean {
  const target = frame.contentWindow;
  if (!target) return false;
  target.postMessage({ type: HOST_REQUEST_TYPE, op, args }, '*');
  return true;
}

export function askFrame(
  frame: HTMLIFrameElement,
  op: string,
  args: unknown,
  timeoutMs: number,
): Promise<unknown> {
  const target = frame.contentWindow;
  if (!target) return Promise.reject(new Error('the app frame has no browsing context'));
  const id = `host-${++askCounter}`;
  return new Promise((resolve, reject) => {
    const settle = (fn: () => void) => {
      window.removeEventListener('message', onReply);
      clearTimeout(timer);
      fn();
    };
    const timer = setTimeout(
      () => settle(() => reject(new Error(`the app did not answer "${op}" in ${timeoutMs}ms`))),
      timeoutMs,
    );
    const onReply = (event: MessageEvent) => {
      if (event.source !== target) return;
      const data = event.data as { type?: string; id?: string; ok?: boolean; value?: unknown; error?: string } | null;
      if (!data || data.type !== HOST_REPLY_TYPE || data.id !== id) return;
      settle(() => (data.ok
        ? resolve(data.value)
        : reject(new Error(data.error || `the app refused "${op}"`))));
    };
    window.addEventListener('message', onReply);
    target.postMessage({ type: HOST_REQUEST_TYPE, id, op, args }, '*');
  });
}

export function subscribeAppEvents(source: MessageEventSource, frame: HTMLIFrameElement, args: unknown): void {
  sseSubscribers.set(source, { frame, types: listenedTypes(args) });
  if (hostStreamOpen) postStreamTo(source, { kind: 'open' });
}

export function updateAppEventTypes(source: MessageEventSource, args: unknown): void {
  const subscriber = sseSubscribers.get(source);
  if (subscriber) subscriber.types = listenedTypes(args);
}

export function unsubscribeAppEvents(source: MessageEventSource): void { sseSubscribers.delete(source); }
export function clearAppEventSubscribers(): void { sseSubscribers.clear(); }
export function resetAppEventStream(): void { sseSubscribers.clear(); hostStreamOpen = false; }
