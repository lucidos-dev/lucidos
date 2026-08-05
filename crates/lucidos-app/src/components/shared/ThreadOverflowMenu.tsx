import { OverflowMenu } from './OverflowMenu';
import { CopyIcon, DownloadIcon, ArchiveIcon, PinIcon } from './icons';
import { copyThreadRef, copyThreadTitle } from '../../utils/threadRef';
import { exportThread } from '../../utils/exportThread';
import { resolveThreadActions } from '../../store/actions/threadActions';
import { handleSaveThread, handleUnsaveThread } from '../../store/actions/threads';
import { threadMap, effectiveThreadStatus } from '../../store/store';
import { threadInfoRows } from '../drawer/threadRowInfo';

/** Per-thread overflow (⋯) menu for a STARTED thread: a Pin/Unpin toggle first,
 *  then Copy thread reference / Copy thread title / Download thread, then the
 *  conditional Archive action, with the Info row auto-appended by <OverflowMenu>.
 *  Built on the shared <OverflowMenu> shell (trigger + anchored menu/Info
 *  popovers + keyboard roving + the full dismiss/Escape/inert contract).
 *
 *  **Archive sits second-last, right above Info.** It is the one mutating action
 *  here, so it stays off the top of the menu, where a stray tap (or the
 *  keyboard-open's focus landing on the first item) would reach it.
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
export function ThreadOverflowMenu({ threadId, title, stopPropagation, extraClass, tabIndex }: {
  threadId: string;
  title: string;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
}) {
  return (
    <OverflowMenu
      ariaLabel="More thread actions"
      stopPropagation={stopPropagation}
      extraClass={extraClass}
      tabIndex={tabIndex}
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
            {archiveAction && (
              <>
                <div class="thread-overflow-divider" role="separator" />
                <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void archiveAction.invoke(); })}>
                  <ArchiveIcon />
                  Archive
                </button>
              </>
            )}
          </>
        );
      }}
    />
  );
}
