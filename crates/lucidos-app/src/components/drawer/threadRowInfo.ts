import type { ThreadMeta, ComposeChannelMode } from '../../store/thread-events';
import type { Scope } from '../../store/store';
import type { ThreadStatus } from '../../generated/thread-lifecycle';
import { resolveVisualStatus, type VisualStatus } from '../shared/ThreadStatusIcon';
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
export type StatusTone = 'running' | 'changes' | 'waiting' | 'paused' | 'failed' | 'idle';

/** One row of the structured thread tooltip: a dim label and its value, with an
 *  optional status `tone` that paints a leading dot in the value cell. */
export interface TooltipRow {
  label: string;
  value: string;
  tone?: StatusTone;
}

/** Card word + tone for each resolved `VisualStatus`. The card derives its
 *  Status row from the SAME `resolveVisualStatus` that paints the row's dot (and
 *  the dot's own hover tooltip), so the word can never contradict the dot — the
 *  bug this replaced: a thread idle-but-waiting-on-children read "Idle" here
 *  while the dot said "Waiting". The wording is intentionally a touch more
 *  conversational than the dot's terse `STATUS_INFO` labels ("Changes ready" vs
 *  "Changes to review", "Waiting for you" vs "Waiting for your answer"). Both
 *  `waiting` (active children) and `question` (paused on a question) paint the
 *  same `waiting` tone — the card has no distinct question dot. */
const CARD_STATUS: Record<VisualStatus, { word: string; tone: StatusTone }> = {
  running: { word: 'Running', tone: 'running' },
  waiting: { word: 'Waiting', tone: 'waiting' },
  question: { word: 'Waiting for you', tone: 'waiting' },
  changes: { word: 'Changes ready', tone: 'changes' },
  paused: { word: 'Paused by a restart', tone: 'paused' },
  failed: { word: 'Failed', tone: 'failed' },
  idle: { word: 'Idle', tone: 'idle' },
  // `resolveVisualStatus` collapses the raw `waiting_for_user_answer` ThreadStatus
  // into `question`, so this key is unreachable in practice — present only because
  // `VisualStatus` is a superset of `ThreadStatus`. Kept in sync with `question`.
  waiting_for_user_answer: { word: 'Waiting for you', tone: 'waiting' },
};

/** Structured rows describing a started thread, rendered as a two-column
 *  label/value grid by the thread overflow menu's Info popover. `status` is the
 *  effective status the caller already derived for the row's dot, so the Status
 *  word and the dot can't disagree. (Compose drafts use `draftRowTooltip`
 *  instead — the meaningful subset for an unsent draft, rendered by the same
 *  Info popover behind DraftOverflowMenu.) */
export function threadInfoRows(meta: ThreadMeta, status: ThreadStatus): TooltipRow[] {
  // Fall back to createdAt/updatedAt if the attributed-recency fields are absent
  // (test fixtures); production always has them.
  const userAt = meta.lastUserAction || meta.createdAt;
  const agentAt = meta.lastAgentAction || meta.updatedAt || meta.createdAt;
  // Resolve the visual status from the exact same three inputs the dot uses
  // (see ThreadDrawer's `resolveVisualStatus(status, …)`), so the Status word
  // tracks the dot — including the active-children "Waiting" case.
  const visual = resolveVisualStatus(status, meta.activeChildrenCount > 0, meta.codingAgentProposed);
  const { word, tone } = CARD_STATUS[visual];
  return [
    { label: 'Status', value: word, tone },
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

/** Structured Info rows for a compose draft row, the draft counterpart of
 *  `threadInfoRows` (both rendered by the ⋯ menu's Info popover). A draft has no
 *  agent activity or exchanges yet, so it's the meaningful subset: its Draft
 *  status, where it's headed, and when it was created. A draft is "Created" but
 *  never "Started" — being started is the first send, which is exactly the
 *  moment it stops being a draft (so `threadInfoRows` says "Started" and this
 *  says "Created"). */
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
