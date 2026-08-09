import { computed } from '@preact/signals';
import {
  threadChannelFilter,
  ALL_CHANNELS,
  CODING_AGENT_CHANNEL,
  selectedTriggerIds,
  selectedRepoIds,
  selectedAppIds,
} from './store';
import { triggerFilterOptions, triggerFilterOptionsAll } from './triggerFilters';
import { repoFilterOptionsAll } from './repoFilters';
import { appFilterOptionsAll } from './appFilters';
import { hasHiddenDeleted } from './deletedFilterOptions';

/** `total === 0` is the registry-loading window: the localStorage-restored
 *  selection IS narrowing the backend query, so a non-empty selection
 *  counts as active even though we don't yet know the universe. Once
 *  loaded, an explicit "all" selection (size === total) excludes nothing
 *  and stays neutral. */
export const threadFilterActive = computed<boolean>(() => {
  if (threadChannelFilter.value.size < ALL_CHANNELS.length) return true;

  if (threadChannelFilter.value.has('trigger')) {
    const selected = selectedTriggerIds.value.size;
    const total = triggerFilterOptions.value.length;
    if (selected > 0 && (total === 0 || selected < total)) return true;
  }

  if (threadChannelFilter.value.has(CODING_AGENT_CHANNEL)) {
    // Coding Agent has two cross-axis sub-selections (repos + apps). When either is
    // non-empty, the drawer is narrowing: even "select every app"
    // (selectedApps === totalApps) excludes Lucidos-source and external-repo
    // coding-agent threads, because threadPassesChannelFilter unions appOk || repoOk
    // and a non-app thread fails appOk. The per-axis "selected === total"
    // shortcut from triggers (where one axis = one universe) doesn't
    // generalize here; report active whenever any coding-agent sub-selection exists.
    if (selectedRepoIds.value.size > 0 || selectedAppIds.value.size > 0) return true;
  }

  return false;
});

/** Whether "Include deleted" being off is actually holding an option back right
 *  now: a deleted trigger, repo or app that exists and is not selected.
 *
 *  The distinction the filter panel's "(filtered)" note is built on, and the
 *  same one `threadFilterActive` draws for the channels: the question is never
 *  whether a setting is at its widest, but whether it EXCLUDES anything. A
 *  workspace that has never deleted a trigger, repo or app has nothing to
 *  include, so the switch being off narrows nothing and the panel must not say
 *  the list is filtered.
 *
 *  False while the registries are still loading, since the `*All` lists return
 *  [] until then: the honest answer at that moment is "nothing known to hide". */
export const deletedOptionsHidden = computed<boolean>(() =>
  hasHiddenDeleted(triggerFilterOptionsAll.value, selectedTriggerIds.value)
  || hasHiddenDeleted(repoFilterOptionsAll.value, selectedRepoIds.value)
  || hasHiddenDeleted(appFilterOptionsAll.value, selectedAppIds.value));
