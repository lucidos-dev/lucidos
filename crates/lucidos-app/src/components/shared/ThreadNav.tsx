import {
  canGoBackThread, canGoForwardThread, threadNavBack, threadNavForward,
  threadNavHistory, threadNavGoTo,
} from '../../store/actions/thread-navigation';
import { focusedThreadId, threadMap } from '../../store/store';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { NavChevron, type NavHistoryItem } from './NavChevron';
import { ThreadTypeIcon } from './ThreadTypeIcon';

interface Props {
  showTooltip?: boolean;
}

/** A history row for one remembered thread — its title + a type glyph
 *  (Lucidos / Claude / Codex / trigger), derived from the live thread meta. */
function threadNavItem(index: number, id: string): NavHistoryItem {
  const thread = threadMap.value.get(id);
  return {
    key: `${index}:${id}`,
    label: thread ? threadDisplayTitle(thread) : 'Untitled Thread',
    // `'unknown'`, not a plausible `'chat'`: a row whose thread has left the
    // map has no known channel. `ThreadTypeIcon` draws the Lucidos mark for
    // both, so the honest value costs nothing.
    icon: <ThreadTypeIcon channel={thread?.meta.channel ?? 'unknown'} codingAgent={thread?.meta.codingAgent} />,
    onSelect: () => threadNavGoTo(index),
  };
}

function threadBackItems(): NavHistoryItem[] {
  const { stack, cursor } = threadNavHistory.value;
  // Unfocused (Compose / Esc): the cursor still points at the thread the user
  // left and the first Back restores it — so include the cursor entry at the
  // top. Focused: the cursor IS the current thread, so Back starts one earlier.
  const top = focusedThreadId.value === null ? cursor : cursor - 1;
  const items: NavHistoryItem[] = [];
  for (let i = top; i >= 0; i--) items.push(threadNavItem(i, stack[i].id));
  return items;
}

function threadForwardItems(): NavHistoryItem[] {
  const { stack, cursor } = threadNavHistory.value;
  const items: NavHistoryItem[] = [];
  for (let i = cursor + 1; i < stack.length; i++) items.push(threadNavItem(i, stack[i].id));
  return items;
}

/** Back and forward as separate exports, because the mobile thread header puts
 *  the Lucidos mark BETWEEN them (see BrandMenuButton) rather than rendering
 *  them as an adjacent pair. Same split, and the same reason, as
 *  `ContentBackButton` / `ContentForwardButton` in ContentNav. */
export function ThreadBackButton({ showTooltip }: Props) {
  return (
    <NavChevron
      direction="back"
      buttonClass="thread-nav-btn"
      disabled={!canGoBackThread.value}
      onStep={() => threadNavBack()}
      getItems={threadBackItems}
      ariaLabel="Previous thread"
      tooltip={showTooltip ? tooltipWithShortcut('Previous thread', 'historyBack') : undefined}
    />
  );
}

export function ThreadForwardButton({ showTooltip }: Props) {
  return (
    <NavChevron
      direction="forward"
      buttonClass="thread-nav-btn"
      disabled={!canGoForwardThread.value}
      onStep={() => threadNavForward()}
      getItems={threadForwardItems}
      ariaLabel="Next thread"
      tooltip={showTooltip ? tooltipWithShortcut('Next thread', 'historyForward') : undefined}
    />
  );
}
