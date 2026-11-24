import { signal } from '@preact/signals';

export const APP_PANEL_SELECTOR = '[data-role="app-ui-panel"]';
export const OVERLAY_MOUNT_SELECTOR = '[data-overlay-layer]';

export const appFullscreenHost = signal<HTMLElement | null>(null);

export function nativeFullscreenElement(): Element | null {
  if (typeof document === 'undefined') return null;
  const doc = document as unknown as { fullscreenElement?: Element | null; webkitFullscreenElement?: Element | null };
  return doc.fullscreenElement ?? doc.webkitFullscreenElement ?? null;
}

export function resolveOverlayMount(
  fullscreenEl: Element | null = nativeFullscreenElement(),
): HTMLElement | null {
  if (fullscreenEl === null || !fullscreenEl.isConnected) return null;
  if (!fullscreenEl.matches?.(APP_PANEL_SELECTOR)) return null;
  return fullscreenEl.querySelector<HTMLElement>(OVERLAY_MOUNT_SELECTOR);
}

export function syncAppFullscreenHost(): void {
  const mount = resolveOverlayMount();
  if (appFullscreenHost.peek() !== mount) appFullscreenHost.value = mount;
}

export function describeOverlayTarget(
  fullscreenEl: Element | null = nativeFullscreenElement(),
): string {
  if (fullscreenEl === null) return 'nothing is fullscreen';
  const tag = fullscreenEl.tagName?.toLowerCase() ?? '?';
  const cls = typeof fullscreenEl.className === 'string' && fullscreenEl.className
    ? `.${fullscreenEl.className.trim().split(/\s+/).join('.')}`
    : '';
  const detached = fullscreenEl.isConnected ? '' : ', detached';
  const panels = typeof document !== 'undefined'
    ? document.querySelectorAll(APP_PANEL_SELECTOR).length
    : 0;
  const mounts = typeof document !== 'undefined'
    ? document.querySelectorAll(OVERLAY_MOUNT_SELECTOR).length
    : 0;
  return `fullscreen: ${tag}${cls}${detached}; app panels: ${panels}; overlay mounts: ${mounts}`;
}

export function fullscreenBlocksHostOverlays(
  fullscreenEl: Element | null = nativeFullscreenElement(),
  mount: HTMLElement | null = resolveOverlayMount(fullscreenEl),
): boolean {
  if (fullscreenEl === null || !fullscreenEl.isConnected) return false;
  return mount === null;
}
