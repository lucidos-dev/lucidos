import { effect, signal } from '@preact/signals';
import {
  threadChannelFilter,
  selectedTriggerIds,
  selectedRepoIds,
  selectedAppIds,
  type ThreadChannel,
} from './store';
import { threadFilterPanelOpen } from './threadFilterPanel';

/** One snapshot of the whole thread-filter selection: the channel set plus the
 *  three facet sub-selections, which are only ever read together (the drawer's
 *  display predicate, the pagination cursor and the archived-count query all
 *  take all four). */
export type ThreadFilterSelection = {
  channels: ReadonlySet<ThreadChannel>;
  triggerIds: ReadonlySet<string>;
  repoIds: ReadonlySet<string>;
  appIds: ReadonlySet<string>;
};

function liveSelection(): ThreadFilterSelection {
  return {
    channels: threadChannelFilter.value,
    triggerIds: selectedTriggerIds.value,
    repoIds: selectedRepoIds.value,
    appIds: selectedAppIds.value,
  };
}

function sameIds<T>(a: ReadonlySet<T>, b: ReadonlySet<T>): boolean {
  if (a === b) return true;
  if (a.size !== b.size) return false;
  for (const id of a) {
    if (!b.has(id)) return false;
  }
  return true;
}

function sameSelection(a: ThreadFilterSelection, b: ThreadFilterSelection): boolean {
  return sameIds(a.channels, b.channels)
    && sameIds(a.triggerIds, b.triggerIds)
    && sameIds(a.repoIds, b.repoIds)
    && sameIds(a.appIds, b.appIds);
}

/** The thread-filter selection the drawer LIST renders and paginates from.
 *
 *  Identical to the live signals the *thread filter panel* writes
 *  (`threadChannelFilter` + `selectedTriggerIds` / `selectedRepoIds` /
 *  `selectedAppIds`), except while that panel is up, when it holds its previous
 *  value and catches up in one pass on close.
 *
 *  The panel is a view INSIDE the drawer pane that covers the list completely
 *  (`.thread-filter-panel` is `position: absolute; inset: 0` over the pane's own
 *  opaque background), so a tick of one of its checkboxes changes nothing the
 *  user can see. Read live, each tick nonetheless re-ran the drawer's O(threads)
 *  categorization pipeline, rebuilt and re-diffed every row (swapping hundreds
 *  of them in and out of the DOM), and fired `reloadAfterFilterChange`, whose
 *  fill loop then pages one sequential round trip at a time until the sentinel
 *  is pushed out of view. All of it behind an opaque panel, and the render half
 *  of it synchronously, so the paint that shows the box as ticked waited on the
 *  whole thing. That is the "the checkbox lags behind my tap" report this
 *  exists for. It is worst on the Coding Agent row, whose threads are the ones
 *  with sub-threads: collapsed families are dropped from the rendered list, so
 *  the fill loop keeps paging without the sentinel ever moving.
 *
 *  It is the single source for BOTH halves, display and fetch
 *  (`currentThreadFilterParams`), deliberately: the pagination cursor is
 *  computed with the same predicate the display uses precisely so the two
 *  cannot drift, and splitting them across live-vs-applied would reintroduce
 *  that drift for the window the panel is up.
 *
 *  Holding is only honest because the one surface that RENDERS from this is
 *  `ThreadList`, which is exactly what the panel covers. The panel's own
 *  checkboxes, its option lists and the header's filter-active highlight all
 *  read the live signals, so they answer the tap at once. A new surface that
 *  displays threads by this selection and is NOT inside the drawer pane would
 *  show a stale list whenever the panel is up: it reads the live signals, or it
 *  earns a reason not to.
 *
 *  Identity is the contract. `ThreadList` memoizes its categorization on this
 *  object and its refetch effect fires on a change of it, so a new object is a
 *  full recategorize plus a refetch. Hence the content compare below: a panel
 *  opened and closed without changing anything, or with a selection toggled
 *  away and back, must leave the same object in place. */
export const appliedThreadFilter = signal<ThreadFilterSelection>(liveSelection());

/** Kept here with the signal rather than in `store/effects.ts` (the usual home
 *  for module-level effects) because it IS the signal's contract: an
 *  `appliedThreadFilter` imported without it would silently never update.
 *
 *  The early return is what does the holding, and it works because it reads
 *  only `threadFilterPanelOpen` on that pass: the effect unsubscribes from the
 *  four live signals while the panel is up, so their ticks re-run nothing at
 *  all, and closing the panel re-runs the effect, which reads them again. */
effect(() => {
  if (threadFilterPanelOpen.value) return;
  const live = liveSelection();
  // `peek`, not `.value`: this effect must not depend on its own output.
  if (sameSelection(appliedThreadFilter.peek(), live)) return;
  appliedThreadFilter.value = live;
});
