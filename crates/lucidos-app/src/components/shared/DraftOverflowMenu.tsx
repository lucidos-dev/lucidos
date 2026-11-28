import { OverflowMenu, type HostOpener } from './OverflowMenu';
import { TrashIcon } from './icons';
import { discardDraft } from '../../store/actions/threadActions';

export function DraftOverflowMenu({ threadId, stopPropagation, extraClass, tabIndex, hostOpener }: {
  threadId: string;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
  hostOpener?: HostOpener;
}) {
  return (
    <OverflowMenu
      ariaLabel="More draft actions"
      stopPropagation={stopPropagation}
      extraClass={extraClass}
      tabIndex={tabIndex}
      hostOpener={hostOpener}
      items={({ run }) => (
        <button type="button" class="thread-overflow-item thread-overflow-item-danger" role="menuitem"
          onClick={run(() => { void discardDraft(threadId); })}>
          <TrashIcon />
          Delete draft
        </button>
      )}
    />
  );
}
