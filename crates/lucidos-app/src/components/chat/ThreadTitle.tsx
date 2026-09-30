import { ThreadStatusIcon, type VisualStatus } from '../shared/ThreadStatusIcon';
import { PinThreadButton } from '../shared/PinThreadButton';
import { ThreadOverflowMenu } from '../shared/ThreadOverflowMenu';
import { revealThreadInList } from '../drawer/ThreadDrawer';
import type { ThreadState } from '../../store/thread-events';

/** The desktop title row's pin, beside the title's menu button. A draft has
 *  nothing to pin. The mobile row draws no pin; its menu carries Pin. */
export function ThreadTitlePin({ thread }: { thread: ThreadState }) {
  const { id, state, saved } = thread.meta;
  if (state === 'composing') return null;
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
  const { id, state } = thread.meta;
  return (
    <ThreadOverflowMenu threadId={id} title={title}
      onShowInThreadList={state === 'composing' ? undefined : () => revealThreadInList(id)}
      face={{
        class: 'thread-title thread-title-menu',
        children: <><ThreadStatusIcon status={status} />{title}</>,
      }} />
  );
}
