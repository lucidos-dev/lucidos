import { computed } from '@preact/signals';
import {
  repositories,
  threadMap,
  selectedRepoIds,
  setSelectedRepoIds,
  selectedAppIds,
  setSelectedAppIds,
  threadChannelFilter,
  filterFacets,
  CODING_AGENT_CHANNEL,
} from './store';
import { toggleChannel } from './triggerFilters';
import { appFilterOptions } from './appFilters';
import { loadedOr } from './types';
import { visibleFilterOptions } from './deletedFilterOptions';

export type RepoFilterOption = {
  id: string;
  label: string;
  /** True when the repo no longer exists in the `/api/v1/repositories` registry
   *  but threads still reference it. Renders with a `(deleted)` suffix. */
  deleted: boolean;
  /** ISO timestamp of the most-recent thread bound to this repo. Used to
   *  order deleted entries — most-recent first so the user's eye lands on
   *  what they were running last. */
  lastActivity?: string;
};

/** Lists every repo that has a coding-agent thread. Completeness comes
 *  from the backend `filterFacets` (all session-having repos, even those whose
 *  threads aren't in the loaded window); the loaded `threadMap` adds
 *  just-created threads immediately; `selectedRepoIds` is always included so a
 *  filter restored from localStorage stays clearable. The repositories registry
 *  decides live-vs-`(deleted)`; the label for a deleted repo comes from the
 *  server-resolved `facet.name` (live registry → `repo_names` projection) or the
 *  thread's `meta.repoName`, falling back to the UUID only when no name was ever
 *  recorded. A repo registered solely for file browsing, with no coding-agent
 *  session, does not clutter the filter. Returns [] until the registry loads —
 *  without it every repo would mis-label as "(deleted)". */
export const repoFilterOptionsAll = computed<RepoFilterOption[]>(() => {
  if (repositories.value.status !== 'loaded') return [];
  const liveById = new Map(repositories.value.data.map(r => [r.id, r]));
  const result: RepoFilterOption[] = [];
  const seen = new Set<string>();

  const push = (id: string, fallbackLabel?: string, lastActivity?: string) => {
    if (seen.has(id)) return;
    seen.add(id);
    const live = liveById.get(id);
    if (live) {
      result.push({ id, label: live.name, deleted: false });
      return;
    }
    result.push({ id, label: fallbackLabel ?? id, deleted: true, lastActivity });
  };

  for (const facet of loadedOr(filterFacets.value, undefined)?.repos ?? []) {
    // `facet.name` carries the server-resolved label (live registry →
    // `repo_names` projection), so a removed repo shows its historical name
    // here rather than falling through to its raw UUID.
    if (facet.id) push(facet.id, facet.name ?? undefined, facet.last_activity ?? undefined);
  }
  for (const entry of threadMap.value.values()) {
    if (entry.meta.channel !== CODING_AGENT_CHANNEL) continue;
    const id = entry.meta.repoId;
    if (!id) continue;
    push(id, entry.meta.repoName, entry.meta.updatedAt);
  }
  for (const id of selectedRepoIds.value) push(id);

  return result;
});

/** The visible slice: unselected deleted entries dropped unless the user opts in
 *  (`visibleFilterOptions`), which is also what `deletedOptionsHidden` reports
 *  on. */
export const repoFilterOptions = computed<RepoFilterOption[]>(() =>
  visibleFilterOptions(repoFilterOptionsAll.value, selectedRepoIds.value));

export function toggleRepoId(id: string): void {
  const next = new Set(selectedRepoIds.value);
  if (next.has(id)) next.delete(id); else next.add(id);
  setSelectedRepoIds(next);
}

/** Tri-state click handler for the Coding Agent parent row. Indeterminate
 *  clears per-repo AND per-app selection (= "all coding-agent threads"); fully checked
 *  turns the channel off and clears both selections; unchecked turns the
 *  channel on. With a single coding target across both groups, "all" and "just
 *  this one" are identical results, so the indeterminate state is meaningless
 *  and the click toggles the channel. */
export function toggleCodingAgentChannel(): void {
  const channelOn = threadChannelFilter.value.has(CODING_AGENT_CHANNEL);
  const hasSelection = selectedRepoIds.value.size + selectedAppIds.value.size > 0;
  const lockstep = repoFilterOptions.value.length + appFilterOptions.value.length === 1;
  if (channelOn && hasSelection && !lockstep) {
    setSelectedRepoIds(new Set());
    setSelectedAppIds(new Set());
    return;
  }
  if (channelOn) {
    setSelectedRepoIds(new Set());
    setSelectedAppIds(new Set());
  }
  toggleChannel(CODING_AGENT_CHANNEL);
}
