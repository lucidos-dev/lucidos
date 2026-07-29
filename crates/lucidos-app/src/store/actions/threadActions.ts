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
 *  - `resolveGlobalActions()` — composes the top overlay-dismiss action with
 *    the focused thread's actions.
 */

import { threadMap, focusedThreadId, changes, showConfirm, effectiveThreadStatus, applyingNowThreadIds, discardingCCThreadIds, archivingThreadIds } from '../store';
import { getCodingAgentWaitingInfo } from '../thread-events';
import { availableThreadActions, type Action } from '../../generated/thread-lifecycle';
import { getDraft, draftIsEmpty } from '../composeDrafts';
import { handleArchiveThread, handleSaveThread, handleUnsaveThread } from './threads';
import { endClaudeCodeAndApply, handleDiscardCCChanges } from './chat-claude-code';
import { discardCompose, updateCompose } from './compose';
import { topOverlay, dismissTopOverlay } from '../overlayStack';

export type ActionCategory = 'close' | 'primary' | 'save' | 'dismiss';

/** The kind discriminator: every `Action` from the core, plus the global-only
 *  overlay-dismiss pseudo-action that `resolveGlobalActions` prepends. */
export type ActionKind = Action | 'dismiss_overlay';

export interface TaggedAction {
  kind: ActionKind;
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
const APPLY_INCOMPLETE_CONFIRM =
  'This change was proposed by a turn that ended in failure (e.g. mid-stream API drop). The worktree contents may be incomplete. Apply anyway?';

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
  if (!(await showConfirm(DISCARD_DRAFT_CONFIRM, 'Discard'))) return false;
  const thread = threadMap.value.get(threadId);
  if (thread?.meta.state === 'active') {
    updateCompose(threadId, { text: '', image_hashes: [] });
  } else {
    void discardCompose(threadId);
  }
  return true;
}

/**
 * Per-thread tagged actions in cascade priority order: the close set
 * (DiscardDraft → Discard/Apply → Archive) followed by the Save/Unsave toggle.
 */
export function resolveThreadActions(threadId: string): TaggedAction[] {
  const thread = threadMap.value.get(threadId);
  if (!thread) return [];

  const threadType = thread.meta.channel === 'claude_code' ? 'claude_code' : 'chat';
  const status = effectiveThreadStatus(thread);
  const ccInfo = threadType === 'claude_code' ? getCodingAgentWaitingInfo(thread.meta) : null;

  const changesLoadable = changes.value;
  const pendingChange =
    changesLoadable.status === 'loaded'
      ? (changesLoadable.data.find(
          (c) => c.thread_id === threadId && c.status === 'pending' && c.file_count > 0,
        ) ?? null)
      : null;
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
    hasUnsentDraft,
    isSaved,
  );

  // External-repo CC can't Apply/Discard a change into a foreign repo — its
  // only exit is Archive (the cascade emits ChangeApplied per pending change
  // before ThreadArchived). Mirror `is_blocking`'s carve-out: replace the
  // change layer (discard + apply) with a single Archive. Draft + save toggle
  // are untouched.
  const isExternalRepo = threadType === 'claude_code' && (ccInfo?.isExternalRepo ?? false);
  let kinds: Action[] = raw;
  if (isExternalRepo && (raw.includes('apply') || raw.includes('discard'))) {
    kinds = raw.flatMap((a): Action[] => {
      if (a === 'discard') return [];
      if (a === 'apply') return ['archive'];
      return [a];
    });
  }

  // A composing draft only exists in the frontend (no persisted thread), and
  // `isExcludedFromSections` keeps it out of every drawer section — including
  // Saved. Saving it would set `is_saved` but the row would still render in the
  // compose/Drafts surface, never moving to the Saved section. Suppress the
  // save toggle so the action isn't offered for something it can't accomplish.
  if (thread.meta.state === 'composing') {
    kinds = kinds.filter((a) => a !== 'save' && a !== 'unsave');
  }

  const requiresRestart =
    !isExternalRepo &&
    threadType === 'claude_code' &&
    kinds.includes('apply') &&
    (pendingChange?.requires_restart || ccInfo?.requiresRestart || false);
  const incomplete = pendingChange?.incomplete ?? false;

  return kinds.map((kind) => tagAction(kind, threadId, { requiresRestart, incomplete }));
}

function tagAction(
  kind: Action,
  threadId: string,
  opts: { requiresRestart: boolean; incomplete: boolean },
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
          if (await showConfirm(DISCARD_CHANGE_CONFIRM, 'Discard')) void handleDiscardCCChanges(threadId);
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
          ? 'This change was proposed by a turn that ended in failure. The worktree contents may be partial work. You will be asked to confirm.'
          : opts.requiresRestart
            ? 'Applies now (non-disruptive). A new engine version builds in the background; you’ll be prompted to switch to it when ready.'
            : undefined,
        invoke: async () => {
          if (opts.incomplete && !(await showConfirm(APPLY_INCOMPLETE_CONFIRM, 'Apply'))) return;
          void endClaudeCodeAndApply(threadId);
        },
      };
    case 'archive':
      return {
        kind,
        category: 'close',
        label: 'Archive',
        // handleArchiveThread confirms internally only when the thread is saved.
        invoke: () => void handleArchiveThread(threadId),
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
      // The change layer is a choice. The dialog IS the confirmation, so the
      // branches call the raw handlers directly rather than the TaggedAction
      // invokes (which would add a redundant second confirm).
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
      if (ok && apply) void endClaudeCodeAndApply(focused);
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

/**
 * Global action availability: the top overlay-dismiss action (when any overlay
 * is open) followed by the focused thread's actions. The centralized Escape
 * dispatcher consults the dismiss action; the close cascade consults the
 * focused thread's `close`-category layers.
 */
export function resolveGlobalActions(): TaggedAction[] {
  const out: TaggedAction[] = [];
  if (topOverlay()) {
    out.push({
      kind: 'dismiss_overlay',
      category: 'dismiss',
      label: 'Dismiss',
      invoke: () => {
        dismissTopOverlay();
      },
    });
  }
  const focused = focusedThreadId.value;
  if (focused) out.push(...resolveThreadActions(focused));
  return out;
}
