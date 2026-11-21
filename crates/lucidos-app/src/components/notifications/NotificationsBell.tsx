import { unreadCount } from '../../store/store';
import { switchMenuItem } from '../../store/actions/menu';
import { tooltipWithShortcut } from '../../store/actions/keybindings';
import { BellIcon } from '../shared/icons';
import { GlyphBadge } from '../shared/GlyphBadge';

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
        <GlyphBadge class="badge">
          {count > 999 ? '999+' : count}
        </GlyphBadge>
      )}
    </button>
  );
}
