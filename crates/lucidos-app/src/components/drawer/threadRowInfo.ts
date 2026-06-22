import type { ThreadMeta, ComposeChannelMode } from '../../store/thread-events';
import type { Scope } from '../../store/store';
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

/** The context ROW for the tooltip — a `{ label, value }` pair so the type word
 *  (Repository / App / Trigger / Type) sits in the dim left column and its name
 *  in the bright right column, matching every other tooltip row. The row chips
 *  are name-only; this keeps the tooltip self-describing. */
function threadContextRow(f: ThreadContextFields): TooltipRow {
  const name = threadContextName(f);
  if (f.channel === 'trigger') return name ? { label: 'Trigger', value: name } : { label: 'Type', value: 'Trigger' };
  if (f.channel === 'claude_code') {
    if (f.codingAgentKind === 'app') return name ? { label: 'App', value: name } : { label: 'Type', value: 'App' };
    return name ? { label: 'Repository', value: name } : { label: 'Type', value: 'Repository' };
  }
  if (f.channel === 'chat') return { label: 'Type', value: 'Chat' };
  return { label: 'Type', value: formatChannel(f.channel) };
}

/** A tone keyword that maps to a small colored status dot in the tooltip's value
 *  cell — the only place a row carries one. Mirrors the row's status dot so the
 *  word and the color can't disagree. */
export type StatusTone = 'running' | 'changes' | 'waiting' | 'failed' | 'idle';

/** One row of the structured thread tooltip: a dim label and its value, with an
 *  optional status `tone` that paints a leading dot in the value cell. */
export interface TooltipRow {
  label: string;
  value: string;
  tone?: StatusTone;
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

/** Status dot tone, kept in lockstep with `statusWord` above. */
function statusTone(status: ThreadStatus, codingAgentProposed: boolean): StatusTone {
  if (status === 'running') return 'running';
  if (codingAgentProposed) return 'changes';
  switch (status) {
    case 'waiting_for_user_answer': return 'waiting';
    case 'failed': return 'failed';
    default: return 'idle';
  }
}

/** Structured rows for a thread row's tooltip, rendered as a two-column
 *  label/value grid by the global tooltip system (`data-tooltip-rows`).
 *  `status` is the effective status the caller already derived for the row's
 *  dot, so the tooltip and the dot can't disagree. */
export function threadRowTooltip(meta: ThreadMeta, status: ThreadStatus): TooltipRow[] {
  // Fall back to createdAt/updatedAt if the attributed-recency fields are absent
  // (test fixtures); production always has them.
  const userAt = meta.lastUserAction || meta.createdAt;
  const agentAt = meta.lastAgentAction || meta.updatedAt || meta.createdAt;
  return [
    { label: 'Status', value: statusWord(status, meta.codingAgentProposed), tone: statusTone(status, meta.codingAgentProposed) },
    { label: 'You', value: formatTimeAgo(new Date(userAt)) },
    { label: 'Agent', value: formatTimeAgo(new Date(agentAt)) },
    threadContextRow(meta),
    { label: 'Exchanges', value: String(meta.messageCount) },
    { label: 'Started', value: formatTimeAgo(new Date(meta.createdAt)) },
  ];
}

/** Context row for a compose draft. A draft hasn't bound its meta to a backend
 *  yet (see `sendCompose`), so its destination is read live from the compose
 *  `mode` + `scope` rather than the thread meta — mirroring `threadContextRow`
 *  and the chip the draft row paints. `contextName` is the already-resolved
 *  name (see `composeDraftContextName`). */
function draftContextRow(mode: ComposeChannelMode, scope: Scope, contextName: string | undefined): TooltipRow {
  if (mode !== 'claude_code') return { label: 'Type', value: 'Chat' };
  if (scope.kind === 'app') return contextName ? { label: 'App', value: contextName } : { label: 'Type', value: 'App' };
  return contextName ? { label: 'Repository', value: contextName } : { label: 'Type', value: 'Repository' };
}

/** Structured tooltip rows for a compose draft row, the draft counterpart of
 *  `threadRowTooltip`. A draft has no agent activity or exchanges yet, so the
 *  tooltip is the meaningful subset: its Draft status, where it's headed, and
 *  when it was created. A draft is "Created" but never "Started" — being
 *  started is the first send, which is exactly the moment it stops being a
 *  draft (so `threadRowTooltip` says "Started" and this says "Created"). */
export function draftRowTooltip(
  mode: ComposeChannelMode,
  scope: Scope,
  contextName: string | undefined,
  createdAt: string,
): TooltipRow[] {
  return [
    { label: 'Status', value: 'Draft', tone: 'idle' },
    draftContextRow(mode, scope, contextName),
    { label: 'Created', value: formatTimeAgo(new Date(createdAt)) },
  ];
}
