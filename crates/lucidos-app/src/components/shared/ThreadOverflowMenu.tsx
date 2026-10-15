import { OverflowMenu, type HostOpener, type TriggerFace } from './OverflowMenu';
import { lazyComponent } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

/** The menu's rows, fetched once the boot splash lifts (ADR 0288). */
export const ThreadOverflowItemsChunk = lazyComponent(() => import('./ThreadOverflowItems').then((m) => m.ThreadOverflowItems));
prefetchWhenIdle(ThreadOverflowItemsChunk);

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
 *  **Show in Folders leads the menu** wherever the host passes
 *  `onShowInFolders`: the thread titles, and a drawer row under Ongoing. A
 *  row under Folders leaves it out, since it would point at itself.
 *  **Find in thread** joins it in the thread titles only
 *  (`onFindInThread`): it searches the open transcript.
 *
 *  **Rename… follows, on a sent thread.** This menu is the only place to
 *  rename, because the title is display-only. A draft is titled by its compose
 *  text, so a rename there would change nothing on screen. The dialog carries
 *  the Suggest link, except on the home thread: only the user names it (ADR 0362).
 *
 *  **Add widget… follows Rename… on a sent thread**, except Home, whose board
 *  has its own Add (ADR 0466). It steps the menu into the reusable widgets,
 *  the board's picker, and a pick pins one to this thread's shelf. Escape or
 *  its Back row steps out.
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
      itemsChunk={ThreadOverflowItemsChunk}
      // OverflowMenu invokes `items` only while open, so a closed menu
      // subscribes to none of the signals the rows read.
      items={({ run }) => (
        <ThreadOverflowItemsChunk threadId={threadId} title={title} run={run}
          onShowInFolders={onShowInFolders} onFindInThread={onFindInThread} />
      )}
    />
  );
}
