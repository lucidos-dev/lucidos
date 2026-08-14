import { updateAvailable, engineBuilding, engineBuildDetail, engineNewVersionReady, engineVersionPending, engineRebuildWedged, embeddingModelStatus, tailscaleServeRun } from '../../store/store';
import { backgroundActivities, type BackgroundActivity } from '../../store/backgroundActivity';
import { openBackgroundActivityToast } from '../../store/actions/backgroundActivity';
import { openEngineVersionToast } from '../../store/actions/engine-update';
import { focusPane } from '../../store/actions/pane';
import { ReloadIcon } from '../shared/icons';

/** Visible state of the brand badge.
 *
 *  - `busy`: background activity is in flight, shown as a spinning refresh
 *    icon. Covers a dev engine rebuild, the embedding-model download AND an
 *    Expose run (see `store/backgroundActivity.ts` for exactly what counts).
 *    Clickable, and it opens the status toast naming what is happening.
 *  - `ready`: a new engine version is ready to switch onto, or a newer client
 *    bundle is available to refresh, shown as a single `!` attention mark. Not
 *    clickable itself; the click falls through to its host, which opens the
 *    Lucidos menu where the Refresh and Restart controls live.
 *  - `pending`: new code exists in source with no version built behind it, shown
 *    as a quieter dot. Clickable, and it re-opens the pending version toast,
 *    which is where both the explanation and the Rebuild live. It is clickable
 *    precisely BECAUSE that toast is dismissable: a persistent badge with no way
 *    back to what it refers to would be a dot the user cannot resolve. Unlike
 *    `ready` it has no home in the Lucidos menu to fall through to, since
 *    Restart would respawn the same engine.
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

export function brandBadgeTooltip(activities: BackgroundActivity[]): string | undefined {
  if (activities.length > 0) {
    // Name the work, and promise details only when there ARE some. The tooltip
    // shows the labels; the toast shows everything else (byte counts, elapsed
    // build time, the commits coming, the not-indexed-yet caveat, the actions).
    // So the promise is DERIVED from whether any activity carries one of those,
    // never asserted beside it: an activity with nothing but a label makes the
    // tap open a toast that repeats this very tooltip, which is exactly what the
    // engine build did before it learned to say more.
    const labels = activities.map((a) => a.label).join(' · ');
    return hasToastDetail(activities) ? `${labels} · tap for details` : labels;
  }
  const newVersion = engineNewVersionReady();
  const update = updateAvailable.value;
  if (newVersion && update) return 'New version available · Client update available';
  if (newVersion) return 'New version available';
  if (update) return 'Client update available';
  // Pending says what EXISTS, never what to press: "new code" is true whether or
  // not a rebuild can turn it into a version, and the two differ only in what
  // the toast behind the tap can offer. So the wedged variant drops the promise
  // of details rather than describing a button that isn't there, on the same
  // rule the activity tooltip above follows.
  if (engineVersionPending.value) {
    return engineRebuildWedged.value
      ? 'New code pending · no rebuild can deliver it'
      : 'New code pending · tap to rebuild';
  }
  return undefined;
}

/** Does the status toast have anything to say that this tooltip doesn't? True
 *  when some activity carries a detail, a note, or an action. Covered through
 *  `brandBadgeTooltip`, whose doc comment holds the rule. */
function hasToastDetail(activities: BackgroundActivity[]): boolean {
  return activities.some((a) => !!(a.detail || a.note || a.action || a.secondaryAction));
}

/** The brand badge shared by the desktop brand label and the mobile mark.
 *  Renders a spinning refresh icon while background activity runs, a `!` when a
 *  switch/refresh is ready, and nothing otherwise. Reads the driving signals in
 *  its own render so it re-renders in place as they change. */
export function BrandBadge() {
  const activities = backgroundActivities(
    engineBuilding.value,
    embeddingModelStatus.value,
    tailscaleServeRun.value,
    engineBuildDetail.value,
  );
  const state = brandBadgeState(activities.length);
  if (state === 'none') return null;
  const tooltip = brandBadgeTooltip(activities);
  if (state === 'ready') {
    return <span class="badge brand-badge" data-tooltip={tooltip}>!</span>;
  }
  const pending = state === 'pending';
  return (
    <button
      type="button"
      class={`badge brand-badge brand-badge-action${pending ? ' brand-badge-dot' : ''}${pending && engineRebuildWedged.value ? ' brand-badge-wedged' : ''}`}
      data-tooltip={tooltip}
      aria-label={tooltip}
      onClick={(e) => {
        // The badge sits INSIDE its host (the desktop `.pane-header-brand-label`,
        // the mobile `.brand-mark-slot`), whose onClick opens the Lucidos menu
        // for any click on a child. Without this the tap would pop that menu
        // over the toast it just opened.
        e.stopPropagation();
        // That swallow also eats the brand's OWN `focusPane('thread')` (the
        // `.pane-header-brand` wrapper in AppHeader), so claim the Threads pane
        // group here instead. `showToast` freezes a new toast over the pane
        // focused at that moment and the badge lives in the thread header, so
        // without this a tap taken while the content pane was focused pins the
        // status toast over the content pane. No-op on mobile, where the toast
        // spans the screen anyway.
        focusPane('thread');
        if (pending) openEngineVersionToast();
        else openBackgroundActivityToast();
      }}
    >
      {/* Pending draws no glyph at all: the badge box IS the dot. The `!` is
          spoken for by `ready`, and a second attention mark beside it would say
          "act on this" about the one state where there is nothing to act on
          yet. The tooltip and the toast carry the words. */}
      {!pending && <span class="brand-badge-spinner"><ReloadIcon /></span>}
    </button>
  );
}
