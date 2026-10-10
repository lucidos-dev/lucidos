import { OverflowMenu, type HostOpener, type TriggerFace } from './OverflowMenu';
import type { ComponentChildren } from 'preact';
import { CopyIcon, DownloadIcon, ArchiveIcon, CheckIcon, EditIcon, CurrentIcon, LocateIcon, MoveToTopIcon, PinIcon, SearchIcon, SetAsideIcon, SparkleIcon, StandingApplyIcon, TrashIcon } from './icons';
import { canRenameThread, canSuggestThreadName, promptRenameThread, suggestThreadName } from '../../store/actions/threadRename';
import { copyThreadRef, copyThreadTitle } from '../../utils/threadRef';
import { exportThread } from '../../utils/exportThread';
import { resolveChangeMenuActions, threadIsPinnable } from '../../store/actions/threadActions';
import type { Action } from '../../generated/thread-lifecycle';
import { focusThread, handleArchiveThread, handleSaveThread, handleUnarchiveThread, handleUnsaveThread } from '../../store/actions/threads';
import { handleDeleteThread } from '../../store/actions/threads-delete';
import { canMoveToTopLevel, handleDetachThread } from '../../store/actions/threads-detach';
import { exitItems } from '../../store/actions/threadBlockers';
import { SUB_THREAD_STATE, moreSubThreads } from '../../store/actions/blockerCopy';
import { threadMap, standingApplyThreadIds } from '../../store/store';
import { threadDisplayTitle } from '../../utils/threadTitle';

/** Blocking sub-threads the menu lists by name before it counts the rest. */
export const LISTED_SUB_THREADS = 3;

const CHANGE_ACTION_ICON: Partial<Record<Action, (armed: boolean) => ComponentChildren>> = {
  apply: () => <CheckIcon />,
  apply_when_settled: (armed) => <StandingApplyIcon armed={armed} />,
  set_aside: () => <SetAsideIcon />,
  discard: () => <TrashIcon />,
};

/** Per-thread overflow (⋯) menu for a STARTED thread: Show in Folders and a
 *  Pin/Unpin toggle first, then Copy thread reference / Copy thread title / Download thread, then the
 *  conditional change, Archive and Delete actions.
 *  Built on the shared <OverflowMenu> shell (trigger + anchored menu popover +
 *  keyboard roving + the full dismiss/Escape/inert contract).
 *
 *  **The change actions get their own section after Download**: Apply (or
 *  Apply on settle), then Discard. They come from the same selector as the
 *  thread's banner, so the menu offers exactly what the banner does, from the
 *  thread list too.
 *
 *  **Archive and Delete sit last.** They are the thread's
 *  exits, so they stay off the top of the menu. A stray tap lands there,
 *  and so does the keyboard-open's focus. Delete sits below Archive, being the
 *  harsher of the two: it removes the thread, its sub-threads and what Lucidos
 *  learned from them, with no undo (ADR 0192). It confirms, and the
 *  confirmation names what this family holds.
 *
 *  **A blocked exit shows dimmed, and is never hidden** (ADR 0378). A note
 *  under the exits gives the reason, from `threadBlocker`, the same action
 *  blocker the engine's refusal names. When sub-threads block, the note
 *  introduces one row per blocking sub-thread, with its title and state. A tap
 *  opens it, so the user can resolve it there.
 *
 *  **Delete is offered in the Archive section too.** The action blocker judges
 *  the thread as if it were in the inbox. Gating Delete on inbox would leave
 *  archived garbage undeletable, which is the case the feature exists for.
 *  An archived thread offers Move to Current where Archive was, and brings its
 *  sub-threads back with it.
 *
 *  **Move to top level shows only on a thread with a parent** (ADR 0278). It
 *  sits with the mutating actions. It confirms, because it cannot be undone,
 *  but it is not red: nothing is stopped or lost.
 *
 *  **Show in Folders leads the menu, in the thread titles only**: they
 *  pass `onShowInFolders`. On a drawer row it would point at the row
 *  just opened. **Find in thread** joins it there, for the same reason
 *  (`onFindInThread`): it searches the open transcript.
 *
 *  **Rename… and Suggest name follow, on a sent thread.** This menu is the only
 *  place to rename, because the title is display-only. A draft is titled by
 *  its compose text, so a rename there would change nothing on screen. The
 *  home thread offers Rename… alone: only the user names it (ADR 0362).
 *
 *  **Pin/Unpin shows on every open.** The mobile title row draws no pin, and a
 *  drawer row's pin is mouse-only (`tabindex=-1`), so the menu must carry it.
 *  Beside an inline pin it repeats that button, which costs nothing. */
export function ThreadOverflowMenu({ threadId, title, onShowInFolders, onFindInThread, stopPropagation, extraClass, tabIndex, hostOpener, face }: {
  threadId: string;
  title: string;
  onShowInFolders?: () => void;
  onFindInThread?: () => void;
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
      items={({ run }) => {
        // These selectors read threadMap/changes signals; OverflowMenu invokes
        // `items` only while open, so a closed menu subscribes to neither.
        const liveThread = threadMap.value.get(threadId);
        const saved = liveThread?.meta.saved ?? false;
        const pinnable = !!liveThread && threadIsPinnable(liveThread.meta);
        const renamable = canRenameThread(liveThread);
        const changeActions = resolveChangeMenuActions(threadId);
        const standingApplyArmed = standingApplyThreadIds.value.has(threadId);
        const movable = canMoveToTopLevel(threadId);
        const exits = exitItems(threadId);
        return (
          <>
            {(onShowInFolders || onFindInThread) && (
              <>
                {onShowInFolders && (
                  <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(onShowInFolders)}>
                    <LocateIcon />
                    Show in Folders
                  </button>
                )}
                {onFindInThread && (
                  <button type="button" class="thread-overflow-item find-in-thread" role="menuitem" onClick={run(onFindInThread)}>
                    <SearchIcon />
                    Find in thread
                  </button>
                )}
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
                {canSuggestThreadName(liveThread) && (
                  <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void suggestThreadName(threadId); })}>
                    <SparkleIcon />
                    Suggest name
                  </button>
                )}
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
            {(movable || exits.archive || exits.unarchive || exits.delete) && <div class="thread-overflow-divider" role="separator" />}
            {movable && (
              <button type="button" class="thread-overflow-item" role="menuitem"
                onClick={run(() => { void handleDetachThread(threadId); })}>
                <MoveToTopIcon />
                Move to top level
              </button>
            )}
            {exits.archive === 'enabled' && (
              <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void handleArchiveThread(threadId); })}>
                <ArchiveIcon />
                Archive
              </button>
            )}
            {exits.archive === 'blocked' && <BlockedExit icon={<ArchiveIcon />} label="Archive" reason={exits.reason} />}
            {exits.unarchive && (
              <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void handleUnarchiveThread(threadId); })}>
                <CurrentIcon />
                Move to Current
              </button>
            )}
            {exits.delete === 'enabled' && (
              <button type="button" class="thread-overflow-item thread-overflow-item-danger" role="menuitem"
                onClick={run(() => { void handleDeleteThread(threadId); })}>
                <TrashIcon />
                Delete thread
              </button>
            )}
            {exits.delete === 'blocked' && <BlockedExit icon={<TrashIcon />} label="Delete thread" danger reason={exits.reason} />}
            {exits.reason && <div class="thread-overflow-note" aria-hidden="true">{exits.reason}</div>}
            {exits.subThreads.slice(0, LISTED_SUB_THREADS).map(({ thread: subThread, blocker }) => (
              <button key={subThread.meta.id} type="button" class="thread-overflow-item" role="menuitem"
                onClick={run(() => { focusThread(subThread.meta.id); })}>
                <LocateIcon />
                <span class="thread-overflow-sub-thread">
                  <span class="thread-overflow-sub-thread-title">{threadDisplayTitle(subThread)}</span>
                  <span class="thread-overflow-sub-thread-state">{SUB_THREAD_STATE[blocker]}</span>
                </span>
              </button>
            ))}
            {exits.subThreads.length > LISTED_SUB_THREADS && (
              <div class="thread-overflow-note">{moreSubThreads(exits.subThreads.length - LISTED_SUB_THREADS)}</div>
            )}
          </>
        );
      }}
    />
  );
}

/** An exit the blocker holds back. `aria-disabled` rather than `disabled`, so it
 *  stays in the roving focus order. The note under the exits draws the reason
 *  once. Each exit also holds it as visually hidden text, so a screen reader
 *  reads it with the item. */
function BlockedExit({ icon, label, reason, danger }: {
  icon: ComponentChildren;
  label: string;
  reason: string | null;
  danger?: boolean;
}) {
  return (
    <button type="button" role="menuitem" aria-disabled="true"
      class={danger ? 'thread-overflow-item thread-overflow-item-blocked thread-overflow-item-danger' : 'thread-overflow-item thread-overflow-item-blocked'}>
      {icon}
      {label}
      {reason && <span class="visually-hidden">{` ${reason}`}</span>}
    </button>
  );
}
