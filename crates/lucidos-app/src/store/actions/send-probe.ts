import { postClientLog } from '../../utils/clientLog';
import { eventFramesReceived, restartEventStream } from './event-stream';
import { refreshThreadEvents } from './thread-loading';

/** How long an accepted send waits for its confirming frame. A live stream
 *  delivers it in milliseconds, even under load. */
export const SEND_CONFIRM_MS = 4000;

/** When a probe last restarted the stream. Sends in a burst each probe, and
 *  one restart answers them all. */
let lastRestartAt = Number.NEGATIVE_INFINITY;

/** Probe the event stream after the engine accepted a send.
 *
 *  The engine emits a frame for an accepted send at once: the message, or the
 *  answered card. When `confirmed()` is still false after `SEND_CONFIRM_MS`,
 *  the stream is suspect. Refetch the thread, which alone repairs what the user
 *  sees. Restart the stream only when that refetch shows the event the stream
 *  never brought. A refetch that failed, or found nothing, proves nothing.
 *
 *  Neither does a frame the stream delivered during the refetch: the event may
 *  have come late on a live stream. A restart in the last `SEND_CONFIRM_MS`
 *  already answered this probe.
 *
 *  It only reads and refetches: it never retries the send or drops its row. */
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
