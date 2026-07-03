// Bridge the app shell's keyboard shortcuts into content-pane PREVIEW iframes
// (file / HTML / diff / PDF previews). These iframes don't load the Lucidos SDK,
// so — unlike app iframes (`AppUiInline` + `keyboardForward.ts`) — their keydowns
// never reach the host. While DOM focus sits inside a preview iframe, every shell
// shortcut silently dies and the chord falls through to Chrome's own default for
// the combo (the reported bug: ⌘⇧↵ to maximize the content pane opened Chrome's
// page context menu instead).
//
// Preview iframes are SAME-ORIGIN (`about:srcdoc` inherits the host origin;
// engine-served files are same-origin), so the host can listen on the iframe's
// own `contentDocument` and dispatch against its shortcut registry — and, having
// the real event, `preventDefault()` the browser default (which the postMessage
// forward path used for apps cannot do). A cross-origin preview (an external URL)
// throws on `contentDocument` access; we swallow it and no-op — there's nothing
// we can reach into.

import { dispatchPreviewIframeShortcut } from '../../hooks/useKeyboardShortcuts';

const onPreviewKeydown = (e: KeyboardEvent) => { dispatchPreviewIframeShortcut(e); };

// Each iframe load installs a fresh `contentDocument`; track the ones we've wired
// so a reload that reuses a document (or a double `load`) can't stack listeners.
// WeakSet so a discarded document is collected with its listener.
const bridged = new WeakSet<Document>();

/** Wire host keyboard shortcuts into a preview iframe's same-origin document.
 *  Call from the iframe's `onLoad` (the `contentDocument` only exists once
 *  loaded). No-op for a missing iframe or a cross-origin document. Capture phase
 *  so we see the chord before the preview content's own handlers might stop it.
 *
 *  Focus (lighting the content pill on a click) is NOT handled here — a PDF's
 *  native plugin swallows pointer events before they reach `contentDocument`, and
 *  a cross-origin preview blocks it entirely. The host-side window-blur tracker
 *  (`installContentPaneIframeFocusTracking` in `components/layout/paneFocus.ts`)
 *  covers every content-pane iframe uniformly instead. */
export function bridgePreviewIframeShortcuts(iframe: HTMLIFrameElement | null): void {
  if (!iframe) return;
  let doc: Document | null;
  try {
    doc = iframe.contentDocument;
  } catch {
    return; // cross-origin preview — can't reach in
  }
  if (!doc || bridged.has(doc)) return;
  bridged.add(doc);
  doc.addEventListener('keydown', onPreviewKeydown, true);
}
