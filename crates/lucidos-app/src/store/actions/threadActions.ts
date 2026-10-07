/** Frontend action-availability tiers (see docs/glossary.md).
 *
 *  `availableThreadActions` (codegen'd from thread_lifecycle.rs) is the
 *  DB-derivable availability core — it returns bare `Action` kinds. This module
 *  ENRICHES that output into `TaggedAction`s: each carries a UI `category`, a
 *  `label`, and an `invoke()` that encapsulates the confirm + handler. Buttons
 *  and the close cascade both render/drive from these, so a shortcut can only
 *  invoke an action whose button is currently available (no enablement drift).
 *
 *  - `resolveThreadActions(threadId)` — per-thread tagged actions. Feeds the
 *    core's `has_unsent_draft` from the live `composeDrafts` signal (fresh,
 *    ahead of the 250 ms compose-debounce), and applies the external-repo
 *    carve-out (Apply can't merge into a foreign repo → Archive instead).
 */

import { threadMap, focusedThreadId, changes, showConfirm, effectiveThreadStatus, applyingNowThreadIds, applyingChangeThreadIds, discardingCCThreadIds, archivingThreadIds, standingApplyThreadIds, armingStandingApplyThreadIds } from '../store';
import { getCodingAgentWaitingInfo, type ThreadMeta } from '../thread-events';
import { availableThreadActions, type Action } from '../../generated/thread-lifecycle';
import { getDraft, draftIsEmpty } from '../composeDrafts';
import { handleArchiveThread, handleSaveThread, handleUnsaveThread } from './threads';
import { endClaudeCodeAndApply, handleDiscardCCChanges } from './chat-claude-code';
import { armStandingApply, disarmStandingApply, setAsideSingleChange, APPLY_NEW_VERSION_TOOLTIP } from './chat-changes';
import { discardCompose, updateCompose } from './compose';

export type ActionCategory = 'close' | 'primary' | 'save';

export interface TaggedAction {
  kind: Action;
  category: ActionCategory;
  label: string;
  /** Optional hover tooltip (e.g. the Apply restart / partial-work hint). */
  tooltip?: string;
  /** Run the action. May open a confirm dialog first; returns its promise so
   *  callers can await completion (the cascade gates re-evaluation on it). */
  invoke: () => void | Promise<void>;
}

/** Confirm copy for the discard-draft action. Discarding an unsent draft is
 *  destructive (typed text is lost), so it confirms. */
const DISCARD_DRAFT_CONFIRM = 'Discard this unsent draft?';
const DISCARD_CHANGE_CONFIRM = 'Discard all changes from this session? This cannot be undone.';
export const APPLY_INCOMPLETE_CONFIRM =
  'This change comes from a turn that did not finish: it was stopped, or it was cut short. The work may be partial. Apply anyway?';
const SET_ASIDE_TOOLTIP =
  'Keep this change for later. It leaves Review and Apply All, and the thread can be archived. Bring it back from the Changes panel.';

/** Discard the focused thread's unsent draft, confirming first. The confirm
 *  lives HERE, on the action — not in any one entry point — so the "Discard
 *  draft" button and the close-cascade shortcut can never diverge on whether
 *  they ask (confirmation is tied to the action, not to how it was invoked).
 *  Returns true when the user confirmed and the draft was dropped.
 *
 *  Active thread → clear the in-progress follow-up text + images but keep the
 *  user in the thread (deleting it would 409 "thread is active — use archive
 *  instead"; compose fields are persisted server-side for active threads too
 *  for cross-device draft sync, so emptying them flows through updateCompose).
 *  Composing throwaway → drop it wholesale via discardCompose. */
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

/** The thread's pending change from the changes list, once the list names it. */
function findPendingChange(threadId: string) {
  const changesLoadable = changes.value;
  if (changesLoadable.status !== 'loaded') return null;
  return changesLoadable.data.find(
    (c) => c.thread_id === threadId && c.status === 'pending' && c.file_count > 0,
  ) ?? null;
}

/** Whether the thread's pending change is incomplete: its turn did not finish
 *  (ADR 0346). Like the restart flag, it unions the change row with the
 *  thread's projection flag, so it holds while either update is in flight.
 *  Its Apply confirms, and the banner leads with Continue. */
export function threadHasIncompleteChange(threadId: string): boolean {
  return (findPendingChange(threadId)?.incomplete ?? false)
    || (threadMap.value.get(threadId)?.meta.codingAgentIncomplete ?? false);
}

/**
 * Per-thread tagged actions in cascade priority order: the close set
 * (DiscardDraft → Discard/Apply → Archive) followed by the Save/Unsave toggle.
 */
/** Whether a pin can do anything for this thread. A draft has nothing to pin
 *  until it is sent. The home thread sits in no drawer section, so a pin
 *  would move it nowhere, and the engine refuses one (ADR 0362). */
export function threadIsPinnable(meta: Pick<ThreadMeta, 'state' | 'home'>): boolean {
  return meta.state !== 'composing' && !meta.home;
}

export function resolveThreadActions(threadId: string): TaggedAction[] {
  const thread = threadMap.value.get(threadId);
  if (!thread) return [];

  const threadType = thread.meta.channel === 'claude_code' ? 'claude_code' : 'chat';
  const status = effectiveThreadStatus(thread);
  const ccInfo = threadType === 'claude_code' ? getCodingAgentWaitingInfo(thread.meta) : null;

  const pendingChange = findPendingChange(threadId);
  const hasPendingChanges = !!pendingChange || (ccInfo?.proposed ?? false);
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

  // External-repo CC can't Apply/Discard a change into a foreign repo — its
  // only exit is Archive (the cascade emits ChangeApplied per pending change
  // before ThreadArchived). Mirror `is_blocking`'s carve-out: replace the
  // change layer (discard + apply) with a single Archive. Draft + save toggle
  // are untouched.
  //
  // Read off `meta`, never off `ccInfo`. That helper answers null while the
  // thread runs AND whenever nothing is proposed, and an external-repo thread
  // never proposes at all (`idle_change_write` refuses). So it is
  // null in every state such a thread can reach, and this whole carve-out sat
  // dead. `codingAgentKind` is the fact that survives: written at
  // `SessionStarted` and locked, so it is there from the thread's first moment,
  // which is the only time the standing apply is offered. The legacy bool is
  // read with it, because an old row carries it with no kind.
  const isExternalRepo =
    threadType === 'claude_code' &&
    (thread.meta.codingAgentKind === 'external' || thread.meta.codingAgentIsExternalRepo);
  let kinds: Action[] = raw;
  if (isExternalRepo && (raw.includes('apply') || raw.includes('discard'))) {
    kinds = raw.flatMap((a): Action[] => {
      if (a === 'discard' || a === 'set_aside') return [];
      if (a === 'apply') return ['archive'];
      return [a];
    });
  }
  // Set aside acts on one change by id. Until the pending list names it (the
  // proposal flag can land first), there is nothing to act on yet.
  if (!pendingChange) kinds = kinds.filter((a) => a !== 'set_aside');
  // The standing apply is an Apply the owner presses early, so it goes wherever
  // Apply goes. An external repo has no Apply to arm, and a change resolving
  // merge conflicts is already mid-apply.
  if (isExternalRepo || pendingChange?.resolving_conflict) {
    kinds = kinds.filter((a) => a !== 'apply_when_settled');
  }

  // No save toggle where a pin can accomplish nothing.
  if (!threadIsPinnable(thread.meta)) {
    kinds = kinds.filter((a) => a !== 'save' && a !== 'unsave');
  }
  // The home thread never ends, so it has no Archive action (ADR 0362). The
  // engine refuses one anyway. The thread menu shows it blocked, with the
  // reason (ADR 0378), from `exitItems`, not from this list.
  if (thread.meta.home) {
    kinds = kinds.filter((a) => a !== 'archive');
  }

  const requiresRestart =
    !isExternalRepo &&
    threadType === 'claude_code' &&
    kinds.includes('apply') &&
    (pendingChange?.requires_restart || ccInfo?.requiresRestart || false);
  const incomplete = threadHasIncompleteChange(threadId);

  // The standing apply is armed against the thread's CURRENT change where it
  // has one, so it cannot reach a change proposed after the owner pressed.
  const armed = standingApplyThreadIds.value.has(threadId);

  return kinds.map((kind) =>
    tagAction(kind, threadId, {
      requiresRestart,
      incomplete,
      armed,
      pendingChangeId: pendingChange?.id,
    }),
  );
}

/** Menu order for the change layer: the positive actions, then Set aside,
 *  then Discard. */
const CHANGE_MENU_ORDER: readonly Action[] = ['apply', 'apply_when_settled', 'set_aside', 'discard'];

/**
 * The thread's change actions (Apply, Apply on settle, Set aside, Discard)
 * for the thread ⋯ menu, positive first. The menu hides an action it cannot run
 * rather than disabling it (ADR 0168), and the thread's banner shows the
 * progress. So it drops every action while an apply or a discard is in
 * flight, and the standing apply while its own request is.
 */
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
  kind: Action,
  threadId: string,
  opts: {
    requiresRestart: boolean;
    incomplete: boolean;
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
        // discardDraft returns a boolean (false = user canceled the confirm),
        // but the cascade only needs to await it, so the wrapper drops the
        // result to keep invoke's `Promise<void>` contract.
        invoke: async () => { await discardDraft(threadId); },
      };
    case 'discard':
      return {
        kind,
        category: 'close',
        label: 'Discard',
        invoke: async () => {
          if (await showConfirm(DISCARD_CHANGE_CONFIRM, 'Discard', { variant: 'danger' })) void handleDiscardCCChanges(threadId);
        },
      };
    case 'apply':
      return {
        kind,
        category: 'primary',
        // Apply is always non-disruptive — it merges the change. For an
        // engine-affecting change it also kicks off a background rebuild that
        // later surfaces as "New version available → Switch to new version"; the
        // restart is that separate switch, never Apply itself. A restart-requiring
        // change gets a compact "Apply*" marker (the tooltip explains the
        // background-build/switch semantics); the former "Apply & Restart" dual
        // label — which implied the restart happens on click — stays retired.
        label: opts.requiresRestart ? 'Apply*' : 'Apply',
        // Tooltip prefers the partial-work warning (more critical) over the
        // new-version hint when both apply.
        tooltip: opts.incomplete
          ? 'This change comes from a turn that did not finish. The work may be partial, so you will be asked to confirm.'
          : opts.requiresRestart
            ? APPLY_NEW_VERSION_TOOLTIP
            : undefined,
        invoke: async () => {
          if (opts.incomplete && !(await showConfirm(APPLY_INCOMPLETE_CONFIRM, 'Apply', { variant: 'default' }))) return;
          void endClaudeCodeAndApply(threadId);
        },
      };
    case 'set_aside':
      return {
        kind,
        category: 'close',
        label: 'Set aside',
        tooltip: SET_ASIDE_TOOLTIP,
        invoke: () => {
          if (opts.pendingChangeId) return setAsideSingleChange(opts.pendingChangeId);
        },
      };
    case 'apply_when_settled':
      return {
        kind,
        category: 'primary',
        // A checked state that toggles off on click. Each surface draws the check
        // itself: the thread menu puts it at the row's end. Never a disabled
        // Apply: ADR 0168 replaces a control that cannot act with the one that can.
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
        // handleArchiveThread confirms internally only when the thread is saved.
        // Its promise is returned, not voided: the close cascade and the delete
        // dialog's Archive both await this to know the archive finished.
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
        // handleUnsaveThread confirms internally.
        invoke: () => void handleUnsaveThread(threadId),
      };
  }
}

export type CloseLayer = 'draft' | 'change' | 'archive';

/** The front-most close LAYER present in a tagged-action list, or null when
 *  there's nothing to close. Layers, in cascade order: draft (discard the
 *  unsent compose draft) → change (resolve the pending change) → archive. The
 *  change layer groups Discard + Apply because closing it is a CHOICE, not a
 *  single action. Pure — exported for unit testing. */
export function nextCloseLayer(actions: TaggedAction[]): CloseLayer | null {
  if (actions.some((a) => a.kind === 'discard_draft')) return 'draft';
  if (actions.some((a) => a.kind === 'apply' || a.kind === 'discard')) return 'change';
  if (actions.some((a) => a.kind === 'archive')) return 'archive';
  return null;
}

/**
 * Progressive close: resolve EXACTLY ONE close layer for the focused thread.
 * Each invocation re-runs the selector, so resolving one layer (which mutates
 * the underlying fact) surfaces the next layer on the next invocation — no
 * cursor, no "cascade in progress" state. Drives the dedicated close shortcut;
 * the per-thread buttons resolve their own layer on click via the same
 * TaggedActions, so the two can never diverge.
 *
 * In-flight resolutions (apply / discard / archive round-trips) are gated to a
 * no-op so a rapid second invocation can't skip past a layer that's still
 * settling — the async bridge that keeps stateless re-eval honest.
 */
export async function runCloseCascade(): Promise<void> {
  const focused = focusedThreadId.value;
  if (!focused) return;
  if (
    applyingNowThreadIds.value.has(focused) ||
    discardingCCThreadIds.value.has(focused) ||
    archivingThreadIds.value.has(focused)
  ) {
    return; // a layer is still resolving — don't fall through to the next one
  }

  const actions = resolveThreadActions(focused);
  switch (nextCloseLayer(actions)) {
    case 'draft': {
      // The draft layer confirms (per-layer reversibility); the confirm lives
      // on the action (discardDraft), which the TaggedAction invoke calls.
      const draft = actions.find((a) => a.kind === 'discard_draft');
      await draft?.invoke();
      return;
    }
    case 'change': {
      // The change layer is a choice, and the dialog is its confirmation.
      // Discard calls its raw handler, whose own confirm would repeat this
      // one. Apply calls its action, whose only extra confirm is for
      // incomplete work.
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
      // The action's own invoke, so incomplete work still asks first.
      if (ok && apply) await apply.invoke();
      return;
    }
    case 'archive': {
      // handleArchiveThread confirms only when the thread is saved.
      const archive = actions.find((a) => a.kind === 'archive');
      await archive?.invoke();
      return;
    }
    default:
      return; // nothing closeable — no-op
  }
}
