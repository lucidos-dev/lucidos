
import { callHost, onHostPush, tellHost } from './_bridge';
import type { EventStream, EventStreamHandlers } from './eventStream';

type Relayed =
  | { kind: 'frame'; data: string }
  | { kind: 'open' }
  | { kind: 'error' };

export function openBridgedEventStream(handlers: EventStreamHandlers, types: readonly string[]): EventStream {
  const off = onHostPush('sse', (payload) => {
    const relayed = payload as Relayed;
    if (relayed?.kind === 'frame') handlers.onFrame(relayed.data);
    else if (relayed?.kind === 'open') handlers.onOpen();
    else if (relayed?.kind === 'error') handlers.onError({ reason: 'error' });
  });

  void callHost('sse.open', { types }).catch(() => {
    handlers.onError({ reason: 'error' });
  });

  return {
    ownsReconnect: true,
    close() {
      off();
      tellHost('sse.close', {});
    },
    submitPong() {},
    revalidate: () => 'live',
    restart() {},
  };
}
