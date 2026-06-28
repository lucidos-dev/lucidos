import { request, requestVoid } from './_fetch';
import { assertPlainObject } from './_validate';
import type { NavigateTarget, SettingsViewTarget } from './generated/navigate-targets';

/** The navigation contract is GENERATED from the engine's `navigate_ui` tool
 *  (the `NAVIGATE_TARGETS` / `NAVIGABLE_SETTINGS_VIEWS` consts in
 *  `crates/lucidos-engine/src/llm/tools/misc.rs`) into
 *  `./generated/navigate-targets.ts`, so the SDK and the LLM tool schema cannot
 *  drift. Re-exported here so consumers keep importing these from `@lucidos/sdk`.
 *  See system-knowhow/js-sdk.md § lucidos.notifications. */
export type { NavigateTarget, SettingsViewTarget };

/** Navigation payload — the same shape the `navigate_ui` LLM tool accepts.
 *  When `tap.kind === 'navigate'`, `tap.to` is this. Required sub-fields
 *  depend on `target`: `app` requires `app_id`, `file` requires `file_path`,
 *  `thread` requires `id` (and accepts optional `event_id` to scroll-and-pulse
 *  a specific event row on land), `trigger` requires `id`, `url` requires
 *  `url`. `settings_view` applies only when `target === 'settings'`. The
 *  page-side router enforces these. */
export interface NavigateUi {
  target: NavigateTarget;
  settings_view?: SettingsViewTarget;
  app_id?: string;
  file_path?: string;
  id?: string;
  url?: string;
  event_id?: string;
  prompt?: string;
}

/** What a notification tap does. `modal` opens the inbox modal showing the
 *  notification body. `none` marks the row read with no navigation (passive
 *  pushes — "OAuth completed"). `navigate` delegates to the same router the
 *  `navigate_ui` LLM tool uses; `to` is its arg shape. Every kind marks the
 *  source notification read on tap. */
export type Tap =
  | { kind: 'modal' }
  | { kind: 'none' }
  | { kind: 'navigate'; to: NavigateUi };

export interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  thread_id?: string;
  /** Specific event UUID inside `thread_id` that raised this notification —
   *  used by the §4 in-app matrix to silently mark-read when the user is
   *  looking at the source event. Distinct from `tap.to.event_id` (which
   *  controls the scroll-and-pulse target when the tap navigates). */
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
