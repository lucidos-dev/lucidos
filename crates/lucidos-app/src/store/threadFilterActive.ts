import { computed } from '@preact/signals';
import {
  threadChannelFilter,
  ALL_CHANNELS,
  selectedTriggerIds,
  selectedRepoIds,
  selectedAppIds,
} from './store';
import { triggerFilterOptions } from './triggerFilters';

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

  if (threadChannelFilter.value.has('claude_code')) {
    // CC has two cross-axis sub-selections (repos + apps). When either is
    // non-empty, the drawer is narrowing: even "select every app"
    // (selectedApps === totalApps) excludes Lucidos-source and external-repo
    // CC threads, because threadPassesChannelFilter unions appOk || repoOk
    // and a non-app thread fails appOk. The per-axis "selected === total"
    // shortcut from triggers (where one axis = one universe) doesn't
    // generalize here; report active whenever any CC sub-selection exists.
    if (selectedRepoIds.value.size > 0 || selectedAppIds.value.size > 0) return true;
  }

  return false;
});
