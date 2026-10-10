// Bridge the app shell's keyboard shortcuts into a same-origin PDF preview
// iframe. The frame runs no SDK, so unlike an app iframe (`AppUiInline` +
// `keyboardForward.ts`) its keydowns never reach the host. While DOM focus sits
// inside it, every shell shortcut would silently die. The chord would fall
// through to the browser's own default instead (the reported bug: ⌘⇧↵ to
// maximize the content pane opened Chrome's page context menu).
//
// An engine-served PDF is same-origin, so the host listens on the frame's own
// `contentDocument` and dispatches against its shortcut registry. Having the
// real event, it can `preventDefault()` the browser default too. A cross-origin
// frame throws on `contentDocument` access; that is swallowed as a no-op.
//
// The HTML artifact preview is NOT bridged here. It runs at an opaque origin
// (ADR 0322), and forwards its chords over `previewFrameBridge.ts` instead.

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
