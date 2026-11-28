import { effect, signal } from '@preact/signals';
import {
  threadChannelFilter,
  selectedTriggerIds,
  selectedRepoIds,
  selectedAppIds,
  type ThreadChannel,
} from './store';
import { threadFilterPanelOpen } from './threadFilterPanel';

export type ThreadFilterSelection = {
  channels: ReadonlySet<ThreadChannel>;
  triggerIds: ReadonlySet<string>;
  repoIds: ReadonlySet<string>;
  appIds: ReadonlySet<string>;
};

function liveSelection(): ThreadFilterSelection {
  return {
    channels: threadChannelFilter.value,
    triggerIds: selectedTriggerIds.value,
    repoIds: selectedRepoIds.value,
    appIds: selectedAppIds.value,
  };
}

function sameIds<T>(a: ReadonlySet<T>, b: ReadonlySet<T>): boolean {
  if (a === b) return true;
  if (a.size !== b.size) return false;
  for (const id of a) {
    if (!b.has(id)) return false;
  }
  return true;
}

function sameSelection(a: ThreadFilterSelection, b: ThreadFilterSelection): boolean {
  return sameIds(a.channels, b.channels)
    && sameIds(a.triggerIds, b.triggerIds)
    && sameIds(a.repoIds, b.repoIds)
    && sameIds(a.appIds, b.appIds);
}

export const appliedThreadFilter = signal<ThreadFilterSelection>(liveSelection());

effect(() => {
  if (threadFilterPanelOpen.value) return;
  const live = liveSelection();
  if (sameSelection(appliedThreadFilter.peek(), live)) return;
  appliedThreadFilter.value = live;
});
