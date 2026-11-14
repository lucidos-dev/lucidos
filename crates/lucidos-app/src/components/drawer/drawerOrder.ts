/** The drawer list on screen, in the order the drawer renders it. The archive
 *  and drop hand-offs walk it to pick the next focus. It ships in the shell
 *  chunk with the family graph, and the entry reads it through
 *  `store/drawerOrderHandle.ts`. */
import { threadMap, drawerGrouping, selectedOngoingGroup, threadSearchQuery, threadSearchResults } from '../../store/store';
import { appliedThreadFilter } from '../../store/appliedThreadFilter';
import { threadPassesChannelFilter } from '../../store/threadFilter';
import type { ThreadState } from '../../store/thread-events';
import { computeFamilyGraph, filterByTopThread, ongoingFilter, orderedCurrentForReview, selectedOngoingRows } from './family-graph';
import type { FamilyGraph } from './family-graph';

/** Threads the user can actually see, given the active channel/trigger/repo/app
 *  filter: the SAME family-scoped predicate the drawer uses (`ThreadDrawer`'s
 *  ThreadList). The post-archive focus must land on a row the user can click,
 *  so a thread the filter hides is never the next focus. Returns the filtered
 *  set plus the family graph (built over the full thread map so parent walks
 *  resolve) for the caller to order. */
function visibleThreadsAndGraph(): { visible: ThreadState[]; graph: FamilyGraph } {
  const all = Array.from(threadMap.value.values());
  const graph = computeFamilyGraph(all);
  // The *applied* selection, which is what the drawer is showing. While the
  // thread filter panel is up it holds still (see `appliedThreadFilter`).
  const { channels: filter, triggerIds: triggerSelection, repoIds: repoSelection, appIds: appSelection } = appliedThreadFilter.value;
  // A trigger/repo/app sub-selection flips the gate to any-member matching, as
  // the drawer does. A coding-agent thread in the selected repo or app then
  // surfaces with its family, even when the root's channel is filtered out.
  const subSelectionActive =
    triggerSelection.size > 0 || repoSelection.size > 0 || appSelection.size > 0;
  const visible = filterByTopThread(all, graph,
    t => threadPassesChannelFilter(t, filter, triggerSelection, repoSelection, appSelection),
    subSelectionActive,
  );
  return { visible, graph };
}

/** The visible thread ids of the drawer list the user is *currently looking
 *  at*, in the same order the drawer renders them. The post-archive focus
 *  walks this, so "next" is the next visible row of the list on screen: the
 *  selected ongoing group's listed rows under Ongoing, the filtered Current rows under
 *  Folders. Mirrors `ThreadDrawer`'s `activeView` resolution: a live search
 *  query overrides the grouping.
 *
 *  The Ongoing grouping deliberately bypasses the channel/trigger/repo/app
 *  filter, exactly as the drawer renders it. Only Folders is filter-aware,
 *  walking the visible Current section (`orderedCurrentForReview`) the same way
 *  the drawer does. */
export function orderedVisibleThreadIds(): string[] {
  // Search overrides the grouping (mirrors ThreadDrawer's activeView).
  if (threadSearchQuery.value.trim().length > 0) {
    const results = threadSearchResults.value;
    return results.status === 'loaded' ? results.data.map(r => r.thread_id) : [];
  }
  if (drawerGrouping.value === 'ongoing') return selectedOngoingRows.value.map(n => n.thread.meta.id);
  const { visible, graph } = visibleThreadsAndGraph();
  return orderedCurrentForReview(visible, graph).map(t => t.meta.id);
}

/** Which drawer list `orderedVisibleThreadIds` walks: a live search, the selected
 *  ongoing group, or Folders. */
export function activeViewKey(): string {
  const query = threadSearchQuery.value.trim();
  if (query.length > 0) return `search:${query}`;
  return drawerGrouping.value === 'ongoing' ? `ongoing:${selectedOngoingGroup.value}:${ongoingFilter.value}` : 'folders';
}
