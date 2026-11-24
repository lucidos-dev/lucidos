
import { useSignal } from '@preact/signals';
import { peerWorkspaces, remotePeerWorkspaces } from '../../store/actions/app-badge';
import { switchMenuItem } from '../../store/actions/menu';
import { showToast, unreadCount, visibleWorkspaceName } from '../../store/store';
import { REMOTE_GATEWAY, WORKSPACE_ID } from '../../utils/basePath';
import { notifyRows, type NotifyRow, type NotifyRowsInput } from '../../utils/notificationRows';
export { notifyRows } from '../../utils/notificationRows';
export type { NotifyRow } from '../../utils/notificationRows';
import { errorDetail } from '../../utils/errorDetail';
import {
  alternateOpenMode,
  middleClickActivates,
  middleClickHandler,
  openModeForClick,
  openWorkspaceIn,
  type WorkspaceOpenMode,
} from '../../utils/workspaceWindow';
import { BellIcon } from '../shared/icons';
import { workspaceActionRow } from './WorkspaceActionRow';
import { GlyphBadge } from '../shared/GlyphBadge';
import { countLabel } from './unreadCountLabel';

const NOTIFY_ACTION_INDENT = 'brand-menu-ws-action-under-icon';

export interface NotificationsGroupProps extends NotifyRowsInput {
  onOpenOwn: () => void;
  onActivatePeer: (row: NotifyRow, e: MouseEvent) => void;
  contextId: string | null;
  onContext: (id: string | null) => void;
  onAlternate: (row: NotifyRow, mode: WorkspaceOpenMode) => void;
}

function rowAlternate(row: NotifyRow, currentId: string | null): WorkspaceOpenMode | null {
  if (row.isSelf || row.id === null) return null;
  return alternateOpenMode(row.state, currentId, row.id);
}

function notifyRow(row: NotifyRow, props: NotificationsGroupProps) {
  const label = row.count === 1
    ? `1 unread notification in ${row.name}`
    : `${row.count} unread notifications in ${row.name}`;
  const activate = (e: MouseEvent) => {
    if (row.isSelf) props.onOpenOwn();
    else props.onActivatePeer(row, e);
  };
  const aux = !row.isSelf && middleClickActivates(row.state)
    ? { onAuxClick: middleClickHandler(activate) }
    : {};
  return (
    <button
      type="button"
      class="brand-menu-item brand-menu-notif-row"
      role="menuitem"
      aria-label={label}
      onClick={activate}
      {...aux}
      onContextMenu={(e: MouseEvent) => { e.preventDefault(); props.onContext(row.id); }}
      key={`notif:${row.id ?? ''}`}
    >
      <BellIcon />
      <span class="brand-menu-ws-name">{row.name}</span>
      <GlyphBadge class="brand-menu-ws-badge" aria-hidden="true">{countLabel(row.count)}</GlyphBadge>
    </button>
  );
}

export function notificationsMenuGroup(props: NotificationsGroupProps) {
  const rows = notifyRows(props);
  if (rows.length === 0) return null;
  return (
    <>
      <div class="brand-menu-notif-group" role="group" aria-label="Unread notifications">
        {rows.flatMap((row) => {
          const mode = rowAlternate(row, props.ownId);
          return [
            notifyRow(row, props),
            row.id !== null && workspaceActionRow({
              open: row.id === props.contextId,
              id: row.id,
              name: row.name,
              mode,
              indentClass: NOTIFY_ACTION_INDENT,
              onActivate: (m) => props.onAlternate(row, m),
            }),
          ];
        })}
      </div>
      <div class="brand-menu-separator" role="separator" />
    </>
  );
}

export function NotificationsMenuGroup({ onClose }: { onClose: () => void }) {
  const contextId = useSignal<string | null>(null);

  const open = (row: NotifyRow, mode: WorkspaceOpenMode) => {
    contextId.value = null;
    onClose();
    if (!row.id) return;
    void openWorkspaceIn(mode, row.id, 'notifications').catch((e: unknown) => {
      showToast(`Could not open ${row.name}: ${errorDetail(e)}`, 'error');
    });
  };

  return notificationsMenuGroup({
    peers: peerWorkspaces.value,
    remotePeers: remotePeerWorkspaces.value,
    ownUnread: unreadCount.value,
    ownId: WORKSPACE_ID,
    ownName: visibleWorkspaceName.value
      ? visibleWorkspaceName.value + (REMOTE_GATEWAY ? ` @${REMOTE_GATEWAY}` : '')
      : null,
    onOpenOwn: () => { onClose(); switchMenuItem('notifications'); },
    onActivatePeer: (row, e) => {
      const mode = openModeForClick(e);
      if (mode !== null) open(row, mode);
    },
    contextId: contextId.value,
    onContext: (id) => { contextId.value = id; },
    onAlternate: open,
  });
}
