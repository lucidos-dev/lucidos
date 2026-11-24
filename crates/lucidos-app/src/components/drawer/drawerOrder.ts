import { threadMap, drawerGrouping, selectedOngoingGroup, threadSearchQuery, threadSearchResults } from '../../store/store';
import { appliedThreadFilter } from '../../store/appliedThreadFilter';
import { threadPassesChannelFilter } from '../../store/threadFilter';
import type { ThreadState } from '../../store/thread-events';
import { computeFamilyGraph, filterByTopThread, ongoingFilter, orderedCurrentForReview, selectedOngoingRows } from './family-graph';
import type { FamilyGraph } from './family-graph';

function visibleThreadsAndGraph(): { visible: ThreadState[]; graph: FamilyGraph } {
  const all = Array.from(threadMap.value.values());
  const graph = computeFamilyGraph(all);
  const { channels: filter, triggerIds: triggerSelection, repoIds: repoSelection, appIds: appSelection } = appliedThreadFilter.value;
  const subSelectionActive =
    triggerSelection.size > 0 || repoSelection.size > 0 || appSelection.size > 0;
  const visible = filterByTopThread(all, graph,
    t => threadPassesChannelFilter(t, filter, triggerSelection, repoSelection, appSelection),
    subSelectionActive,
  );
  return { visible, graph };
}

export function orderedVisibleThreadIds(): string[] {
  if (threadSearchQuery.value.trim().length > 0) {
    const results = threadSearchResults.value;
    return results.status === 'loaded' ? results.data.map(r => r.thread_id) : [];
  }
  if (drawerGrouping.value === 'ongoing') return selectedOngoingRows.value.map(n => n.thread.meta.id);
  const { visible, graph } = visibleThreadsAndGraph();
  return orderedCurrentForReview(visible, graph).map(t => t.meta.id);
}

export function activeViewKey(): string {
  const query = threadSearchQuery.value.trim();
  if (query.length > 0) return `search:${query}`;
  return drawerGrouping.value === 'ongoing' ? `ongoing:${selectedOngoingGroup.value}:${ongoingFilter.value}` : 'folders';
}
