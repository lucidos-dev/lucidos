import type { Tap, NavigateUi } from '@lucidos/sdk';
import { isWorkspaceSlug } from '../../utils/basePath';
import type { Notification } from '../types';
import { hasDangerousScheme } from '../../utils/dangerousUrlScheme';

export type DeepLinkAction =
  | { type: 'navigate'; to: NavigateUi; notification: string | null }
  | { type: 'restore-snapshot'; tap: Extract<Tap, { kind: 'restore-snapshot' }>; notification: string | null }
  | { type: 'undo-config-change'; tap: Extract<Tap, { kind: 'undo-config-change' }>; notification: string | null }
  | { type: 'view-notification'; id: string }
  | { type: 'noop' };

export interface DeepLinkTarget {
  notification?: string | null;
  restoreGeneration?: string;
  thread?: string | null;
  event?: string | null;
  tap?: Tap | null;
}

export function resolveDeepLink(target: DeepLinkTarget): DeepLinkAction {
  const tap: Tap = target.tap ?? { kind: 'modal' };
  if (tap.kind === 'navigate') {
    return { type: 'navigate', to: tap.to, notification: target.notification ?? null };
  }
  const validTap = validateTap(tap);
  if (validTap?.kind === 'restore-snapshot') {
    return { type: 'restore-snapshot', tap: validTap, notification: target.notification ?? null };
  }
  if (validTap?.kind === 'undo-config-change') {
    return { type: 'undo-config-change', tap: validTap, notification: target.notification ?? null };
  }
  if (target.notification) return { type: 'view-notification', id: target.notification };
  return { type: 'noop' };
}

const DEEP_LINK_KEYS = ['notification', 'thread', 'event', 'tap', 'restore_generation'] as const;

export function parseDeepLinkFromUrl(loc: URL | Location): DeepLinkTarget {
  const hashParams = loc.hash.startsWith('#')
    ? new URLSearchParams(loc.hash.slice(1))
    : new URLSearchParams();
  const queryParams = new URLSearchParams(loc.search);
  const get = (k: string) => hashParams.get(k) ?? queryParams.get(k);
  return {
    notification: get('notification'),
    restoreGeneration: get('restore_generation') ?? 'unknown',
    thread: get('thread'),
    event: get('event'),
    tap: parseTapFromUrlParam(get('tap')),
  };
}

function parseTapFromUrlParam(raw: string | null): Tap | null {
  if (!raw) return null;
  try {
    return validateTap(JSON.parse(raw));
  } catch {
    return null;
  }
}

export function hasDeepLinkParams(target: DeepLinkTarget): boolean {
  return !!target.notification;
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function validateTap(raw: unknown): Tap | null {
  if (!raw || typeof raw !== 'object') return null;
  const obj = raw as Record<string, unknown>;
  switch (obj.kind) {
    case 'restore-snapshot':
      if (typeof obj.workspace_id !== 'string' || !isWorkspaceSlug(obj.workspace_id)
        || typeof obj.handle !== 'string' || !/^snap_[0-7][0-9A-HJKMNP-TV-Z]{25}$/.test(obj.handle)) return null;
      return { kind: 'restore-snapshot', workspace_id: obj.workspace_id, handle: obj.handle };
    case 'undo-config-change':
      if (typeof obj.event_id !== 'string' || !UUID_RE.test(obj.event_id)) return null;
      return { kind: 'undo-config-change', event_id: obj.event_id };
    case 'modal':
      return { kind: 'modal' };
    case 'navigate': {
      const to = obj.to;
      if (!to || typeof to !== 'object') return null;
      const nav = to as Record<string, unknown>;
      if (typeof nav.target !== 'string') return null;
      if (typeof nav.url === 'string' && hasDangerousScheme(nav.url)) return null;
      return { kind: 'navigate', to: to as NavigateUi };
    }
    default:
      return null;
  }
}

export function parseDeepLinkFromSwMessage(raw: unknown): DeepLinkTarget | null {
  if (!raw || typeof raw !== 'object') return null;
  const obj = raw as Record<string, unknown>;
  return {
    restoreGeneration: obj.restore_generation === null ? 'none'
      : typeof obj.restore_generation === 'string' ? obj.restore_generation : 'unknown',
    notification: typeof obj.notification_id === 'string' ? obj.notification_id : null,
    thread: typeof obj.thread_id === 'string' ? obj.thread_id : null,
    event: typeof obj.event_id === 'string' ? obj.event_id : null,
    tap: validateTap(obj.tap),
  };
}

export function parseDeepLinkFromInboxRow(
  n: Pick<Notification, 'id' | 'thread_id' | 'event_id' | 'tap'>,
): DeepLinkTarget {
  return {
    notification: n.id,
    thread: n.thread_id ?? null,
    event: n.event_id ?? null,
    tap: n.tap ?? null,
  };
}

export function stripDeepLinkFromUrl(loc: URL): URL {
  const next = new URL(loc.href);
  for (const k of DEEP_LINK_KEYS) next.searchParams.delete(k);
  if (next.hash.startsWith('#')) {
    const hashParams = new URLSearchParams(next.hash.slice(1));
    for (const k of DEEP_LINK_KEYS) hashParams.delete(k);
    const rest = hashParams.toString();
    next.hash = rest ? `#${rest}` : '';
  }
  return next;
}
