import type { Notification } from '../../store/types';

export interface NotificationPosition {
  hasNewer: boolean;
  hasOlder: boolean;
}

export function notificationPosition(
  items: Notification[],
  id: string,
  hasMore: boolean,
): NotificationPosition {
  const index = items.findIndex((n) => n.id === id);
  if (index < 0) return { hasNewer: false, hasOlder: false };
  return {
    hasNewer: index > 0,
    hasOlder: index < items.length - 1 || hasMore,
  };
}
