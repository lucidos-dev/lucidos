import type { App } from '../../store/types';
import { isAppPinned, togglePinApp } from '../../store/actions/pinnedApps';
import { EditIcon, TrashIcon } from '../shared/icons';
import { AppIcon } from '../shared/AppIcon';
import { OverflowMenu, type OverflowMenuContext } from '../shared/OverflowMenu';
import { PinButton } from '../shared/PinButton';
import { useRowActionsGesture } from '../shared/useRowActionsGesture';
import { useSkeleton, SkText, SkBlock } from '../shared/Skeleton';

interface AppRowProps {
  app: App;
  onOpen: () => void;
  onEdit: () => void;
  onDelete: () => void;
}

/** Self-skeletonizing row: rendered with no props inside a SkeletonProvider
 *  (`<AppRow />`) it draws itself as a loading placeholder via the Sk* leaves;
 *  with real props it renders normally. Props are optional only to support the
 *  skeleton call; real call sites pass them all.
 *
 *  The row's actions work as a thread drawer row's do. The pin toggle is always
 *  drawn and shows the pinned state. Edit and Delete sit in the row's menu.
 *  A desktop row opens it from its ⋯ or a right-click, and a phone row from a
 *  long press (`useRowActionsGesture`). Marketplace and update state live in
 *  the Plugins panel, so the row stays calm. */
export function AppRow({ app, onOpen, onEdit, onDelete }: Partial<AppRowProps>) {
  const sk = useSkeleton();
  const gesture = useRowActionsGesture({ onTap: onOpen, enabled: !sk });
  const pinned = !!app && isAppPinned(app.id);
  return (
    <div class={`list-row app-row${sk ? '' : ' clickable'}`} {...gesture.handlers}>
      <div class="app-row-icon">
        <SkBlock w="var(--app-icon-size)" h="var(--app-icon-size)">
          {app && <AppIcon appId={app.id} name={app.name} icon={app.icon} />}
        </SkBlock>
      </div>
      <div class="list-row-info">
        <div class="app-row-title-line">
          <SkText class="title list-row-name" w="9rem">{app?.name}</SkText>
        </div>
        {(sk || app?.description) && (
          <SkText class="app-row-description" as="div" w="16rem">{app?.description}</SkText>
        )}
      </div>
      <div class="list-row-actions">
        <SkBlock w="2.25rem" h="2.25rem" round>
          {app && (
            <PinButton
              pinned={pinned}
              label={`${pinned ? 'Unpin' : 'Pin'} ${app.name}`}
              tooltip={pinned ? 'Pinned to the menu. Click to unpin' : 'Pin to the menu'}
              onToggle={() => togglePinApp(app.id)}
              stopPropagation
            />
          )}
        </SkBlock>
        {app && (
          <OverflowMenu
            ariaLabel={`Actions for ${app.name}`}
            stopPropagation
            hostOpener={gesture.hostOpener}
            items={(ctx) => appMenuItems(ctx, { onEdit, onDelete })}
          />
        )}
      </div>
    </div>
  );
}

/** Edit, then Delete on its own below a divider, as in the thread menu. The
 *  row's own pin toggle covers pinning, so the menu does not repeat it. */
function appMenuItems(
  { run }: OverflowMenuContext,
  { onEdit, onDelete }: Pick<Partial<AppRowProps>, 'onEdit' | 'onDelete'>,
) {
  return (
    <>
      <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => onEdit?.())}>
        <EditIcon />
        Edit
      </button>
      <div class="thread-overflow-divider" role="separator" />
      <button type="button" class="thread-overflow-item thread-overflow-item-danger" role="menuitem" onClick={run(() => onDelete?.())}>
        <TrashIcon />
        Delete
      </button>
    </>
  );
}
