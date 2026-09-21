import { parseAppId } from '@lucidos/sdk';
import { API_BASE } from '../api/client/_core';

/** The mounted app iframe a postMessage `event.source` belongs to, or null.
 *
 *  Identity against the live DOM, which is the only test that survives the app
 *  frame's opaque origin. `event.origin` reads `"null"` for every isolated
 *  frame. So it tells two of them apart no better than it tells an app from a
 *  nested embed inside one. Host-side handlers use this to reject a message
 *  from any other frame, so it cannot drive host-shell behavior. */
export function appFrameFor(source: MessageEventSource | null): HTMLIFrameElement | null {
  if (!source) return null;
  const frames = document.querySelectorAll<HTMLIFrameElement>('iframe[data-role="app-ui-frame"]');
  return Array.from(frames).find((f) => f.contentWindow === source) ?? null;
}

/** Which app a mounted frame is running, read from the `src` the host set.
 *
 *  The bridge is the first place in the system that knows this. The engine
 *  cannot tell an app's request from the shell's (ADR 0156 decision 1), and
 *  anything the frame sent would be the app's own claim. The element belongs to
 *  the host, so its `src` is not.
 *
 *  Parsed by the SDK's own `parseAppId`, so the host and the frame agree on
 *  what an app id is behind a gateway prefix. */
export function appIdForFrame(frame: HTMLIFrameElement | null): string | null {
  const src = frame?.getAttribute('src');
  if (!src) return null;
  try {
    // `API_BASE` is already `''` or `/<slug>` with no trailing slash, which is
    // the shape `parseAppId` strips (`utils/basePath.ts`).
    return parseAppId(new URL(src, window.location.origin).pathname, API_BASE);
  } catch {
    return null;
  }
}

/** True when `source` is the content window of a currently mounted app iframe.
 *  Shared by the SDK confirm bridge (`useStartup.ts`), the app-frame keyboard
 *  shortcut forwarder (`useKeyboardShortcuts.ts`) and the app bridge. */
export function isKnownAppFrame(source: MessageEventSource | null): boolean {
  return appFrameFor(source) !== null;
}
