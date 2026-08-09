import { includeDeletedFilterOptions } from './store';

/** What every filter-option list has in common: a pickable entity that may have
 *  been deleted while its threads live on. `triggerFilterOptions`,
 *  `repoFilterOptions` and `appFilterOptions` all widen this. */
export type DeletableOption = {
  id: string;
  label: string;
  deleted: boolean;
  /** ISO timestamp of the last thread bound to it. Set on deleted entries, so
   *  the group can be ordered most-recent first. */
  lastActivity?: string;
};

/** True when the CURRENT "Include deleted" setting is actually holding an option
 *  back: a deleted entity that exists and the user has not selected. Selected
 *  ones always stay visible so the filter remains clearable, so they are never
 *  "hidden" however the switch is set.
 *
 *  This is the difference between the setting being narrow and it NARROWING
 *  anything: on a workspace that has never deleted a trigger, repo or app,
 *  leaving the switch off excludes nothing, and the filter panel must not claim
 *  the list is filtered. `deletedOptionsHidden` (threadFilterActive.ts) is the
 *  reactive union of this across the three lists. */
export function hasHiddenDeleted(all: readonly DeletableOption[], selected: Set<string>): boolean {
  if (includeDeletedFilterOptions.value) return false;
  return all.some(o => o.deleted && !selected.has(o.id));
}

/** The options to SHOW: deleted entries are dropped unless the user opts in,
 *  except a selected one, which always stays so its filter can be cleared.
 *
 *  Sorted live-first, then deleted most-recent-first (the eye lands on the one
 *  they were most likely running last), then by label. Shared because all three
 *  lists want exactly this and had three copies of it. */
export function visibleFilterOptions<T extends DeletableOption>(
  all: readonly T[],
  selected: Set<string>,
): T[] {
  const includeDeleted = includeDeletedFilterOptions.value;
  return all
    .filter(o => includeDeleted || !o.deleted || selected.has(o.id))
    .sort((a, b) => {
      if (a.deleted !== b.deleted) return a.deleted ? 1 : -1;
      if (a.deleted) {
        const aTime = a.lastActivity ?? '';
        const bTime = b.lastActivity ?? '';
        if (aTime !== bTime) return bTime.localeCompare(aTime);
      }
      return a.label.localeCompare(b.label);
    });
}
