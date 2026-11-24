import {
  canGoBackThread, canGoForwardThread, threadNavBack, threadNavForward,
  threadNavHistory, threadNavGoTo, type ThreadNavEntry,
} from '../../store/actions/thread-navigation';
import { focusedThreadId, threadMap } from '../../store/store';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { NavChevron, type NavHistoryItem } from './NavChevron';
import { DashboardIcon } from './icons';
import { ThreadTypeIcon } from './ThreadTypeIcon';

interface Props {
  showTooltip?: boolean;
}

function threadNavItem(index: number, { type, id }: ThreadNavEntry): NavHistoryItem {
  const thread = threadMap.value.get(id);
  const title = thread ? threadDisplayTitle(thread) : 'Untitled Thread';
  if (type === 'dashboard') {
    return { key: `${index}:dashboard`, label: `Dashboard \u00b7 ${title}`, icon: <DashboardIcon />, onSelect: () => threadNavGoTo(index) };
  }
  return {
    key: `${index}:${id}`,
    label: title,
    icon: <ThreadTypeIcon channel={thread?.meta.channel ?? 'unknown'} codingAgent={thread?.meta.codingAgent} />,
    onSelect: () => threadNavGoTo(index),
  };
}

function threadBackItems(): NavHistoryItem[] {
  const { stack, cursor } = threadNavHistory.value;
  const top = focusedThreadId.value === null ? cursor : cursor - 1;
  const items: NavHistoryItem[] = [];
  for (let i = top; i >= 0; i--) items.push(threadNavItem(i, stack[i]));
  return items;
}

function threadForwardItems(): NavHistoryItem[] {
  const { stack, cursor } = threadNavHistory.value;
  const items: NavHistoryItem[] = [];
  for (let i = cursor + 1; i < stack.length; i++) items.push(threadNavItem(i, stack[i]));
  return items;
}

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
