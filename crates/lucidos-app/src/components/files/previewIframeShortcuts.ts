
import { dispatchPreviewIframeShortcut } from '../../hooks/useKeyboardShortcuts';

const onPreviewKeydown = (e: KeyboardEvent) => { dispatchPreviewIframeShortcut(e); };

const bridged = new WeakSet<Document>();

export function bridgePreviewIframeShortcuts(iframe: HTMLIFrameElement | null): void {
  if (!iframe) return;
  let doc: Document | null;
  try {
    doc = iframe.contentDocument;
  } catch {
    return;
  }
  if (!doc || bridged.has(doc)) return;
  bridged.add(doc);
  doc.addEventListener('keydown', onPreviewKeydown, true);
}
