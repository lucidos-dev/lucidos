
import type { Tap } from '@lucidos/sdk';
import { aliasRetiredSettingsSubview } from '../store';
import type { MenuItem } from '../types';
import { resolveFileTarget } from './fileTarget';

export const threadVisitKey = (id: string): string => `thread:${id}`;
export const appVisitKey = (appId: string): string => `app:${appId}`;
export const fileVisitKey = (path: string): string => `file:${path}`;
export const triggerVisitKey = (id: string): string => `trigger:${id}`;
export const settingsVisitKey = (view: string): string => `settings:${view}`;
export const panelVisitKey = (item: MenuItem): string => `panel:${item}`;

export interface NavigateLike {
  target: string;
  settings_view?: string;
  app_id?: string;
  file_path?: string;
  id?: string;
  event_id?: string;
}

export function navigateVisitKey(nav: NavigateLike): string | null {
  switch (nav.target) {
    case 'thread':
      return nav.id ? threadVisitKey(nav.id) : null;
    case 'app':
    case 'app-ui':
      return nav.app_id ? appVisitKey(nav.app_id) : null;
    case 'file':
      return nav.file_path ? fileVisitKey(resolveFileTarget(nav.file_path).path) : null;
    case 'trigger':
      return nav.id ? triggerVisitKey(nav.id) : null;
    case 'settings':
      return settingsVisitKey(
        nav.settings_view ? aliasRetiredSettingsSubview(nav.settings_view) : 'main',
      );
    case 'thread-queue':
      return settingsVisitKey('thread-queue');
    case 'app-store':
    case 'plugins':
      return panelVisitKey('plugins');
    case 'files':
    case 'apps':
    case 'triggers':
    case 'changes':
    case 'notifications':
      return panelVisitKey(nav.target);
    default:
      return null;
  }
}

export type SeenTarget =
  | { kind: 'event'; threadId: string; eventId: string }
  | { kind: 'place'; key: string };

export function notificationTarget(tap: Tap | null | undefined): SeenTarget | null {
  if (!tap || tap.kind !== 'navigate') return null;
  const to = tap.to;
  if (to.target === 'thread' && to.id && to.event_id) {
    return { kind: 'event', threadId: to.id, eventId: to.event_id };
  }
  const key = navigateVisitKey(to);
  return key ? { kind: 'place', key } : null;
}
