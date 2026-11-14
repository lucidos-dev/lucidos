// Every full screen request in an app frame leaves breadcrumbs in the engine
// log as `[Client/app-fullscreen]`. The SDK wraps the browser's own request
// methods, so no app calls anything. On an iPhone, WebKit can accept a request
// and never open it, and the log is the only place that shows it.

import { traceToHost } from './hostTrace';

/** A request with no open after this long never opened. */
export const FULL_SCREEN_WAIT_MS = 2000;

type Detail = Record<string, unknown>;

function trace(step: string, detail: Detail = {}): void {
  traceToHost('log.app-fullscreen', step, detail);
}

/** What the element looked like at the request. A video adds its readiness. */
function describeRequest(api: string, element: Element): Detail {
  const detail: Detail = {
    api,
    element: element.tagName.toLowerCase(),
    gesture: navigator.userActivation?.isActive,
  };
  if (element instanceof HTMLVideoElement) {
    detail.readyState = element.readyState;
    detail.srcSet = Boolean(element.src);
    detail.supportsFullscreen = (element as { webkitSupportsFullscreen?: boolean }).webkitSupportsFullscreen;
  }
  return detail;
}

/** Wrap `owner[method]`: `before` runs ahead of each call and `threw` after a
 *  synchronous throw. The call itself runs as the app made it, with its `this`,
 *  arguments, return value and error. Returns the undo. */
function wrapMethod(
  owner: object,
  method: string,
  hooks: { before: (self: Element) => void; threw: (self: Element) => void },
): () => void {
  const slots = owner as Record<string, unknown>;
  const original = slots[method];
  if (typeof original !== 'function') return () => {};
  slots[method] = function (this: Element, ...args: unknown[]): unknown {
    try {
      hooks.before(this);
    } catch {
      // Telemetry must never break the request.
    }
    try {
      return original.apply(this, args);
    } catch (error) {
      try {
        trace('threw', { api: method, error: String(error) });
        hooks.threw(this);
      } catch {
        // Same: the app's own error is the one that counts.
      }
      throw error;
    }
  };
  return () => { slots[method] = original; };
}

/** Start tracing full screen in this document. Returns the teardown. */
export function installFullScreenTrace(): () => void {
  const pending = new Map<EventTarget, ReturnType<typeof setTimeout>>();

  const settle = (element: EventTarget): void => {
    clearTimeout(pending.get(element));
    pending.delete(element);
  };

  const hooksFor = (api: string) => ({
    before: (element: Element) => {
      const detail = describeRequest(api, element);
      trace('request', detail);
      settle(element);
      pending.set(element, setTimeout(() => {
        pending.delete(element);
        trace('never-opened', detail);
      }, FULL_SCREEN_WAIT_MS));
    },
    threw: settle,
  });

  const opened = (event: Event): void => {
    settle(event.target as EventTarget);
    trace('opened', { api: event.type, element: (event.target as Element).tagName?.toLowerCase() });
  };
  const closed = (event: Event): void => {
    trace('closed', { api: event.type, element: (event.target as Element).tagName?.toLowerCase() });
  };
  const changed = (event: Event): void => (document.fullscreenElement ? opened(event) : closed(event));

  const undo = [
    wrapMethod(HTMLVideoElement.prototype, 'webkitEnterFullscreen', hooksFor('webkitEnterFullscreen')),
    wrapMethod(Element.prototype, 'requestFullscreen', hooksFor('requestFullscreen')),
  ];
  const listeners: Array<[string, (event: Event) => void]> = [
    ['webkitbeginfullscreen', opened],
    ['webkitendfullscreen', closed],
    ['fullscreenchange', changed],
  ];
  for (const [type, listener] of listeners) document.addEventListener(type, listener, true);

  return () => {
    for (const fn of undo) fn();
    for (const [type, listener] of listeners) document.removeEventListener(type, listener, true);
    for (const timer of pending.values()) clearTimeout(timer);
    pending.clear();
  };
}
