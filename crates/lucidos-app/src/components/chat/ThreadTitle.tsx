import { ThreadStatusIcon } from '../shared/ThreadStatusIcon';
import type { VisualStatus } from '../shared/threadVisualStatus';
import { PinThreadButton } from '../shared/PinThreadButton';
import { threadIsPinnable } from '../../store/actions/threadActions';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { revealThreadInFolders } from '../drawer/ThreadDrawer';
import { isElementVisible } from './scrollState';
import { openFind } from '../../store/actions/find-bar';
import type { ThreadState } from '../../store/thread-events';

/** The desktop title row's pin, beside the title's menu button, where a pin
 *  can do anything (`threadIsPinnable`). The mobile row draws no pin; its menu
 *  carries Pin. */
export function ThreadTitlePin({ thread }: { thread: ThreadState }) {
  const { id, saved } = thread.meta;
  if (!threadIsPinnable(thread.meta)) return null;
  return (
    <span class="thread-view-header-actions">
      <PinThreadButton threadId={id} saved={saved} />
    </span>
  );
}

/** The focused thread's title, drawn as its own menu button on both title
 *  rows. A click toggles the thread menu; a hold or a right-click opens it.
 *  The title never edits in place: Rename… lives in that menu (ADR 0331). */
export function ThreadTitleMenu({ thread, title, status }: {
  thread: ThreadState;
  title: string;
  /** Drawn inline before the title's first word, as a drawer row draws it. */
  status: VisualStatus | null;
}) {
  const { id, state, home } = thread.meta;
  // No thread list draws Home, so there is no row to show it in.
  const listed = state !== 'composing' && !home;
  return (
    <ThreadOverflowMenu threadId={id} title={title}
      onShowInFolders={listed ? () => revealThreadInFolders(id) : undefined}
      onFindInThread={state !== 'composing' ? () => openFind('thread') : undefined}
      face={{
        class: 'thread-title thread-title-menu',
        children: <span class="thread-title-text"><ThreadStatusIcon status={status} />{title}</span>,
      }} />
  );
}

/** Open the focused thread's menu by pressing the visible title. Both title
 *  rows are mounted and CSS shows one, so a hidden copy is skipped. */
export function openThreadTitleMenu(): void {
  for (const button of document.querySelectorAll<HTMLElement>('.thread-title-menu')) {
    if (isElementVisible(button)) {
      button.click();
      return;
    }
  }
}
