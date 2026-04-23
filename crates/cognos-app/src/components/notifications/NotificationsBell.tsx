import { unreadCount } from '../../store/store';
import { switchMenuItem } from '../../store/actions/menu';
import { BellIcon } from '../shared/icons';

export function NotificationsBell() {
  const count = unreadCount.value;

  return (
    <div class="icon-btn header-icon notifications-bell" onClick={() => switchMenuItem('notifications')} data-tooltip="View notifications">
      <BellIcon />
      {count > 0 && (
        <span class="badge">
          {count > 999 ? '999+' : count}
        </span>
      )}
    </div>
  );
}
