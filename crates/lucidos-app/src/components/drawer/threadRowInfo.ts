import type { ThreadMeta } from '../../store/thread-events';
import type { ThreadStatus } from '../../generated/thread-lifecycle';
import { appIdFromFolder } from '../../utils/appIdFromFolder';
import { formatChannel } from '../../utils/formatChannel';
import { formatTimeAgo } from '../../utils/formatTime';

/** Minimal shape needed to name a thread's context. `ThreadMeta` is structurally
 *  assignable; search-result rows (snake-case) map their fields onto it. */
export interface ThreadContextFields {
  channel: string;
  triggerName?: string | null;
  repoName?: string | null;
  codingAgentKind?: 'lucidos' | 'app' | 'external' | null;
  codingAgentFolder?: string | null;
}

/** The specific context NAME shown as a chip in the thread row, alongside the
 *  channel/type tag: the repo name (coding-agent), the app id (app thread), or
 *  the trigger name. Undefined for plain chat — the "Chat" type tag already says
 *  everything, and there's no name to add. */
export function threadContextName(f: ThreadContextFields): string | undefined {
  if (f.channel === 'trigger') return f.triggerName || undefined;
  if (f.channel === 'claude_code') {
    return f.codingAgentKind === 'app'
      ? appIdFromFolder(f.codingAgentFolder) ?? undefined
      : f.repoName || undefined;
  }
  return undefined;
}

/** The context LINE for the tooltip — the type word plus the name, so the
 *  tooltip is self-describing even though the row chips are name-only. */
function threadContextLine(f: ThreadContextFields): string {
  const name = threadContextName(f);
  if (f.channel === 'trigger') return name ? `Trigger · ${name}` : 'Trigger';
  if (f.channel === 'claude_code') {
    if (f.codingAgentKind === 'app') return name ? `App · ${name}` : 'App';
    return name ? `Repository · ${name}` : 'Repository';
  }
  if (f.channel === 'chat') return 'Chat';
  return formatChannel(f.channel);
}

/** Human-readable status word for the tooltip. A pending change reads as
 *  "Changes ready" (more useful than the underlying 'idle' status), but only
 *  when the loop isn't mid-stream. */
function statusWord(status: ThreadStatus, codingAgentProposed: boolean): string {
  if (status === 'running') return 'Running';
  if (codingAgentProposed) return 'Changes ready';
  switch (status) {
    case 'waiting_for_user_answer': return 'Waiting for you';
    case 'failed': return 'Failed';
    case 'idle': return 'Idle';
    default: return status;
  }
}

/** Multi-line tooltip text for a thread row, rendered via the global
 *  `data-tooltip` system (which honors `\n` through `white-space: pre-line`).
 *  `status` is the effective status the caller already derived for the row's
 *  dot, so the tooltip and the dot can't disagree. */
export function threadRowTooltip(meta: ThreadMeta, status: ThreadStatus): string {
  const exchanges = `${meta.messageCount} exchange${meta.messageCount === 1 ? '' : 's'}`;
  // Fall back to createdAt/updatedAt if the attributed-recency fields are absent
  // (test fixtures); production always has them.
  const userAt = meta.lastUserAction || meta.createdAt;
  const agentAt = meta.lastAgentAction || meta.updatedAt || meta.createdAt;
  return [
    `You · ${formatTimeAgo(new Date(userAt))}`,
    `Agent · ${formatTimeAgo(new Date(agentAt))}`,
    threadContextLine(meta),
    `Status · ${statusWord(status, meta.codingAgentProposed)}`,
    exchanges,
    `Started · ${formatTimeAgo(new Date(meta.createdAt))}`,
  ].join('\n');
}
