import { computed } from '@preact/signals';
import {
  appsList,
  threadMap,
  selectedAppIds,
  setSelectedAppIds,
  filterFacets,
} from './store';
import { appIdFromFolder } from '../utils/appIdFromFolder';
import { loadedOr } from './types';

export type AppFilterOption = {
  id: string;
  label: string;
  /** True when the app no longer appears in the live `appsList` but threads
   *  still reference it. Renders with a `(deleted)` suffix. Mirrors
   *  `RepoFilterOption.deleted`. */
  deleted: boolean;
  /** ISO timestamp of the most-recent thread bound to this app. Used to order
   *  deleted entries — most-recent first. */
  lastActivity?: string;
};

/** Lists every app that has a coding-agent thread. Completeness comes
 *  from the backend `filterFacets` (all session-having apps, even those whose
 *  threads aren't in the loaded window); the loaded `threadMap` adds
 *  just-created threads immediately; `selectedAppIds` is always included so a
 *  filter restored from localStorage stays clearable. The `appsList` registry
 *  supplies live-vs-`(deleted)` labels. Returns [] until `appsList` loads —
 *  without it every app would mis-label as "(deleted)". */
export const appFilterOptions = computed<AppFilterOption[]>(() => {
  if (appsList.value.status !== 'loaded') return [];
  const liveById = new Map(appsList.value.data.map(a => [a.id, a]));
  const result: AppFilterOption[] = [];
  const seen = new Set<string>();

  const push = (id: string, fallbackLabel?: string, lastActivity?: string) => {
    if (seen.has(id)) return;
    seen.add(id);
    const live = liveById.get(id);
    if (live) {
      result.push({ id, label: live.name || live.id, deleted: false });
      return;
    }
    result.push({ id, label: fallbackLabel ?? id, deleted: true, lastActivity });
  };

  for (const facet of loadedOr(filterFacets.value, undefined)?.apps ?? []) {
    if (facet.id) push(facet.id, facet.id, facet.last_activity ?? undefined);
  }
  for (const entry of threadMap.value.values()) {
    if (entry.meta.codingAgentKind !== 'app') continue;
    const id = appIdFromFolder(entry.meta.codingAgentFolder);
    if (!id) continue;
    push(id, id, entry.meta.updatedAt);
  }
  for (const id of selectedAppIds.value) push(id);

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

export function toggleAppId(id: string): void {
  const next = new Set(selectedAppIds.value);
  if (next.has(id)) next.delete(id); else next.add(id);
  setSelectedAppIds(next);
}
