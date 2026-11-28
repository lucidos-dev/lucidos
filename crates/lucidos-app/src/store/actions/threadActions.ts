
import { threadMap, focusedThreadId, pendingChangeForThread, showConfirm, effectiveThreadStatus, applyingNowThreadIds, applyingChangeThreadIds, discardingCCThreadIds, standingApplyThreadIds, armingStandingApplyThreadIds } from '../store';
import { archiveRequestInFlight } from '../rowIntents';
import { getCodingAgentWaitingInfo, threadChangeWork, type ThreadMeta } from '../thread-events';
import { availableThreadActions, type ThreadAction } from '../../generated/thread-lifecycle';
import { getDraft, draftIsEmpty } from '../composeDrafts';
import { handleArchiveThread, handleSaveThread, handleUnsaveThread } from './threads';
import { endClaudeCodeAndApply, handleDiscardCCChanges } from './codingAgentChanges';
import { armStandingApply, disarmStandingApply, APPLY_NEW_VERSION_TOOLTIP } from './chat-changes';
import { HOLD_TOOLTIP, promptAndHoldThread } from './threadHold';
import { discardCompose, updateCompose } from './compose';

export type ActionCategory = 'close' | 'primary' | 'save';

export interface TaggedAction {
  kind: ThreadAction;
  category: ActionCategory;
  label: string;
  tooltip?: string;
  invoke: () => void | Promise<void>;
}

const DISCARD_DRAFT_CONFIRM = 'Discard this unsent draft?';
const DISCARD_CHANGE_CONFIRM = 'Discard all changes from this session? This cannot be undone.';
const DISCARD_DRAFT_TOOLTIP = 'Throw away this unsent draft.';
const DISCARD_TOOLTIP = "Throw away this thread's change. This cannot be undone.";
const APPLY_TOOLTIP = "Merge this thread's change and land it.";
const ARCHIVE_TOOLTIP = 'Move this thread out of the inbox into the archive.';

export async function discardDraft(threadId: string): Promise<boolean> {
  if (!(await showConfirm(DISCARD_DRAFT_CONFIRM, 'Discard', { variant: 'danger' }))) return false;
  const thread = threadMap.value.get(threadId);
  if (thread?.meta.state === 'active') {
    updateCompose(threadId, { text: '', image_hashes: [] });
  } else {
    void discardCompose(threadId);
  }
  return true;
}

function findPendingChange(threadId: string) {
  const change = pendingChangeForThread(threadId);
  return change && change.file_count > 0 ? change : null;
}

export function threadIsPinnable(meta: Pick<ThreadMeta, 'state' | 'home'>): boolean {
  return meta.state !== 'composing' && !meta.home;
}

export function resolveThreadActions(threadId: string): TaggedAction[] {
  const thread = threadMap.value.get(threadId);
  if (!thread) return [];

  const threadType = thread.meta.channel === 'claude_code' ? 'claude_code' : 'chat';
  const status = effectiveThreadStatus(thread);
  const ccInfo = threadType === 'claude_code' ? getCodingAgentWaitingInfo(thread.meta) : null;

  const isExternalRepo =
    threadType === 'claude_code' &&
    (thread.meta.codingAgentKind === 'external' || thread.meta.codingAgentIsExternalRepo);
  const pendingChange = findPendingChange(threadId);
  const heldWork = !isExternalRepo && threadChangeWork(thread.meta) === 'held';
  const hasPendingChanges = !!pendingChange || (ccInfo?.proposed ?? false) || heldWork;
  const descendantsBlockArchive = thread.meta.blockingDescendantCount > 0;
  const hasUnsentDraft = !draftIsEmpty(getDraft(threadId));
  const isSaved = thread.meta.saved;

  const raw = availableThreadActions(
    threadType,
    status,
    thread.meta.section,
    hasPendingChanges,
    descendantsBlockArchive,
    thread.meta.liveEventWaitCount > 0,
    hasUnsentDraft,
    isSaved,
  );

  let kinds: ThreadAction[] = raw;
  if (isExternalRepo && (raw.includes('apply') || raw.includes('discard'))) {
    kinds = raw.flatMap((a): ThreadAction[] => {
      if (a === 'discard' || a === 'hold') return [];
      if (a === 'apply') return ['archive'];
      return [a];
    });
  }
  if (isExternalRepo || pendingChange?.resolving_conflict) {
    kinds = kinds.filter((a) => a !== 'apply_when_settled');
  }

  if (!threadIsPinnable(thread.meta)) {
    kinds = kinds.filter((a) => a !== 'save' && a !== 'unsave');
  }
  if (thread.meta.home) {
    kinds = kinds.filter((a) => a !== 'archive');
  }

  const requiresRestart =
    !isExternalRepo &&
    threadType === 'claude_code' &&
    kinds.includes('apply') &&
    (pendingChange?.requires_restart || ccInfo?.requiresRestart || false);

  const armed = standingApplyThreadIds.value.has(threadId);

  return kinds.map((kind) =>
    tagAction(kind, threadId, {
      requiresRestart,
      armed,
      pendingChangeId: pendingChange?.id,
    }),
  );
}

const CHANGE_MENU_ORDER: readonly ThreadAction[] = ['apply', 'apply_when_settled', 'hold', 'discard'];

export function resolveChangeMenuActions(threadId: string): TaggedAction[] {
  if (
    applyingNowThreadIds.value.has(threadId) ||
    applyingChangeThreadIds.value.has(threadId) ||
    discardingCCThreadIds.value.has(threadId)
  ) {
    return [];
  }
  const arming = armingStandingApplyThreadIds.value.has(threadId);
  const actions = resolveThreadActions(threadId)
    .filter((a) => !(arming && a.kind === 'apply_when_settled'));
  return CHANGE_MENU_ORDER.flatMap((kind) => actions.filter((a) => a.kind === kind));
}

function tagAction(
  kind: ThreadAction,
  threadId: string,
  opts: {
    requiresRestart: boolean;
    armed: boolean;
    pendingChangeId?: string;
  },
): TaggedAction {
  switch (kind) {
    case 'discard_draft':
      return {
        kind,
        category: 'close',
        label: 'Discard draft',
        tooltip: DISCARD_DRAFT_TOOLTIP,
        invoke: async () => { await discardDraft(threadId); },
      };
    case 'discard':
      return {
        kind,
        category: 'close',
        label: 'Discard',
        tooltip: DISCARD_TOOLTIP,
        invoke: async () => {
          if (await showConfirm(DISCARD_CHANGE_CONFIRM, 'Discard', { variant: 'danger' })) void handleDiscardCCChanges(threadId);
        },
      };
    case 'apply':
      return {
        kind,
        category: 'primary',
        label: opts.requiresRestart ? 'Apply*' : 'Apply',
        tooltip: opts.requiresRestart ? APPLY_NEW_VERSION_TOOLTIP : APPLY_TOOLTIP,
        invoke: () => void endClaudeCodeAndApply(threadId),
      };
    case 'hold':
      return {
        kind,
        category: 'close',
        label: 'Hold…',
        tooltip: HOLD_TOOLTIP,
        invoke: () => promptAndHoldThread(threadId),
      };
    case 'apply_when_settled':
      return {
        kind,
        category: 'primary',
        label: opts.armed ? 'Applying on settle' : 'Apply on settle',
        tooltip: opts.armed
          ? 'Armed. The change applies when this thread finishes, and drops with a report if the thread stops on a question or fails. Click to cancel.'
          : 'The thread has not finished. Apply its change the moment it does.',
        invoke: () =>
          opts.armed
            ? void disarmStandingApply(threadId)
            : void armStandingApply(threadId, opts.pendingChangeId),
      };
    case 'archive':
      return {
        kind,
        category: 'close',
        label: 'Archive',
        tooltip: ARCHIVE_TOOLTIP,
        invoke: () => handleArchiveThread(threadId),
      };
    case 'save':
      return {
        kind,
        category: 'save',
        label: 'Pin',
        invoke: () => void handleSaveThread(threadId),
      };
    case 'unsave':
      return {
        kind,
        category: 'save',
        label: '✓ Pinned',
        invoke: () => void handleUnsaveThread(threadId),
      };
  }
}

export type CloseLayer = 'draft' | 'change' | 'archive';

export function nextCloseLayer(actions: TaggedAction[]): CloseLayer | null {
  if (actions.some((a) => a.kind === 'discard_draft')) return 'draft';
  if (actions.some((a) => a.kind === 'apply' || a.kind === 'discard')) return 'change';
  if (actions.some((a) => a.kind === 'archive')) return 'archive';
  return null;
}

export async function runCloseCascade(): Promise<void> {
  const focused = focusedThreadId.value;
  if (!focused) return;
  if (
    applyingNowThreadIds.value.has(focused) ||
    discardingCCThreadIds.value.has(focused) ||
    archiveRequestInFlight(focused)
  ) {
    return;
  }

  const actions = resolveThreadActions(focused);
  switch (nextCloseLayer(actions)) {
    case 'draft': {
      const draft = actions.find((a) => a.kind === 'discard_draft');
      await draft?.invoke();
      return;
    }
    case 'change': {
      const apply = actions.find((a) => a.kind === 'apply');
      const ok = await showConfirm(
        'This thread has a pending change. Apply it now, or discard it?',
        apply?.label ?? 'Apply',
        {
          variant: 'default',
          cancelLabel: 'Cancel',
          extraAction: actions.some((a) => a.kind === 'discard')
            ? { label: 'Discard', onClick: () => void handleDiscardCCChanges(focused) }
            : undefined,
        },
      );
      if (ok && apply) await apply.invoke();
      return;
    }
    case 'archive': {
      const archive = actions.find((a) => a.kind === 'archive');
      await archive?.invoke();
      return;
    }
    default:
      return;
  }
}
