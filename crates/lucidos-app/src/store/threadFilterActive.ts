import { computed } from '@preact/signals';
import {
  threadChannelFilter,
  ALL_CHANNELS,
  CODING_AGENT_CHANNEL,
  selectedTriggerIds,
  selectedRepoIds,
  selectedAppIds,
} from './store';
import { triggerFilterOptionsAll } from './triggerFilters';

export const threadFilterActive = computed<boolean>(() => {
  if (threadChannelFilter.value.size < ALL_CHANNELS.length) return true;

  if (threadChannelFilter.value.has('trigger')) {
    const selected = selectedTriggerIds.value.size;
    const total = triggerFilterOptionsAll.value.length;
    if (selected > 0 && (total === 0 || selected < total)) return true;
  }

  if (threadChannelFilter.value.has(CODING_AGENT_CHANNEL)) {
    if (selectedRepoIds.value.size > 0 || selectedAppIds.value.size > 0) return true;
  }

  return false;
});

