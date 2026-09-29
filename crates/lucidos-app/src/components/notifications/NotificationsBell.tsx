import { unreadCount } from '../../store/store';
import { switchMenuItem } from '../../store/actions/menu';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { BellIcon } from '../shared/icons';

export function NotificationsBell() {
  const count = unreadCount.value;

  return (
    <button
      class="icon-btn header-icon notifications-bell"
      onClick={() => switchMenuItem('notifications')}
      data-tooltip={tooltipWithShortcut('View notifications', 'openNotifications')}
      aria-label="View notifications"
    >
      <BellIcon />
      {count > 0 && (
        <span class="badge">
          {count > 999 ? '999+' : count}
        </span>
      )}
    </button>
  );
}
