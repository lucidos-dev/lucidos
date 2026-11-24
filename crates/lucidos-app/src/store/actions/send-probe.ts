import { postClientLog } from '../../utils/clientLog';
import { eventFramesReceived, restartEventStream } from './event-stream';
import { refreshThreadEvents } from './thread-loading';

export const SEND_CONFIRM_MS = 4000;

let lastRestartAt = Number.NEGATIVE_INFINITY;

export function probeStreamAfterSend(threadId: string, confirmed: () => boolean): void {
  setTimeout(async () => {
    if (confirmed()) return;
    const framesBefore = eventFramesReceived();
    const refetched = await refreshThreadEvents(threadId);
    if (!refetched || !confirmed() || eventFramesReceived() !== framesBefore) return;
    if (Date.now() - lastRestartAt < SEND_CONFIRM_MS) return;
    lastRestartAt = Date.now();
    const transport = restartEventStream();
    if (transport) postClientLog('event-stream', 'missed-frame', { transport, waitedMs: SEND_CONFIRM_MS });
  }, SEND_CONFIRM_MS);
}
