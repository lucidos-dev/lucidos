import { OverflowMenu, type HostOpener } from './OverflowMenu';
import { CopyIcon, DownloadIcon, ArchiveIcon, MoveToTopIcon, PinIcon, TrashIcon } from './icons';
import { copyThreadRef, copyThreadTitle } from '../../utils/threadRef';
import { exportThread } from '../../utils/exportThread';
import { resolveThreadActions } from '../../store/actions/threadActions';
import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { handleDeleteThread } from '../../store/actions/threads-delete';
import { canMoveToTopLevel, handleDetachThread } from '../../store/actions/threads-detach';
import { threadIsDeletable } from '../../generated/thread-lifecycle';
import { threadMap, effectiveThreadStatus } from '../../store/store';
import { threadInfoRows } from '../drawer/threadRowInfo';

/** Per-thread overflow (⋯) menu for a STARTED thread: a Pin/Unpin toggle first,
 *  then Copy thread reference / Copy thread title / Download thread, then the
 *  conditional Archive action, with the Info row auto-appended by <OverflowMenu>.
 *  Built on the shared <OverflowMenu> shell (trigger + anchored menu/Info
 *  popovers + keyboard roving + the full dismiss/Escape/inert contract).
 *
 *  **Archive and Delete sit last, right above Info.** They are the mutating
 *  actions here, so they stay off the top of the menu. A stray tap lands there,
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
 *  **Pin/Unpin shows only on a keyboard-open.** Every inline pin button sitting
 *  next to a ⋯ trigger is mouse-only (`tabindex=-1`), so the menu is the
 *  keyboard's only route to it — but on a pointer-open that inline button is
 *  right there. So the item is gated on `ctx.openedViaKeyboard`.
 *
 *  The Info popover shows the thread's structured details (Status / You / Agent /
 *  Type / Exchanges / Started) — the same rows that used to ride the drawer
 *  row's hover tooltip, now reachable everywhere the ⋯ menu lives (drawer row +
 *  both thread-title headers). */
export function ThreadOverflowMenu({ threadId, title, stopPropagation, extraClass, tabIndex, hostOpener }: {
  threadId: string;
  title: string;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
  /** Lets the drawer row open this menu from its own gesture: a desktop
   *  right-click, or a mobile long press. See <OverflowMenu>. */
  hostOpener?: HostOpener;
}) {
  return (
    <OverflowMenu
      ariaLabel="More thread actions"
      stopPropagation={stopPropagation}
      extraClass={extraClass}
      tabIndex={tabIndex}
      hostOpener={hostOpener}
      // Read live thread meta only while a popover is open (OverflowMenu gates the
      // call). An unhydrated search hit has no live thread → null → no Info.
      infoRows={() => {
        const thread = threadMap.value.get(threadId);
        return thread ? threadInfoRows(thread.meta, effectiveThreadStatus(thread)) : null;
      }}
      items={({ openedViaKeyboard, run }) => {
        // These selectors read threadMap/changes signals; OverflowMenu invokes
        // `items` only while open, so a closed menu subscribes to neither.
        const liveThread = threadMap.value.get(threadId);
        const saved = liveThread?.meta.saved ?? false;
        const showPin = !!liveThread && openedViaKeyboard;
        const archiveAction = resolveThreadActions(threadId).find((a) => a.kind === 'archive');
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
            {showPin && (
              <>
                <button type="button" class="thread-overflow-item" role="menuitem"
                  onClick={run(() => { if (saved) void handleUnsaveThread(threadId); else void handleSaveThread(threadId); })}>
                  <PinIcon filled={saved} />
                  {saved ? 'Unpin thread' : 'Pin thread'}
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
