import { updateAvailable, engineNewVersionReady, engineVersionPending, engineRebuildWedged, mobileView } from '../../store/store';
import { crossWorkspaceUnreadTotal } from '../../store/actions/app-badge';
import { liveActivityRows, type ActivityRow } from '../../store/actions/activityRows';
// One cap for both header counts, so the mark and the menu rows cannot start
// eliding at different numbers. Each site's own box is its own business; the
// number at which a count stops being spelled out is shared.
import { countLabel } from './NotificationsMenuRows';
import { HourglassIcon, ReloadIcon } from '../shared/icons';

/** Visible state of the brand badge. PURELY VISUAL in every state: a tap on it
 *  is a tap on the mark, which opens the Lucidos menu. The menu's activity
 *  group is where each state is explained and acted on.
 *
 *  - `busy`: a job is in flight, shown as a spinning refresh icon. The jobs are
 *    the activity group's rows (`store/actions/activityRows.ts`): builds, the
 *    embedding-model download, an Expose run, and applies.
 *  - `ready`: a new engine version is ready to switch onto, or a newer client
 *    bundle is available to refresh, shown as a single `!` attention mark. The
 *    menu's Refresh and Restart rows take it.
 *  - `pending`: new code exists in source with no version built behind it,
 *    shown as a quieter dot. The menu's pending row re-opens its toast.
 *  - `none`: nothing to surface, and the badge is not rendered.
 *
 *  Ordered by how much the user can do about it. Busy wins over ready (a
 *  switch/refresh isn't offered until the work lands), and ready wins over
 *  pending (something you can take now beats something that isn't built). */
export type BrandBadgeState = 'busy' | 'ready' | 'pending' | 'none';

export function brandBadgeState(activityCount: number): BrandBadgeState {
  if (activityCount > 0) return 'busy';
  if (engineNewVersionReady() || updateAvailable.value) return 'ready';
  if (engineVersionPending.value) return 'pending';
  return 'none';
}

/** What the state badge stands for, in the menu's own words, or `undefined`
 *  when it is not drawn. The MARK speaks it, in its label and its tooltip:
 *  the badge is click-through, so neither a pointer nor a screen reader ever
 *  lands on the badge itself. */
export function brandBadgeLabel(rows: ActivityRow[]): string | undefined {
  if (rows.length > 0) return rows.map((r) => r.label).join(' · ');
  const newVersion = engineNewVersionReady();
  const update = updateAvailable.value;
  if (newVersion && update) return 'New version available · Client update available';
  if (newVersion) return 'New version available';
  if (update) return 'Client update available';
  if (engineVersionPending.value) {
    return engineRebuildWedged.value ? 'New code pending · no rebuild can deliver it' : 'New code pending';
  }
  return undefined;
}

/** The unread count in words, or `null` when there is nothing to say.
 *
 *  Lives here with the badge, but is spoken by the MARK: see
 *  {@link UnreadBrandBadge} for why the badge itself is silent. */
export function unreadBadgeLabel(count: number): string | null {
  if (count <= 0) return null;
  return count === 1 ? '1 unread notification' : `${count} unread notifications`;
}

/** The unread count on the mark, and the ONLY in-app mirror of the app-icon
 *  badge. Both read `crossWorkspaceUnreadTotal`, so they cannot show different
 *  numbers.
 *
 *  It rides the mark rather than the bell because the bell is not on every
 *  pane: `ContentHeaderActions` owns it, so the thread pane and the threads
 *  drawer carried no count at all. The mark is on all three.
 *
 *  It sits at the mark's BOTTOM-right corner, not the top-right the bell and
 *  the filter button use for a count. The artwork puts its sparkle top-right,
 *  and a resident badge there leaves the brand as three plain squares. The
 *  state badge above covers it only while something is happening.
 *
 *  PURELY VISUAL, and that is why it carries neither a tooltip nor a name. It
 *  is `pointer-events: none` in CSS, so the tap and the hover both belong to
 *  the mark underneath. A `data-tooltip` here could never fire: `useTooltip`
 *  resolves its target by walking UP from the hovered element, and this badge
 *  is never that element. The mark speaks the count instead, through
 *  {@link unreadBadgeLabel} in its own label and tooltip. */
export function UnreadBrandBadge() {
  const count = crossWorkspaceUnreadTotal.value;
  if (count <= 0) return null;
  return (
    <span class="badge brand-unread-badge" aria-hidden="true">
      {countLabel(count)}
    </span>
  );
}

/** The brand badge shared by the desktop brand label and the mobile mark.
 *  It spins while a job runs, shows `!` when a switch or refresh is ready,
 *  and a dot when new code is pending. Reads the driving signals in its own
 *  render, so it re-renders in place as they change. */
export function BrandBadge() {
  const rows = liveActivityRows();
  const state = brandBadgeState(rows.length);
  if (state === 'none') return null;
  if (state === 'ready') {
    return <span class="badge brand-badge" aria-hidden="true">!</span>;
  }
  const pending = state === 'pending';
  const waiting = rows.every((r) => r.queued);
  return (
    <span
      class={`badge brand-badge${pending ? ' brand-badge-dot' : ''}${pending && engineRebuildWedged.value ? ' brand-badge-wedged' : ''}`}
      aria-hidden="true"
    >
      {/* Pending draws no glyph at all: the badge box IS the dot. The `!` is
          spoken for by `ready`, and a second attention mark beside it would say
          "act on this" about the one state where there is nothing to act on
          yet.

          Keyed on the mobile pane: each mobile header has its own badge, and
          the hidden headers are `display: none`. iOS WebKit can leave a spin
          frozen on an element that comes back from that, so each swap draws
          a fresh element whose animation starts anew. */}
      {!pending && !waiting && <span key={mobileView.value} class="brand-badge-spinner"><ReloadIcon /></span>}
      {!pending && waiting && <span class="brand-badge-queued"><HourglassIcon /></span>}
    </span>
  );
}
