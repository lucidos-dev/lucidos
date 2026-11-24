import { parseAppId } from '@lucidos/sdk';
import { API_BASE } from '../api/client/_core';

export function appFrameFor(source: MessageEventSource | null): HTMLIFrameElement | null {
  if (!source) return null;
  return mountedAppFrames().find((f) => f.contentWindow === source) ?? null;
}

export function mountedAppFrames(): HTMLIFrameElement[] {
  return Array.from(document.querySelectorAll<HTMLIFrameElement>('iframe[data-role="app-ui-frame"]'));
}

export function appIdForFrame(frame: HTMLIFrameElement | null): string | null {
  const src = frame?.getAttribute('src');
  if (!src) return null;
  try {
    return parseAppId(new URL(src, window.location.origin).pathname, API_BASE);
  } catch {
    return null;
  }
}

export const CANVAS_APP_FRAME_SELECTOR = '[data-role="app-ui-panel"] iframe[data-role="app-ui-frame"]';

export function isCanvasAppFrame(frame: HTMLIFrameElement | null): boolean {
  return frame?.closest('[data-role="app-ui-panel"]') != null;
}

export function isKnownAppFrame(source: MessageEventSource | null): boolean {
  return appFrameFor(source) !== null;
}
