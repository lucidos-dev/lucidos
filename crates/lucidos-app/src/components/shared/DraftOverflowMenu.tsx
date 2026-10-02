import { OverflowMenu, type HostOpener } from './OverflowMenu';
import { TrashIcon } from './icons';
import { discardDraft } from '../../store/actions/threadActions';

/** Per-draft overflow (⋯) menu for a compose draft row: a Delete action that
 *  drops the unsent draft (`discardDraft`, which confirms first). The draft
 *  counterpart of ThreadOverflowMenu, sharing the same <OverflowMenu> shell. */
export function DraftOverflowMenu({ threadId, stopPropagation, extraClass, tabIndex, hostOpener }: {
  threadId: string;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
  /** Lets the drawer row open this menu from its own gesture: a desktop
   *  right-click, or a mobile long press. See <OverflowMenu>. */
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
        // `discardDraft` confirms first and handles its own error/rollback toast,
        // so fire-and-forget (`void`) is safe here.
        <button type="button" class="thread-overflow-item thread-overflow-item-danger" role="menuitem"
          onClick={run(() => { void discardDraft(threadId); })}>
          <TrashIcon />
          Delete draft
        </button>
      )}
    />
  );
}
