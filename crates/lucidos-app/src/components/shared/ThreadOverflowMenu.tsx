import { OverflowMenu, type HostOpener, type TriggerFace } from './OverflowMenu';
import { lazyComponent } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

export const ThreadOverflowItemsChunk = lazyComponent(() => import('./ThreadOverflowItems').then((m) => m.ThreadOverflowItems));
prefetchWhenIdle(ThreadOverflowItemsChunk);

export function ThreadOverflowMenu({ threadId, title, onShowInFolders, onFindInThread, stopPropagation, extraClass, tabIndex, hostOpener, face }: {
  threadId: string;
  title: string;
  onShowInFolders?: () => void;
  onFindInThread?: () => void;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
  hostOpener?: HostOpener;
  face?: TriggerFace;
}) {
  const opening = face ? { face } : { hostOpener };
  return (
    <OverflowMenu
      ariaLabel="More thread actions"
      stopPropagation={stopPropagation}
      extraClass={extraClass}
      tabIndex={tabIndex}
      {...opening}
      itemsChunk={ThreadOverflowItemsChunk}
      items={({ run }) => (
        <ThreadOverflowItemsChunk threadId={threadId} title={title} run={run}
          onShowInFolders={onShowInFolders} onFindInThread={onFindInThread} />
      )}
    />
  );
}
