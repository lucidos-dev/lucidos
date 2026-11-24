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
import { appFilterOptions, widgetFilterOptions } from './appFilters';
import { loadedOr } from './types';
import { visibleFilterOptions } from './deletedFilterOptions';

export type RepoFilterOption = {
  id: string;
  label: string;
  deleted: boolean;
  lastActivity?: string;
};

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

export const repoFilterOptions = computed<RepoFilterOption[]>(() =>
  visibleFilterOptions(repoFilterOptionsAll.value, selectedRepoIds.value));

export function toggleRepoId(id: string): void {
  const next = new Set(selectedRepoIds.value);
  if (next.has(id)) next.delete(id); else next.add(id);
  setSelectedRepoIds(next);
}

export function toggleCodingAgentChannel(): void {
  const channelOn = threadChannelFilter.value.has(CODING_AGENT_CHANNEL);
  const hasSelection = selectedRepoIds.value.size + selectedAppIds.value.size > 0;
  const lockstep = repoFilterOptions.value.length + appFilterOptions.value.length
    + widgetFilterOptions.value.length === 1;
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
