import { computed } from '@preact/signals';
import {
  repositories,
  threadMap,
  selectedRepoIds,
  setSelectedRepoIds,
  threadChannelFilter,
} from './store';
import { toggleChannel } from './triggerFilters';

export type RepoFilterOption = {
  id: string;
  label: string;
  /** True when the repo no longer exists in the `/api/repositories` registry
   *  but threads still reference it. Renders with a `(deleted)` suffix. */
  deleted: boolean;
  /** ISO timestamp of the most-recent thread bound to this repo. Used to
   *  order deleted entries — most-recent first so the user's eye lands on
   *  what they were running last. */
  lastActivity?: string;
};

/** Returns [] until the registry loads — without it every live repo would
 *  mis-label as "(deleted)". Selected ids are always included so a filter
 *  restored from localStorage with no matching threads loaded is still
 *  clearable. Mirrors `triggerFilterOptions`. */
export const repoFilterOptions = computed<RepoFilterOption[]>(() => {
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

  for (const repo of repositories.value.data) push(repo.id);
  for (const entry of threadMap.value.values()) {
    if (entry.meta.channel !== 'claude_code') continue;
    const id = entry.meta.repoId;
    if (!id) continue;
    push(id, entry.meta.repoName, entry.meta.updatedAt);
  }
  for (const id of selectedRepoIds.value) push(id);

  return result.sort((a, b) => {
    if (a.deleted !== b.deleted) return a.deleted ? 1 : -1;
    if (a.deleted) {
      const aTime = a.lastActivity ?? '';
      const bTime = b.lastActivity ?? '';
      if (aTime !== bTime) return bTime.localeCompare(aTime);
    }
    return a.label.localeCompare(b.label);
  });
});

export function toggleRepoId(id: string): void {
  const next = new Set(selectedRepoIds.value);
  if (next.has(id)) next.delete(id); else next.add(id);
  setSelectedRepoIds(next);
}

/** Tri-state click handler for the Claude Code parent row. Indeterminate
 *  clears per-repo selection (= "all CC threads"); fully checked turns the
 *  channel off and clears selection; unchecked turns the channel on. With a
 *  single repo, "all" and "just this one" are identical results, so the
 *  indeterminate state is meaningless and the click toggles the channel. */
export function toggleClaudeCodeChannel(): void {
  const channelOn = threadChannelFilter.value.has('claude_code');
  const hasSelection = selectedRepoIds.value.size > 0;
  const lockstep = repoFilterOptions.value.length === 1;
  if (channelOn && hasSelection && !lockstep) {
    setSelectedRepoIds(new Set());
    return;
  }
  if (channelOn) setSelectedRepoIds(new Set());
  toggleChannel('claude_code');
}
