
import { onHostPush } from './_bridge';

export const CAPABILITY_SEGMENT = '~cap';

export const CAPABILITY_CHANNEL = 'frame-capability';

export interface SplitPath {
  prefix: string;
  capability: string;
  rest: string;
}

export function splitCapability(path: string): SplitPath | null {
  const marker = `/${CAPABILITY_SEGMENT}/`;
  const at = path.indexOf(marker);
  if (at < 0) return null;
  const after = path.slice(at + marker.length);
  const slash = after.indexOf('/');
  if (slash <= 0) return null;
  return { prefix: path.slice(0, at), capability: after.slice(0, slash), rest: after.slice(slash) };
}

function baseElement(): HTMLBaseElement | null {
  if (typeof document === 'undefined') return null;
  return document.querySelector('base');
}

export function currentCapability(): string | null {
  const href = baseElement()?.getAttribute('href');
  if (!href) return null;
  return splitCapability(href)?.capability ?? null;
}

export function capabilityCarrier(): string {
  const capability = currentCapability();
  return capability ? `/${CAPABILITY_SEGMENT}/${capability}` : '';
}

export function adoptCapability(capability: string): boolean {
  if (!capability || capability.includes('/')) return false;
  const element = baseElement();
  const href = element?.getAttribute('href');
  if (!element || !href) return false;
  const split = splitCapability(href);
  if (!split) return false;
  element.setAttribute(
    'href',
    `${split.prefix}/${CAPABILITY_SEGMENT}/${capability}${split.rest}`,
  );
  return true;
}

export function documentRelative(url: string | URL): string | URL {
  if (!currentCapability()) return url;
  try {
    return new URL(url, location.href).href;
  } catch {
    return url;
  }
}

export function inPageFragment(href: string | null): string | null {
  if (!href?.startsWith('#') || !currentCapability()) return null;
  return new URL(href, location.href).href;
}

type HistoryUrlMethods = Pick<History, 'pushState' | 'replaceState'>;

export function anchorHistoryToDocument(target: HistoryUrlMethods): void {
  for (const method of ['pushState', 'replaceState'] as const) {
    const native = target[method];
    target[method] = function (this: History, data: unknown, unused: string, url?: string | URL | null) {
      if (url == null) return native.call(this, data, unused, url);
      return native.call(this, data, unused, documentRelative(url));
    };
  }
}

export function installDocumentRelativeUrls(): void {
  anchorHistoryToDocument(History.prototype);
  const followInPageLink = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.composedPath().find(
      (node): node is Element => node instanceof Element && node.matches('a[href], area[href]'),
    );
    if (!link || link.hasAttribute('download')) return;
    const target = link.getAttribute('target');
    if (target && target !== '_self') return;
    const url = inPageFragment(link.getAttribute('href'));
    if (!url) return;
    event.preventDefault();
    location.assign(url);
  };
  window.addEventListener('click', () => {
    window.removeEventListener('click', followInPageLink);
    window.addEventListener('click', followInPageLink, { once: true });
  }, true);
}

interface RenewedCapability {
  capability?: unknown;
}

export function installCapabilityRenewal(): () => void {
  return onHostPush(CAPABILITY_CHANNEL, (payload) => {
    const renewed = (payload ?? {}) as RenewedCapability;
    if (typeof renewed.capability === 'string') adoptCapability(renewed.capability);
  });
}
