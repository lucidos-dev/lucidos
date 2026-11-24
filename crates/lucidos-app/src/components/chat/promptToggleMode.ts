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

export function effectiveSendMode(focusedThread: ComposeFocus | undefined): ComposeMode {
  if (!focusedThread) return currentComposeMode();
  if (focusedThread.meta.state === 'composing') {
    return getDraft(focusedThread.meta.id).mode ?? currentComposeMode();
  }
  return focusedThread.meta.channel === 'claude_code' ? 'claude_code' : 'lucidos';
}

export function effectiveCodingAgentBackend(
  focusedThread: ComposeFocus | undefined,
  selectedAgent: CodingAgent,
): CodingAgent | null {
  if (effectiveSendMode(focusedThread) !== 'claude_code') return null;
  if (!focusedThread || focusedThread.meta.state === 'composing') return selectedAgent;
  return focusedThread.meta.codingAgent ?? 'claude-code';
}
