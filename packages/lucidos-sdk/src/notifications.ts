import { request, requestVoid } from './_fetch';
import { assertPlainObject } from './_validate';

export interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  title: string;
  message: string;
  created_at: string;
  read: boolean;
}

export interface NotificationListResult {
  notifications: Notification[];
  unread_count: number;
  has_more: boolean;
}

export const notifications = {
  list(params?: { limit?: number; before?: number; filter?: string }): Promise<NotificationListResult> {
    if (params !== undefined) assertPlainObject('params', params);
    const qs = new URLSearchParams();
    if (params?.limit != null) qs.set('limit', String(params.limit));
    if (params?.before != null) qs.set('before', String(params.before));
    if (params?.filter) qs.set('filter', params.filter);
    const q = qs.toString();
    return request(`/api/notifications${q ? `?${q}` : ''}`);
  },

  markRead(id: string): Promise<void> {
    return requestVoid(`/api/notification/read?id=${encodeURIComponent(id)}`, {
      method: 'POST',
    });
  },

  markAllRead(): Promise<void> {
    return requestVoid('/api/notifications/read-all', { method: 'POST' });
  },
};
