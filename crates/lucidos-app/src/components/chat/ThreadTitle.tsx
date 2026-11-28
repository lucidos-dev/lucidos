import { ThreadStatusIcon } from '../shared/ThreadStatusIcon';
import { ThreadIcon } from '../shared/ThreadIcon';
import type { VisualStatus } from '../shared/threadVisualStatus';
import { PinThreadButton } from '../shared/PinThreadButton';
import { threadIsPinnable } from '../../store/actions/threadActions';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { revealThreadInFolders } from '../drawer/ThreadDrawer';
import { isElementVisible } from '../../utils/elementVisibility';
import { openFind } from '../../store/actions/find-bar';
import type { ThreadState } from '../../store/thread-events';

export function ThreadTitlePin({ thread }: { thread: ThreadState }) {
  const { id, saved } = thread.meta;
  if (!threadIsPinnable(thread.meta)) return null;
  return (
    <span class="thread-view-header-actions">
      <PinThreadButton threadId={id} saved={saved} />
    </span>
  );
}

export function ThreadTitleMenu({ thread, title, status }: {
  thread: ThreadState;
  title: string;
  status: VisualStatus | null;
}) {
  const { id, state, home } = thread.meta;
  const listed = state !== 'composing' && !home;
  return (
    <ThreadOverflowMenu threadId={id} title={title}
      onShowInFolders={listed ? () => revealThreadInFolders(id) : undefined}
      onFindInThread={state !== 'composing' ? () => openFind('thread') : undefined}
      face={{
        class: 'thread-title thread-title-menu',
        children: <span class="thread-title-text"><ThreadStatusIcon status={status} /><ThreadIcon home={home === true} />{title}</span>,
      }} />
  );
}

export function openThreadTitleMenu(): void {
  for (const button of document.querySelectorAll<HTMLElement>('.thread-title-menu')) {
    if (isElementVisible(button)) {
      button.click();
      return;
    }
  }
}
