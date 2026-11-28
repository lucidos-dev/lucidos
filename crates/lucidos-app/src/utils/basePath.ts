
import {
  REMOTE_GATEWAY_NAME_MAX_LEN,
  REMOTE_ROUTE_SEPARATOR,
  SIGIL,
} from '../generated/gateway-constants';

function baseHref(): string {
  if (typeof document === 'undefined') return '/';
  const href = document.querySelector('base')?.getAttribute('href');
  return href && href.length > 0 ? href : '/';
}

export function normalizeBasePath(href: string): string {
  let path = href;
  try {
    if (/^https?:\/\//i.test(href)) path = new URL(href).pathname;
  } catch {
  }
  if (!path.startsWith('/')) path = `/${path}`;
  path = path.replace(/\/+$/, '');
  return path;
}

export const BASE_PATH: string = normalizeBasePath(baseHref());

export const API = `${BASE_PATH}/api/v1`;

export const IS_PICKER: boolean = BASE_PATH === `/${SIGIL}`;

function readGatewayPort(): number | null {
  if (typeof document === 'undefined') return null;
  const raw = document
    .querySelector('meta[name="lucidos-gateway-port"]')
    ?.getAttribute('content');
  if (!raw) return null;
  const n = Number(raw);
  return Number.isInteger(n) && n > 0 ? n : null;
}

export const GATEWAY_PORT: number | null = readGatewayPort();

export function computeGatewayPickerHref(opts: {
  behindGateway: boolean;
  gatewayPort: number | null;
  protocol: string;
  hostname: string;
}): string | null {
  if (opts.behindGateway) return `/${SIGIL}/?pick`;
  if (opts.gatewayPort === null) return null;
  return `${opts.protocol}//${opts.hostname}:${opts.gatewayPort}/${SIGIL}/?pick`;
}

export const WORKSPACE_ID: string | null =
  IS_PICKER || BASE_PATH === '' ? null : BASE_PATH.slice(1);

export function gatewayPickerHref(): string | null {
  return computeGatewayPickerHref({
    behindGateway: WORKSPACE_ID !== null || IS_PICKER,
    gatewayPort: GATEWAY_PORT,
    protocol: typeof location !== 'undefined' ? location.protocol : 'https:',
    hostname: typeof location !== 'undefined' ? location.hostname : 'localhost',
  });
}

export function withBase(path: string): string {
  const normalized = path.startsWith('/') ? path : `/${path}`;
  return `${BASE_PATH}${normalized}`;
}

export const SCOPE_PATH: string = `${BASE_PATH}/`;

const SLUG_RE = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/;

export function isWorkspaceSlug(segment: string): boolean {
  return segment.length <= 64 && SLUG_RE.test(segment);
}

function isRemoteGatewayName(name: string): boolean {
  return (
    name.length <= REMOTE_GATEWAY_NAME_MAX_LEN && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(name)
  );
}

export interface WorkspaceRoute {
  slug: string;
  gateway: string | null;
}

export function parseWorkspaceRoute(segment: string): WorkspaceRoute | null {
  const at = segment.indexOf(REMOTE_ROUTE_SEPARATOR);
  if (at < 0) return isWorkspaceSlug(segment) ? { slug: segment, gateway: null } : null;
  const slug = segment.slice(0, at);
  const gateway = segment.slice(at + 1);
  return isWorkspaceSlug(slug) && isRemoteGatewayName(gateway) ? { slug, gateway } : null;
}

export function workspaceRouteSegment(route: WorkspaceRoute): string {
  return route.gateway === null ? route.slug : `${route.slug}${REMOTE_ROUTE_SEPARATOR}${route.gateway}`;
}

export function workspaceRouteUrlSegment(id: string): string {
  return encodeURIComponent(id)
    .split(encodeURIComponent(REMOTE_ROUTE_SEPARATOR))
    .join(REMOTE_ROUTE_SEPARATOR);
}

export const REMOTE_GATEWAY: string | null =
  WORKSPACE_ID === null ? null : (parseWorkspaceRoute(WORKSPACE_ID)?.gateway ?? null);

export const REMOTE_PREFLIGHT_QUERY = 'preflight=1';

export function clearRemotePreflightQuery(): void {
  if (window.location.search !== `?${REMOTE_PREFLIGHT_QUERY}`) return;
  window.history.replaceState(window.history.state, '', window.location.pathname + window.location.hash);
}

export function baseContextValidFor(workspaceId: string | null): boolean {
  return workspaceId === null || parseWorkspaceRoute(workspaceId) !== null;
}

export function baseContextIsValid(): boolean {
  return baseContextValidFor(WORKSPACE_ID);
}
