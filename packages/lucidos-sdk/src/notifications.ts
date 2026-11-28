import { request, requestVoid } from './_fetch';
import { assertPlainObject } from './_validate';
import type {
  CodingAgentTarget,
  NavigateTarget,
  SettingsViewTarget,
} from './generated/navigate-targets';

export type { CodingAgentTarget, NavigateTarget, SettingsViewTarget };

export interface NavigateUi {
  target: NavigateTarget;
  settings_view?: SettingsViewTarget;
  app_id?: string;
  fragment?: string;
  file_path?: string;
  line?: number;
  line_end?: number;
  id?: string;
  url?: string;
  event_id?: string;
  prompt?: string;
  coding_agent?: CodingAgentTarget;
  folder?: string;
}

export type Tap =
  | { kind: 'modal' }
  | { kind: 'restore-snapshot'; workspace_id: string; handle: string }
  | { kind: 'undo-config-change'; event_id: string }
  | { kind: 'navigate'; to: NavigateUi };

export interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  thread_id?: string;
  event_id?: string;
  title: string;
  message: string;
  created_at: string;
  read: boolean;
  tap?: Tap;
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
    return request(`/notifications${q ? `?${q}` : ''}`);
  },

  markRead(id: string): Promise<void> {
    return requestVoid(`/notification/read?id=${encodeURIComponent(id)}`, {
      method: 'POST',
    });
  },

  markAllRead(): Promise<void> {
    return requestVoid('/notifications/read-all', { method: 'POST' });
  },
};
