import type { ThreadComposeState, ThreadMeta } from '../../store/thread-events';
import { type ComposeMode, currentComposeMode } from '../../store/actions/compose';
import { getDraft } from '../../store/composeDrafts';

interface ComposeFocus {
  meta: { id: string; state: ThreadComposeState; channel: ThreadMeta['channel'] };
}

/** Channel the next send will travel through. Single source for the toggle UI
 *  AND the submit/typing routing — without that consolidation, toggling Claude
 *  on a draft created in Lucidos updated composeMode (toggle UI) but not the
 *  send path, so the message routed via Lucidos despite the UI showing Claude.
 *
 *  - composing thread → draft.mode (mutable via toggle; falls back to global
 *    inputMode while the engine ack with the picked mode is still in flight)
 *  - any other state → channel (locked once the thread goes active; toggle is
 *    hidden but the send path still resolves through here)
 *  - no thread (compose view) → currentComposeMode (the global toggle state) */
export function effectiveSendMode(focusedThread: ComposeFocus | undefined): ComposeMode {
  if (!focusedThread) return currentComposeMode();
  if (focusedThread.meta.state === 'composing') {
    return getDraft(focusedThread.meta.id).mode ?? currentComposeMode();
  }
  return focusedThread.meta.channel === 'claude_code' ? 'claude_code' : 'lucidos';
}
