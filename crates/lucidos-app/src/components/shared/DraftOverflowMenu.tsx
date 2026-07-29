import { OverflowMenu } from './OverflowMenu';
import { TrashIcon } from './icons';
import { discardDraft } from '../../store/actions/threadActions';
import { draftRowTooltip } from '../drawer/threadRowInfo';
import type { ComposeChannelMode } from '../../store/thread-events';
import type { Scope } from '../../store/store';

/** Per-draft overflow (⋯) menu for a compose draft row — a Delete action, with
 *  the Info row auto-appended by <OverflowMenu>. The draft counterpart of
 *  ThreadOverflowMenu, sharing the same <OverflowMenu> shell. It replaces the
 *  draft row's old hover/long-press tooltip: Delete drops the unsent draft
 *  (`discardDraft`, which confirms first), and Info surfaces the same structured
 *  details the tooltip used to (Status / Type / Created).
 *
 *  A draft hasn't bound its meta to a backend yet (see `sendCompose`), so its
 *  destination is read live from the draft's compose `mode` + `scope` — the row
 *  already resolves these for its chips, so they're passed in rather than
 *  re-derived here. */
export function DraftOverflowMenu({ threadId, mode, scope, contextName, createdAt, stopPropagation, extraClass, tabIndex }: {
  threadId: string;
  mode: ComposeChannelMode;
  scope: Scope;
  contextName: string | undefined;
  createdAt: string;
  stopPropagation?: boolean;
  extraClass?: string;
  tabIndex?: number;
}) {
  return (
    <OverflowMenu
      ariaLabel="More draft actions"
      stopPropagation={stopPropagation}
      extraClass={extraClass}
      tabIndex={tabIndex}
      infoRows={() => draftRowTooltip(mode, scope, contextName, createdAt)}
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
