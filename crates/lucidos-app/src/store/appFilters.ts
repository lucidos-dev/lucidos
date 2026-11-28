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
  deleted: boolean;
  widget: boolean;
  lastActivity?: string;
};

const threadAppIds = computed<ReadonlyMap<string, string | undefined>>(() => {
  const ids = new Map<string, string | undefined>();
  for (const entry of threadMap.value.values()) {
    if (entry.meta.codingAgentKind !== 'app') continue;
    const id = appIdFromFolder(entry.meta.codingAgentFolder);
    if (id && !ids.has(id)) ids.set(id, entry.meta.updatedAt);
  }
  return ids;
});

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

export const appIdsMissingFromFacets = computed<string[]>(() => {
  const facets = filterFacets.value;
  if (facets.status !== 'loaded') return [];
  const labelled = new Set(facets.data.apps.map(f => f.id));
  return [...threadAppIds.value.keys()].filter(id => !labelled.has(id));
});

export const appFilterOptions = computed<AppFilterOption[]>(() =>
  visibleFilterOptions(appFilterOptionsAll.value.filter(o => !o.widget), selectedAppIds.value));

export const widgetFilterOptions = computed<AppFilterOption[]>(() =>
  visibleFilterOptions(appFilterOptionsAll.value.filter(o => o.widget), selectedAppIds.value));

export function toggleAppId(id: string): void {
  const next = new Set(selectedAppIds.value);
  if (next.has(id)) next.delete(id); else next.add(id);
  setSelectedAppIds(next);
}
