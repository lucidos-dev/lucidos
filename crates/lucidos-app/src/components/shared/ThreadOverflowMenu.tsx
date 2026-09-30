import { OverflowMenu, type HostOpener, type TriggerFace } from './OverflowMenu';
import type { ComponentChildren } from 'preact';
import { CopyIcon, DownloadIcon, ArchiveIcon, CheckIcon, EditIcon, LocateIcon, MoveToTopIcon, PinIcon, SetAsideIcon, SparkleIcon, StandingApplyIcon, TrashIcon } from './icons';
import { canRenameThread, promptRenameThread, suggestThreadName } from '../../store/actions/threadRename';
import { copyThreadRef, copyThreadTitle } from '../../utils/threadRef';
import { exportThread } from '../../utils/exportThread';
import { resolveChangeMenuActions, resolveThreadActions } from '../../store/actions/threadActions';
import type { Action } from '../../generated/thread-lifecycle';
import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { handleDeleteThread } from '../../store/actions/threads-delete';
import { canMoveToTopLevel, handleDetachThread } from '../../store/actions/threads-detach';
import { threadIsDeletable } from '../../generated/thread-lifecycle';
import { threadMap, effectiveThreadStatus, standingApplyThreadIds } from '../../store/store';
import { threadInfoRows } from '../drawer/threadRowInfo';

const CHANGE_ACTION_ICON: Partial<Record<Action, (armed: boolean) => ComponentChildren>> = {
  apply: () => <CheckIcon />,
  apply_when_settled: (armed) => <StandingApplyIcon armed={armed} />,
  set_aside: () => <SetAsideIcon />,
  discard: () => <TrashIcon />,
};

/** Per-thread overflow (⋯) menu for a STARTED thread: Show in thread list and a
 *  Pin/Unpin toggle first, then Copy thread reference / Copy thread title / Download thread, then the
 *  conditional change, Archive and Delete actions, with the Info row
 *  auto-appended by <OverflowMenu>.
 *  Built on the shared <OverflowMenu> shell (trigger + anchored menu/Info
 *  popovers + keyboard roving + the full dismiss/Escape/inert contract).
 *
 *  **The change actions get their own section after Download**: Apply (or
 *  Apply on settle), then Discard. They come from the same selector as the
 *  thread's banner, so the menu offers exactly what the banner does, from the
 *  thread list too.
 *
 *  **Archive and Delete sit last, right above Info.** They are the thread's
 *  exits, so they stay off the top of the menu. A stray tap lands there,
 *  and so does the keyboard-open's focus. Delete sits below Archive, being the
 *  harsher of the two: it removes the thread, its sub-threads and what Lucidos
 *  learned from them, with no undo (ADR 0192). It confirms, and the
 *  confirmation names what this family holds.
 *
 *  **Delete is offered in the Archive section too.** `threadIsDeletable` asks
 *  `is_blocking` of the thread as if it were in the inbox, which is the one way
 *  it differs from Archive. Gating it on inbox would leave archived garbage
 *  undeletable, which is the case the feature exists for.
 *
 *  **Move to top level shows only on a thread with a parent** (ADR 0278). It
 *  sits with the mutating actions. It confirms, because it cannot be undone,
 *  but it is not red: nothing is stopped or lost.
 *
 *  **Show in thread list leads the menu, in the thread titles only**: they
 *  pass `onShowInThreadList`. On a drawer row it would point at the row
 *  just opened.
 *
 *  **Rename… and Suggest name follow, on a sent thread.** This menu is the only
 *  place to rename, because the title is display-only. A draft is titled by
 *  its compose text, so a rename there would change nothing on screen.
 *
 *  **Pin/Unpin shows on every open.** The mobile title row draws no pin, and a
 *  drawer row's pin is mouse-only (`tabindex=-1`), so the menu must carry it.
 *  Beside an inline pin it repeats that button, which costs nothing.
 *
 *  The Info popover shows the thread's structured details (Status / You / Agent /
 *  Type / Exchanges / Started) — the same rows that used to ride the drawer
 *  row's hover tooltip, now reachable everywhere the ⋯ menu lives (drawer row +
 *  both thread-title headers). */
export function ThreadOverflowMenu({ threadId, title, onShowInThreadList, stopPropagation, extraClass, tabIndex, hostOpener, face }: {
  threadId: string;
  title: string;
  onShowInThreadList?: () => void;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
  /** Lets the drawer row open this menu from its own gesture: a desktop
   *  right-click, or a mobile long press. See <OverflowMenu>. */
  hostOpener?: HostOpener;
  /** The thread title, drawn as the menu button. See <OverflowMenu>. */
  face?: TriggerFace;
}) {
  const opening = face ? { face } : { hostOpener };
  return (
    <OverflowMenu
      ariaLabel="More thread actions"
      stopPropagation={stopPropagation}
      extraClass={extraClass}
      tabIndex={tabIndex}
      {...opening}
      // Read live thread meta only while a popover is open (OverflowMenu gates the
      // call). An unhydrated search hit has no live thread → null → no Info.
      infoRows={() => {
        const thread = threadMap.value.get(threadId);
        return thread ? threadInfoRows(thread.meta, effectiveThreadStatus(thread)) : null;
      }}
      items={({ run }) => {
        // These selectors read threadMap/changes signals; OverflowMenu invokes
        // `items` only while open, so a closed menu subscribes to neither.
        const liveThread = threadMap.value.get(threadId);
        const saved = liveThread?.meta.saved ?? false;
        // A draft has nothing to pin until it is sent.
        const pinnable = !!liveThread && liveThread.meta.state !== 'composing';
        const renamable = canRenameThread(liveThread);
        const archiveAction = resolveThreadActions(threadId).find((a) => a.kind === 'archive');
        const changeActions = resolveChangeMenuActions(threadId);
        const standingApplyArmed = standingApplyThreadIds.value.has(threadId);
        const movable = canMoveToTopLevel(threadId);
        // Read from the same projection facts the server gate re-asks over the
        // locked family, so the item is hidden rather than offered and refused.
        const deletable = !!liveThread
          && liveThread.meta.state !== 'composing'
          && threadIsDeletable(
            liveThread.meta.channel === 'claude_code' ? 'claude_code' : 'chat',
            effectiveThreadStatus(liveThread),
            liveThread.meta.codingAgentProposed ?? false,
            liveThread.meta.codingAgentKind === 'external' || liveThread.meta.codingAgentIsExternalRepo,
            liveThread.meta.blockingDescendantCount > 0,
          );
        return (
          <>
            {onShowInThreadList && (
              <>
                <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(onShowInThreadList)}>
                  <LocateIcon />
                  Show in thread list
                </button>
                <div class="thread-overflow-divider" role="separator" />
              </>
            )}
            {pinnable && (
              <>
                <button type="button" class="thread-overflow-item" role="menuitem"
                  onClick={run(() => { if (saved) void handleUnsaveThread(threadId); else void handleSaveThread(threadId); })}>
                  <PinIcon filled={saved} />
                  {saved ? 'Unpin thread' : 'Pin thread'}
                </button>
                <div class="thread-overflow-divider" role="separator" />
              </>
            )}
            {renamable && (
              <>
                <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void promptRenameThread(threadId); })}>
                  <EditIcon />
                  Rename…
                </button>
                <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void suggestThreadName(threadId); })}>
                  <SparkleIcon />
                  Suggest name
                </button>
                <div class="thread-overflow-divider" role="separator" />
              </>
            )}
            <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => copyThreadRef(threadId, title))}>
              <CopyIcon />
              Copy thread reference
            </button>
            <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => copyThreadTitle(title))}>
              <CopyIcon />
              Copy thread title
            </button>
            <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void exportThread(threadId, title); })}>
              <DownloadIcon />
              Download thread
            </button>
            {changeActions.length > 0 && <div class="thread-overflow-divider" role="separator" />}
            {changeActions.map((action) => {
              const toggle = action.kind === 'apply_when_settled';
              return (
                <button key={action.kind} type="button"
                  role={toggle ? 'menuitemcheckbox' : 'menuitem'}
                  aria-checked={toggle ? standingApplyArmed : undefined}
                  class={action.kind === 'discard' ? 'thread-overflow-item thread-overflow-item-danger' : 'thread-overflow-item'}
                  data-tooltip={action.tooltip}
                  onClick={run(() => { void action.invoke(); })}>
                  {CHANGE_ACTION_ICON[action.kind]?.(standingApplyArmed)}
                  {action.label}
                  {toggle && standingApplyArmed && (
                    <span class="thread-overflow-check thread-overflow-check-end" aria-hidden="true">✓</span>
                  )}
                </button>
              );
            })}
            {(movable || archiveAction || deletable) && <div class="thread-overflow-divider" role="separator" />}
            {movable && (
              <button type="button" class="thread-overflow-item" role="menuitem"
                onClick={run(() => { void handleDetachThread(threadId); })}>
                <MoveToTopIcon />
                Move to top level
              </button>
            )}
            {archiveAction && (
              <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void archiveAction.invoke(); })}>
                <ArchiveIcon />
                Archive
              </button>
            )}
            {deletable && (
              <button type="button" class="thread-overflow-item thread-overflow-item-danger" role="menuitem"
                onClick={run(() => { void handleDeleteThread(threadId); })}>
                <TrashIcon />
                Delete thread
              </button>
            )}
          </>
        );
      }}
    />
  );
}
