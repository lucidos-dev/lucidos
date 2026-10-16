import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { HANDOFF_REQUEST } from '@lucidos/engine-constants';
import { fetchThreadContext, sendToSuccessor, type ThreadContext } from '../../api/client';
import { effectiveThreadStatus, focusedThreadId, showToast, successorOffer, threadMap } from '../../store/store';
import { getDraft } from '../../store/composeDrafts';
import { getThreadEventsBump } from '../../store/threadActivity';
import { sendFollowup, updateCompose } from '../../store/actions/compose';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import type { ThreadState } from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { formatTokens } from '../../utils/formatTokens';
import { Disclosure } from '../shared/Disclosure';

type Offer = 'send' | 'continue';

const BUTTON_LABEL: Record<Offer, string> = {
  send: 'Send to a new thread',
  continue: 'Continue in a new thread',
};

/** The strip's size label, from the engine's reading. */
export function contextSizeLabel(context: ThreadContext): string {
  return context.input_tokens === null ? 'A large thread' : `${formatTokens(context.input_tokens)} tokens here`;
}

/** The events that change a thread's context size. */
const SIZE_EVENTS: ReadonlySet<string> = new Set(['ContextCaptured', 'CodingAgentCompacted']);

/** When to re-read the focused thread's context: once per idle, after its
 *  newest model call or compaction. `null` while a turn runs, so a busy turn
 *  reads nothing. */
function contextReadKey(thread: ThreadState | undefined): string | null {
  if (!thread || thread.meta.channel !== 'claude_code' || effectiveThreadStatus(thread) !== 'idle') return null;
  let newest = -1;
  for (const [seq, event] of thread.events) if (SIZE_EVENTS.has(event.type) && seq > newest) newest = seq;
  return `${thread.meta.id}:${newest}`;
}

/** A strip on top of the composer offering a *successor thread* to a large
 *  coding-agent thread at a clean idle (`successorOffer`). */
export function SuccessorStrip() {
  const threadId = focusedThreadId.value;
  const thread = threadId ? threadMap.value.get(threadId) : undefined;
  const eventsBump = threadId ? getThreadEventsBump(threadId) : 0;
  const readKey = useMemo(() => contextReadKey(thread), [thread, eventsBump, thread?.meta.status]);
  // The latest reading per thread, so switching back to one needs no refetch.
  const [readings, setReadings] = useState<ReadonlyMap<string, { key: string; context: ThreadContext }>>(new Map());
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!readKey || !threadId || readings.get(threadId)?.key === readKey) return;
    let live = true;
    fetchThreadContext(threadId).then(
      (context) => {
        if (live) setReadings((prev) => new Map(prev).set(threadId, { key: readKey, context }));
      },
      (err) => {
        // Best-effort carve-out (frontend.md), so no toast: the user did not
        // ask for this offer, and one that cannot be measured is simply
        // absent. The next turn end reads again.
        console.warn('[SuccessorStrip] context read failed', err);
      },
    );
    return () => { live = false; };
  }, [readKey]);

  // Only a reading of the thread as it stands now: an older one may predate a
  // compaction, and would open the strip just to shut it.
  const reading = threadId ? readings.get(threadId) : undefined;
  const context = reading && reading.key === readKey ? reading.context : null;
  const draft = getDraft(threadId);
  const offer = successorOffer(thread, context, { text: draft.text, imageCount: draft.image_hashes.length });
  // The last offer shown, so the strip keeps its words while it rolls shut.
  const shown = useRef<{ threadId: string; offer: Offer; label: string } | null>(null);
  if (threadId && offer && context) shown.current = { threadId, offer, label: contextSizeLabel(context) };
  const last = shown.current;

  async function act(target: { threadId: string; offer: Offer }): Promise<void> {
    setBusy(true);
    try {
      if (target.offer === 'send') {
        const spawned = await sendToSuccessor(target.threadId, getDraft(target.threadId).text.trim());
        updateCompose(target.threadId, { text: '', image_hashes: [] });
        focusThreadOrBootstrap(spawned.successor_thread_id);
      } else {
        await sendFollowup(target.threadId, HANDOFF_REQUEST);
      }
    } catch (err) {
      showToast(`Could not continue in a new thread: ${errorDetail(err)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  return (
    <Disclosure open={offer !== null}>
      {last && (
        <div class="successor-strip" data-offer={last.offer}>
          <span class="successor-strip-label">{last.label}</span>
          <button class="action-btn protected-surface" disabled={busy} onClick={() => void act(last)}>
            {BUTTON_LABEL[last.offer]}
          </button>
        </div>
      )}
    </Disclosure>
  );
}
