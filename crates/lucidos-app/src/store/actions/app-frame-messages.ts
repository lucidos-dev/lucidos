/** The host chrome an app frame can ask for: confirm, prompt, toast, file
 *  preview, pull to refresh, the ready signal, and a widget's height.
 *
 *  Installed by shell startup, before `<App/>` first renders, so before any
 *  app frame can exist to ask (ADR 0288). The frame bridge that answers engine
 *  calls is `app-bridge.ts`, which client startup installs. */

import { syncAppFullscreenHost } from '../appFullscreenHost';
import { showConfirm, showPrompt } from '../store';
import { isKnownAppFrame } from '../../utils/appFrame';
import { openFilePreviewModal, filePreviewRequestError, filePreviewBlockedReason } from './filePreviewModal';
import { handleAppToastMessage } from './app-toast-bridge';
import { handleAppPullMessage } from './app-pull-bridge';
import { handleAppReadyMessage } from './app-ready-bridge';
import { handleAppHeightMessage } from './app-height-bridge';

/** Answer every app frame message the host chrome handles. Returns the
 *  teardown. */
export function installAppFrameMessages(): () => void {
  window.addEventListener('message', onAppFrameMessage);
  window.addEventListener('message', handleAppPullMessage);
  window.addEventListener('message', handleAppReadyMessage);
  window.addEventListener('message', handleAppHeightMessage);
  return () => {
    window.removeEventListener('message', onAppFrameMessage);
    window.removeEventListener('message', handleAppPullMessage);
    window.removeEventListener('message', handleAppReadyMessage);
    window.removeEventListener('message', handleAppHeightMessage);
  };
}

function onAppFrameMessage(event: MessageEvent) {
  const data = event.data as {
    type?: unknown;
    id?: unknown;
    payload?: {
      title?: unknown; message?: unknown; okLabel?: unknown; cancelLabel?: unknown; danger?: unknown;
      type?: unknown; durationMs?: unknown; dismissable?: unknown; key?: unknown; spinning?: unknown;
      defaultValue?: unknown; placeholder?: unknown; multiline?: unknown;
      file_path?: unknown; line?: unknown; line_end?: unknown;
    };
  } | null;
  if (!data || typeof data !== 'object') return;
  if (
    data.type !== 'lucidos:ui:confirm' && data.type !== 'lucidos:ui:toast'
    && data.type !== 'lucidos:ui:dismissToast'
    && data.type !== 'lucidos:ui:prompt' && data.type !== 'lucidos:ui:preview-file'
  ) return;
  const payload = data.payload;
  if (!payload || typeof payload !== 'object') return;

  // Reject messages from any iframe that isn't a current app iframe,
  // so nested iframes (embeds, ads) can't trigger host modals / toasts.
  // Ahead of every branch below, deliberately: a frame we don't know gets no
  // host chrome and no reply, whatever it asked for.
  const source = event.source as Window | null;
  if (!source || !isKnownAppFrame(source)) return;

  // File preview: a read-only modal over the app, carrying a locator rather
  // than a message. Answered as soon as we have decided, since the SDK's
  // promise resolves when the preview is showing (not when it is dismissed).
  if (data.type === 'lucidos:ui:preview-file') {
    if (typeof data.id !== 'string') return;
    // Re-derive where host chrome renders before deciding. The refusal below
    // and the portal that would act on it must read the same instant. The
    // layer's target is published from a component render, which can lag a
    // fullscreen transition by a frame.
    syncAppFullscreenHost();
    // The request itself, then whether the host can put anything on screen
    // at all (it cannot render over a fullscreen element it does not own).
    // A refusal is the honest answer: a resolved promise with no visible
    // modal is what made this fail silently.
    const error = filePreviewRequestError(payload) ?? filePreviewBlockedReason();
    if (!error) {
      openFilePreviewModal({
        file_path: payload.file_path as string,
        line: payload.line,
        line_end: payload.line_end,
      });
    }
    try {
      source.postMessage(
        { type: 'lucidos:ui:preview-file:result', id: data.id, ok: error === null, error: error ?? undefined },
        '*',
      );
    } catch {
      // Source iframe may have unloaded, so drop the reply silently.
    }
    return;
  }

  // Toast and its dismissal: fire-and-forget, no id, no result reply. Ahead
  // of the message guard below deliberately, since a dismissal carries only
  // a key and that guard would swallow it.
  if (handleAppToastMessage(data.type, payload)) return;

  // Confirm and prompt both carry a message and are useless without one.
  if (typeof payload.message !== 'string' || payload.message.length === 0) return;

  // Both confirm and prompt carry an id and post a result back.
  if (typeof data.id !== 'string') return;
  const title = typeof payload.title === 'string' ? payload.title : undefined;
  const cancelLabel = typeof payload.cancelLabel === 'string' && payload.cancelLabel.length > 0 ? payload.cancelLabel : 'Cancel';

  // Prompt: text input; resolves a string (OK) or null (cancel).
  if (data.type === 'lucidos:ui:prompt') {
    const okLabel = typeof payload.okLabel === 'string' && payload.okLabel.length > 0 ? payload.okLabel : 'OK';
    showPrompt(payload.message, {
      title,
      cancelLabel,
      okLabel,
      defaultValue: typeof payload.defaultValue === 'string' ? payload.defaultValue : undefined,
      placeholder: typeof payload.placeholder === 'string' ? payload.placeholder : undefined,
      multiline: payload.multiline === true,
    }).then((value) => {
      try {
        source.postMessage({ type: 'lucidos:ui:prompt:result', id: data.id, value }, '*');
      } catch {
        // Source iframe may have unloaded, so drop the reply silently.
      }
    }).catch(() => { /* showPrompt rejection: drop, modal already closed */ });
    return;
  }

  // Confirm: boolean result.
  const okLabel = typeof payload.okLabel === 'string' && payload.okLabel.length > 0 ? payload.okLabel : 'Confirm';
  const variant: 'danger' | 'default' = payload.danger === true ? 'danger' : 'default';

  showConfirm(payload.message, okLabel, { title, cancelLabel, variant }).then((ok) => {
    try {
      source.postMessage({ type: 'lucidos:ui:confirm:result', id: data.id, ok }, '*');
    } catch {
      // Source iframe may have unloaded, so drop the reply silently.
    }
  }).catch(() => { /* showConfirm rejection: drop, modal already closed */ });
}
