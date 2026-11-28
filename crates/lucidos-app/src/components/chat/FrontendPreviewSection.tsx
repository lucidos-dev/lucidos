import {
  frontendPreview,
  frontendPreviewBusy,
  previewHref,
  startPreviewForThread,
  stopPreview,
} from '../../store/actions/frontend-preview';
import { isTauri } from '../../utils/platform';
import { keepFocusOnPress } from '../../utils/dom';

export function FrontendPreviewSection({ threadId }: { threadId: string }) {
  const loadable = frontendPreview.value;
  const preview = loadable.status === 'loaded' ? loadable.data : null;
  const known = preview !== null || loadable.status === 'failed';
  const busy = frontendPreviewBusy.value;
  const running = preview?.running === true;
  const isThisThread = running && preview?.thread_id === threadId;
  const href = isThisThread ? previewHref(preview, location.origin, threadId) : null;

  return (
    <>
      <div class="control-section-label">Frontend preview</div>
      <div class="control-preview-row" data-state={loadable.status}>
        {isThisThread ? (
          <>
            {href && (
              <a class="action-btn" href={href} target={isTauri() ? '_self' : '_blank'} rel="noopener">
                Open
              </a>
            )}
            <button
              class="action-btn action-btn-danger"
              disabled={busy}
              onMouseDown={keepFocusOnPress}
              onClick={() => void stopPreview()}
            >
              {busy ? 'Stopping...' : 'Stop'}
            </button>
          </>
        ) : (
          <button
            class="action-btn"
            disabled={busy || !known}
            onMouseDown={keepFocusOnPress}
            onClick={() => void startPreviewForThread(threadId)}
          >
            {busy ? 'Starting...' : running ? 'Move here' : 'Start'}
          </button>
        )}
      </div>
      <div class="control-preview-hint">
        {loadable.status === 'failed'
          ? `Could not read the preview status: ${loadable.error}`
          : isThisThread
              ? 'This branch, live, with hot reload. No Apply needed.'
              : running
                ? 'Running for another thread. Moving it replaces that one.'
                : 'Serve this branch through the gateway, so UI changes show up before Apply.'}
      </div>
    </>
  );
}
