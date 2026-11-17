/**
 * The notifications group: the Lucidos menu's top rows, one per workspace
 * holding unread notifications.
 *
 * It exists because the app-icon badge is a CROSS-WORKSPACE total while every
 * in-app count is per-workspace. A gateway install badges one icon for the
 * whole origin. So the OS can show a `1` belonging to a workspace the user is
 * not looking at, and nothing on screen said where it lived. These rows are
 * that answer, and tapping one goes to the notifications view that owns it.
 *
 * It also puts a count on the two panes the bell never reaches. The bell lives
 * in `ContentHeaderActions`, the content pane's header, so the thread pane and
 * the threads drawer carry no unread count at all. The brand mark's badge is
 * the glanceable half of this; the rows are the actionable half.
 *
 * WHERE a peer row opens is not decided here. `utils/workspaceWindow.ts` owns
 * that, as it does for the switcher below and for the picker. So a
 * notification row and a switcher row for one workspace cannot disagree. What
 * this group adds is the LANDING: a peer opens on its notifications view.
 *
 * Two shapes, the same split `WorkspaceSwitcher.tsx` uses beside it.
 * `notificationsMenuGroup` is PURE, so every branch is unit-testable through
 * `vnodeToText` with no DOM. `NotificationsMenuGroup` is the hook-bearing
 * wrapper that holds the one piece of state an opening of the menu has.
 */

import { useSignal } from '@preact/signals';
import { peerWorkspaces, remotePeerWorkspaces } from '../../store/actions/app-badge';
import { switchMenuItem } from '../../store/actions/menu';
import { showToast, unreadCount, visibleWorkspaceName } from '../../store/store';
import { REMOTE_GATEWAY, WORKSPACE_ID } from '../../utils/basePath';
import { notifyRows, type NotifyRow, type NotifyRowsInput } from '../../utils/notificationRows';
export { notifyRows } from '../../utils/notificationRows';
export type { NotifyRow } from '../../utils/notificationRows';
import { errorDetail } from '../../utils/errorDetail';
import {
  alternateOpenMode,
  middleClickActivates,
  middleClickHandler,
  openModeForClick,
  openWorkspaceIn,
  type WorkspaceOpenMode,
} from '../../utils/workspaceWindow';
import { BellIcon } from '../shared/icons';
import { workspaceActionRow } from './WorkspaceActionRow';
import { GlyphBadge } from '../shared/GlyphBadge';
import { countLabel } from './unreadCountLabel';

/** The indent an unfolded action takes under a row of THIS group, whose rows
 *  are `.brand-menu-item`s leading with the bell. The switcher passes its own,
 *  its rows leading with a status dot instead. */
const NOTIFY_ACTION_INDENT = 'brand-menu-ws-action-under-icon';

/** What the group needs to draw itself. Threaded whole rather than unpacked
 *  into positional arguments, matching `SwitcherListProps` next door. */
export interface NotificationsGroupProps extends NotifyRowsInput {
  /** This workspace's row was activated, which is an in-app view switch. */
  onOpenOwn: () => void;
  /** A peer row was activated. The event decides the mode, so a cmd-click or a
   *  middle-click opens beside where a plain click switches in place. */
  onActivatePeer: (row: NotifyRow, e: MouseEvent) => void;
  /** The peer whose action row is unfolded, from a right-click on it. At most
   *  one, so a second right-click moves it rather than stacking. */
  contextId: string | null;
  /** A right-click landed on the row for this workspace. `null` is a page with
   *  no slug, whose one row is its own and unfolds nothing anyway. */
  onContext: (id: string | null) => void;
  /** The unfolded action row was pressed, in the mode it offered. */
  onAlternate: (row: NotifyRow, mode: WorkspaceOpenMode) => void;
}

/** The alternate a right-click offers on `row`, or `null` for a row with none.
 *
 *  This workspace's own row never has one: activating it switches an in-app
 *  view, which is not a window question at all. Every other row asks the same
 *  function the switcher and the picker ask, so the three cannot disagree about
 *  what a right-click is worth offering. */
function rowAlternate(row: NotifyRow, currentId: string | null): WorkspaceOpenMode | null {
  if (row.isSelf || row.id === null) return null;
  return alternateOpenMode(row.state, currentId, row.id);
}

function notifyRow(row: NotifyRow, props: NotificationsGroupProps) {
  // The workspace is named in the accessible label rather than left to the
  // visible text. The count beside it is a bare number, so "dev 2" read aloud
  // says nothing about what the 2 counts.
  const label = row.count === 1
    ? `1 unread notification in ${row.name}`
    : `${row.count} unread notifications in ${row.name}`;
  // A plain activation. This workspace's row is an in-app switch and takes no
  // mode; a peer's hands the event on, and the caller reads the gesture.
  const activate = (e: MouseEvent) => {
    if (row.isSelf) props.onOpenOwn();
    else props.onActivatePeer(row, e);
  };
  // A middle press means "open this beside", so a row with nowhere to open must
  // do nothing rather than fall back to its primary action. Never on this
  // workspace's row: a wheel press must not switch the view under the user.
  const aux = !row.isSelf && middleClickActivates(row.state)
    ? { onAuxClick: middleClickHandler(activate) }
    : {};
  return (
    <button
      type="button"
      class="brand-menu-item brand-menu-notif-row"
      role="menuitem"
      aria-label={label}
      onClick={activate}
      {...aux}
      // Claim the right-click for the action row below. Without
      // `preventDefault` the webview's own menu covers the panel, which is what
      // a right-click on one of these rows does today. Claimed on EVERY row,
      // not only the ones an action can unfold under: tying the two together is
      // what left the switcher with a row raising the native menu.
      onContextMenu={(e: MouseEvent) => { e.preventDefault(); props.onContext(row.id); }}
      // A colon cannot appear in a slug, so this can never collide with a
      // sibling key. `null` is this workspace on a page with no slug, and there
      // is at most one of those.
      key={`notif:${row.id ?? ''}`}
    >
      <BellIcon />
      {/* The switcher list's own name class, not a copy: both rows carry a
          workspace name that must ellipsise. The count lives only here, since
          the switcher list leaves it to this group. */}
      <span class="brand-menu-ws-name">{row.name}</span>
      <GlyphBadge class="brand-menu-ws-badge" aria-hidden="true">{countLabel(row.count)}</GlyphBadge>
    </button>
  );
}

/** The group, or nothing at all when every workspace is read.
 *
 *  Rendering nothing is the common case and has to cost nothing. A user with no
 *  unreads must see the menu exactly as it was before this existed, with no
 *  empty box and no stray separator. So the separator is emitted from HERE,
 *  under the rows, rather than by the caller. */
export function notificationsMenuGroup(props: NotificationsGroupProps) {
  const rows = notifyRows(props);
  if (rows.length === 0) return null;
  return (
    <>
      <div class="brand-menu-notif-group" role="group" aria-label="Unread notifications">
        {/* The action is emitted right under the row it belongs to, rather
            than at the end, so it cannot be read as belonging to whichever row
            it happened to land beside. Same shape as `workspaceSwitcherList`,
            and it asks the same function which rows carry one. */}
        {rows.flatMap((row) => {
          const mode = rowAlternate(row, props.ownId);
          return [
            notifyRow(row, props),
            row.id !== null && workspaceActionRow({
              open: row.id === props.contextId,
              id: row.id,
              name: row.name,
              mode,
              indentClass: NOTIFY_ACTION_INDENT,
              onActivate: (m) => props.onAlternate(row, m),
            }),
          ];
        })}
      </div>
      <div class="brand-menu-separator" role="separator" />
    </>
  );
}

/** The group plus the one piece of state an opening of the menu holds: which
 *  row has its action unfolded.
 *
 *  A COMPONENT rather than a call, so that state can live inside the menu's
 *  `<Overlay>`, which unmounts on close. A fresh open therefore starts with
 *  nothing unfolded. Holding the flag in `LucidosMenu` does not work: that
 *  component renders the Overlay rather than sitting inside it. So it stays
 *  mounted while the menu is shut, and carries a stale unfold into the next
 *  open. `WorkspacesMenuRow` is the same trick for the same reason.
 *
 *  Reading the counts here rather than taking them as props keeps the two
 *  halves of the menu symmetrical, and keeps the caller to one line. */
export function NotificationsMenuGroup({ onClose }: { onClose: () => void }) {
  const contextId = useSignal<string | null>(null);

  // Every way out of a peer's row. The menu always shuts. An in-place switch
  // replaces the document, and a menu left over the loading page reads as a tap
  // that missed. A separate view happens elsewhere, so the menu would sit over
  // work already done.
  //
  // `openWorkspaceIn` decides the window or the tab, exactly as it does for the
  // switcher and the picker. The landing is what this row adds. The toast is
  // the only report the user gets, and this is a direct click, so the telemetry
  // carve-out in `.claude/rules/frontend.md` does not cover it.
  const open = (row: NotifyRow, mode: WorkspaceOpenMode) => {
    contextId.value = null;
    onClose();
    // Non-null for a peer: peers only exist behind the gateway, and every row
    // there comes from the control listing.
    if (!row.id) return;
    void openWorkspaceIn(mode, row.id, 'notifications').catch((e: unknown) => {
      showToast(`Could not open ${row.name}: ${errorDetail(e)}`, 'error');
    });
  };

  return notificationsMenuGroup({
    peers: peerWorkspaces.value,
    remotePeers: remotePeerWorkspaces.value,
    ownUnread: unreadCount.value,
    ownId: WORKSPACE_ID,
    // An empty name means `/health` has not answered, so the row falls back.
    ownName: visibleWorkspaceName.value
      ? visibleWorkspaceName.value + (REMOTE_GATEWAY ? ` @${REMOTE_GATEWAY}` : '')
      : null,
    onOpenOwn: () => { onClose(); switchMenuItem('notifications'); },
    // A null mode is a gesture that is not an activation at all (a macOS
    // Ctrl-click, which the action row already answers).
    onActivatePeer: (row, e) => {
      const mode = openModeForClick(e);
      if (mode !== null) open(row, mode);
    },
    contextId: contextId.value,
    onContext: (id) => { contextId.value = id; },
    onAlternate: open,
  });
}
