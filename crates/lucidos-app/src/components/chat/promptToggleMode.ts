import type { ThreadComposeState, ThreadMeta } from '../../store/thread-events';
import { type ComposeMode, currentComposeMode } from '../../store/actions/compose';
import { getDraft } from '../../store/composeDrafts';
import type { CodingAgent } from '../../api/types';

interface ComposeFocus {
  meta: {
    id: string;
    state: ThreadComposeState;
    channel: ThreadMeta['channel'];
    codingAgent?: CodingAgent;
  };
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

/** Specific backend for the next coding-agent send/control surface.
 *
 *  `channel === "claude_code"` is the historical wire value for every coding
 *  agent, including Codex. While a draft is still composing, the backend is
 *  mutable and lives in the compose picker; once active, the thread summary's
 *  `codingAgent` binding wins. */
export function effectiveCodingAgentBackend(
  focusedThread: ComposeFocus | undefined,
  selectedAgent: CodingAgent,
): CodingAgent | null {
  if (effectiveSendMode(focusedThread) !== 'claude_code') return null;
  if (!focusedThread || focusedThread.meta.state === 'composing') return selectedAgent;
  return focusedThread.meta.codingAgent ?? 'claude-code';
}
