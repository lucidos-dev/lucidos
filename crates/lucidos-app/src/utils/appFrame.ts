/** True when a postMessage `event.source` is the content window of a currently
 *  mounted app iframe (`data-role="app-ui-frame"`). Lets host-side message
 *  handlers reject messages from any OTHER frame — a nested embed/ad inside an
 *  app, or an unrelated window — so they can't drive host-shell behavior.
 *  Shared by the SDK confirm bridge (`useStartup.ts`) and the app-frame keyboard
 *  shortcut forwarder (`useKeyboardShortcuts.ts`). */
export function isKnownAppFrame(source: MessageEventSource | null): boolean {
  if (!source) return false;
  const frames = document.querySelectorAll<HTMLIFrameElement>('iframe[data-role="app-ui-frame"]');
  return Array.from(frames).some((f) => f.contentWindow === source);
}
