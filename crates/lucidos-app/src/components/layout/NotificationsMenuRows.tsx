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
 * Pure, holding no hooks, so every branch is unit-testable through
 * `vnodeToText` with no DOM. Same split as `workspaceSwitcherList` beside it.
 *
 * See docs/plans/2026-08-21-unread-total-on-the-brand-and-in-the-menu.md.
 */

import type { WorkspaceStatus } from '../../api/client/control';
import { BellIcon } from '../shared/icons';

/** One row's worth of the group, resolved from the two count sources. */
export interface NotifyRow {
  /** Registry slug, or `null` for this workspace on a non-gateway page. */
  id: string | null;
  name: string;
  count: number;
  /** True for the workspace this page serves, which routes in-app. */
  isSelf: boolean;
}

/** What the group needs to draw itself. Threaded whole rather than unpacked
 *  into positional arguments, matching `SwitcherListProps` next door. */
export interface NotificationsGroupProps {
  /** Every workspace the gateway serves, from the shared cache in
   *  `store/actions/app-badge.ts`. Empty outside a gateway-served page. */
  peers: WorkspaceStatus[];
  /** This workspace's LIVE unread count, never its polled row. See
   *  {@link notifyRows}. */
  ownUnread: number;
  /** This workspace's slug, or `null` on a legacy no-gateway page. */
  ownId: string | null;
  /** What to call this workspace. Falls back when `/health` has not answered. */
  ownName: string | null;
  onOpenOwn: () => void;
  onOpenPeer: (row: NotifyRow) => void;
}

/** What this workspace's row is called before its name has resolved. */
const OWN_FALLBACK_NAME = 'This workspace';

/** Above this the count is drawn as `99+`. The bell caps at 999, and can
 *  afford to: it is a full-size badge on a roomy icon. This one rides a menu
 *  row beside a name it must not squeeze. */
const MAX_SHOWN_COUNT = 99;

/** The rows to draw, in order: this workspace first, then the peers as the
 *  gateway listed them.
 *
 *  Two count sources, deliberately. This workspace's comes from the caller's
 *  LIVE `unreadCount`, so an optimistic mark-read empties the row on the same
 *  tick. Every other workspace's comes from the polled listing, the only view
 *  we have of an engine this page holds no connection to.
 *
 *  Our own row is dropped from `peers` for the same reason. That row is stale
 *  for a second or two after a mark-read. Two rows for one workspace
 *  disagreeing is worse than one slightly stale row.
 *
 *  A workspace with nothing unread is not a row. A stopped one reports no count
 *  at all and is therefore never a row either, which is the same
 *  running-workspaces-only rule the icon badge follows. */
export function notifyRows({ peers, ownUnread, ownId, ownName }: NotificationsGroupProps): NotifyRow[] {
  const rows: NotifyRow[] = [];
  if (ownUnread > 0) {
    rows.push({ id: ownId, name: ownName ?? OWN_FALLBACK_NAME, count: ownUnread, isSelf: true });
  }
  for (const w of peers) {
    if (w.id === ownId) continue;
    const count = w.unread_count ?? 0;
    if (count > 0) rows.push({ id: w.id, name: w.name, count, isSelf: false });
  }
  return rows;
}

/** The count, capped. Pure and exported so the cap is pinned by a test rather
 *  than by the two call sites happening to agree. */
export function countLabel(count: number): string {
  return count > MAX_SHOWN_COUNT ? `${MAX_SHOWN_COUNT}+` : String(count);
}

function notifyRow(row: NotifyRow, props: NotificationsGroupProps) {
  // The workspace is named in the accessible label rather than left to the
  // visible text. The count beside it is a bare number, so "dev 2" read aloud
  // says nothing about what the 2 counts.
  const label = row.count === 1
    ? `1 unread notification in ${row.name}`
    : `${row.count} unread notifications in ${row.name}`;
  return (
    <button
      type="button"
      class="brand-menu-item brand-menu-notif-row"
      role="menuitem"
      aria-label={label}
      onClick={() => (row.isSelf ? props.onOpenOwn() : props.onOpenPeer(row))}
      // A colon cannot appear in a slug, so this can never collide with a
      // sibling key. `null` is this workspace on a page with no slug, and there
      // is at most one of those.
      key={`notif:${row.id ?? ''}`}
    >
      <BellIcon />
      {/* The switcher list's own name and count classes, not copies of them.
          These rows carry the same two things (a workspace name that must
          ellipsise, and its unread count as a pill), so a second pair would be
          two rules to keep in step for one appearance. */}
      <span class="brand-menu-ws-name">{row.name}</span>
      <span class="brand-menu-ws-badge" aria-hidden="true">{countLabel(row.count)}</span>
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
        {rows.map((row) => notifyRow(row, props))}
      </div>
      <div class="brand-menu-separator" role="separator" />
    </>
  );
}
