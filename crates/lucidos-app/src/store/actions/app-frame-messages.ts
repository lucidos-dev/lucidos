
import { syncAppFullscreenHost } from '../appFullscreenHost';
import { showConfirm, showPrompt } from '../store';
import { isKnownAppFrame } from '../../utils/appFrame';
import { openFilePreviewModal, filePreviewRequestError, filePreviewBlockedReason } from './filePreviewModal';
import { handleAppToastMessage } from './app-toast-bridge';
import { handleAppPullMessage } from './app-pull-bridge';
import { handleAppReadyMessage } from './app-ready-bridge';
import { handleWidgetFrameMessage } from './widget-frame-bridge';
import { handleAppSoundMessage, watchShellSound } from './app-sound-bridge';

export function installAppFrameMessages(): () => void {
  window.addEventListener('message', onAppFrameMessage);
  window.addEventListener('message', handleAppPullMessage);
  window.addEventListener('message', handleAppReadyMessage);
  window.addEventListener('message', handleWidgetFrameMessage);
  window.addEventListener('message', handleAppSoundMessage);
  const unwatchShellSound = watchShellSound();
  return () => {
    window.removeEventListener('message', onAppFrameMessage);
    window.removeEventListener('message', handleAppPullMessage);
    window.removeEventListener('message', handleAppReadyMessage);
    window.removeEventListener('message', handleWidgetFrameMessage);
    window.removeEventListener('message', handleAppSoundMessage);
    unwatchShellSound();
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

  const source = event.source as Window | null;
  if (!source || !isKnownAppFrame(source)) return;

  if (data.type === 'lucidos:ui:preview-file') {
    if (typeof data.id !== 'string') return;
    syncAppFullscreenHost();
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
    }
    return;
  }

  if (handleAppToastMessage(data.type, payload)) return;

  if (typeof payload.message !== 'string' || payload.message.length === 0) return;

  if (typeof data.id !== 'string') return;
  const title = typeof payload.title === 'string' ? payload.title : undefined;
  const cancelLabel = typeof payload.cancelLabel === 'string' && payload.cancelLabel.length > 0 ? payload.cancelLabel : 'Cancel';

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
      }
    }).catch(() => {  });
    return;
  }

  const okLabel = typeof payload.okLabel === 'string' && payload.okLabel.length > 0 ? payload.okLabel : 'Confirm';
  const variant: 'danger' | 'default' = payload.danger === true ? 'danger' : 'default';

  showConfirm(payload.message, okLabel, { title, cancelLabel, variant }).then((ok) => {
    try {
      source.postMessage({ type: 'lucidos:ui:confirm:result', id: data.id, ok }, '*');
    } catch {
    }
  }).catch(() => {  });
}
