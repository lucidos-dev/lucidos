import { computed } from '@preact/signals';
import { triggers, historicalTriggers, threadMap, selectedTriggerIds, setSelectedTriggerIds, threadChannelFilter, THREAD_CHANNEL_FILTER_KEY, filterFacets } from './store';
import type { ThreadChannel } from './store';
import { loadedOr } from './types';

export function toggleChannel(channel: ThreadChannel) {
  const current = threadChannelFilter.value;
  const next = new Set(current);
  if (next.has(channel)) next.delete(channel);
  else next.add(channel);
  threadChannelFilter.value = next;
  localStorage.setItem(THREAD_CHANNEL_FILTER_KEY, JSON.stringify([...next]));
}

export type TriggerFilterOption = {
  id: string;
  label: string;
  deleted: boolean;
  /** ISO timestamp of the last thread spawned by this trigger. Set on
   *  deleted entries so the dropdown can disambiguate same-named ones with
   *  an `(until <date>)` suffix. Live entries don't carry it. */
  lastActivity?: string;
};

/** Returns [] until the registry is loaded — without it we'd mis-label
 *  every live trigger as "(deleted)". Selected ids are always included so a
 *  filter restored from localStorage with no matching threads loaded is
 *  still clearable. */
export const triggerFilterOptions = computed<TriggerFilterOption[]>(() => {
  if (triggers.value.status !== 'loaded') return [];
  const liveById = new Map(triggers.value.data.map(t => [t.id, t]));
  const historical = loadedOr(historicalTriggers.value, []);
  const historicalById = new Map(historical.map(t => [t.id, t]));
  const result: TriggerFilterOption[] = [];
  const seen = new Set<string>();

  const push = (id: string, fallbackLabel?: string) => {
    if (seen.has(id)) return;
    seen.add(id);
    const live = liveById.get(id);
    if (live) {
      result.push({ id, label: live.name, deleted: false });
      return;
    }
    const histEntry = historicalById.get(id);
    result.push({
      id,
      label: histEntry?.name ?? fallbackLabel ?? id,
      deleted: true,
      lastActivity: histEntry?.last_activity,
    });
  };

  for (const trigger of triggers.value.data) {
    if (trigger.run.type === 'intent') push(trigger.id);
  }
  for (const t of historical) push(t.id, t.name ?? undefined);
  for (const entry of threadMap.value.values()) {
    if (entry.meta.channel !== 'trigger') continue;
    const id = entry.meta.triggerId;
    if (!id) continue;
    push(id, entry.meta.triggerName);
  }
  // Backend facets add any trigger that has a thread beyond the loaded window
  // (completeness parity with repos/apps). Run last among the name-bearing
  // sources so registry / historical / threadMap labels win — a facet-only
  // trigger falls back to its id via push().
  for (const facet of loadedOr(filterFacets.value, undefined)?.triggers ?? []) {
    if (facet.id) push(facet.id);
  }
  for (const id of selectedTriggerIds.value) push(id);

  return result.sort((a, b) => {
    if (a.deleted !== b.deleted) return a.deleted ? 1 : -1;
    if (a.deleted) {
      // Within the deleted group, most-recent first so the user's eye lands
      // on the trigger they were most likely running last.
      const aTime = a.lastActivity ?? '';
      const bTime = b.lastActivity ?? '';
      if (aTime !== bTime) return bTime.localeCompare(aTime);
    }
    return a.label.localeCompare(b.label);
  });
});

export function toggleTriggerId(id: string): void {
  const next = new Set(selectedTriggerIds.value);
  if (next.has(id)) next.delete(id); else next.add(id);
  setSelectedTriggerIds(next);
}

/** Tri-state click handler for the Trigger parent row. Indeterminate clears
 *  per-trigger selection (= "all triggers"); fully checked turns the channel
 *  off and clears selection; unchecked turns the channel on. With a single
 *  trigger, "all" and "just this one" are identical results, so the
 *  indeterminate state is meaningless and the click toggles the channel. */
export function toggleTriggerChannel(): void {
  const channelOn = threadChannelFilter.value.has('trigger');
  const hasSelection = selectedTriggerIds.value.size > 0;
  const lockstep = triggerFilterOptions.value.length === 1;
  if (channelOn && hasSelection && !lockstep) {
    setSelectedTriggerIds(new Set());
    return;
  }
  if (channelOn) setSelectedTriggerIds(new Set());
  toggleChannel('trigger');
}
