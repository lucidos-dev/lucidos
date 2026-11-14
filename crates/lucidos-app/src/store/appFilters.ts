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
import { visibleFilterOptions } from './deletedFilterOptions';

export type AppFilterOption = {
  id: string;
  label: string;
  /** True once the engine says the folder is gone, while threads still
   *  reference it. Renders with a `(deleted)` suffix. Mirrors
   *  `RepoFilterOption.deleted`. */
  deleted: boolean;
  /** True for a widget (ADR 0402), live or deleted. Lists under Widgets. */
  widget: boolean;
  /** ISO timestamp of the most-recent thread bound to this app. Used to order
   *  deleted entries, most-recent first. */
  lastActivity?: string;
};

/** App ids of the loaded coding-agent threads, with each one's newest time. */
const threadAppIds = computed<ReadonlyMap<string, string | undefined>>(() => {
  const ids = new Map<string, string | undefined>();
  for (const entry of threadMap.value.values()) {
    if (entry.meta.codingAgentKind !== 'app') continue;
    const id = appIdFromFolder(entry.meta.codingAgentFolder);
    if (id && !ids.has(id)) ids.set(id, entry.meta.updatedAt);
  }
  return ids;
});

/** Lists every app that has a coding-agent thread.
 *
 *  The engine labels each facet (ADR 0404), so a widget, which the apps list
 *  never holds, still labels as itself. Completeness comes from the facets;
 *  the loaded `threadMap` adds just-created threads, labelled from `appsList`
 *  until the facets catch up (`appIdsMissingFromFacets`). An id nothing can
 *  label is left out, never called deleted on a guess. A selected one stays,
 *  under its id, so a filter restored from localStorage stays clearable. */
export const appFilterOptionsAll = computed<AppFilterOption[]>(() => {
  const facets = loadedOr(filterFacets.value, undefined)?.apps ?? [];
  const listed = new Map(loadedOr(appsList.value, []).map(a => [a.id, a]));
  const selected = selectedAppIds.value;
  const result: AppFilterOption[] = [];
  const seen = new Set<string>();

  for (const facet of facets) {
    seen.add(facet.id);
    result.push({
      id: facet.id,
      label: facet.name || facet.id,
      deleted: facet.deleted,
      widget: facet.kind === 'widget',
      lastActivity: facet.deleted ? facet.last_activity ?? undefined : undefined,
    });
  }
  for (const id of [...threadAppIds.value.keys(), ...selected]) {
    if (seen.has(id)) continue;
    seen.add(id);
    const app = listed.get(id);
    if (app) {
      result.push({ id, label: app.name || id, deleted: false, widget: false });
    } else if (selected.has(id)) {
      result.push({ id, label: id, deleted: false, widget: false });
    }
  }
  return result;
});

/** Loaded threads' app ids the facets do not label yet: a widget a brand-new
 *  thread works in, say. Empty until the facets load. */
export const appIdsMissingFromFacets = computed<string[]>(() => {
  const facets = filterFacets.value;
  if (facets.status !== 'loaded') return [];
  const labelled = new Set(facets.data.apps.map(f => f.id));
  return [...threadAppIds.value.keys()].filter(id => !labelled.has(id));
});

/** The visible apps: unselected deleted entries dropped unless the user opts
 *  in (`visibleFilterOptions`). */
export const appFilterOptions = computed<AppFilterOption[]>(() =>
  visibleFilterOptions(appFilterOptionsAll.value.filter(o => !o.widget), selectedAppIds.value));

/** The visible widgets, sliced the same way. */
export const widgetFilterOptions = computed<AppFilterOption[]>(() =>
  visibleFilterOptions(appFilterOptionsAll.value.filter(o => o.widget), selectedAppIds.value));

export function toggleAppId(id: string): void {
  const next = new Set(selectedAppIds.value);
  if (next.has(id)) next.delete(id); else next.add(id);
  setSelectedAppIds(next);
}
