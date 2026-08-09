import {
  frontendPreview,
  frontendPreviewBusy,
  previewHref,
  startPreviewForThread,
  stopPreview,
} from '../../store/actions/frontend-preview';
import { readDeviceId } from '../../utils/deviceIdHeader';

/**
 * The frontend preview's controls, rendered as a section of the coding-agent
 * control menu: the surface the user is already in while chatting with the
 * agent that is making the change.
 *
 * Three states, because there is exactly ONE preview slot per workspace
 * (`engine::frontend_preview`):
 *   - nothing running        → Start
 *   - running for this thread → Open, Stop
 *   - running for another one → Move here (which replaces it)
 *
 * The href is built from this page's `location`, not from the engine's answer:
 * the engine derives its `url` from whichever request last touched the
 * endpoint, and that may have been the CLI on the host machine, whose `Host` is
 * `localhost`. A localhost link on a phone is a dead link. It also carries this
 * thread, so Open lands on the conversation the preview is serving rather than
 * on the preview origin's own empty compose view.
 *
 * Not part of the menu's keyboard-navigable `flatItems` list: those are
 * commands sent to the agent session, and these are direct actions on a
 * process. They are ordinary buttons, so Tab still reaches them.
 */
export function FrontendPreviewSection({ threadId }: { threadId: string }) {
  const preview = frontendPreview.value;
  const busy = frontendPreviewBusy.value;
  const running = preview?.running === true;
  const isThisThread = running && preview?.thread_id === threadId;
  const href = isThisThread
    ? previewHref(
        preview?.port,
        { protocol: location.protocol, hostname: location.hostname },
        readDeviceId(),
        threadId,
      )
    : null;

  return (
    <>
      <div class="control-section-label">Frontend preview</div>
      <div class="control-preview-row">
        {isThisThread ? (
          <>
            {href && (
              <a class="action-btn" href={href} target="_blank" rel="noopener">
                Open
              </a>
            )}
            <button class="action-btn action-btn-danger" disabled={busy} onClick={() => void stopPreview()}>
              {busy ? 'Stopping...' : 'Stop'}
            </button>
          </>
        ) : (
          <button
            class="action-btn"
            disabled={busy}
            onClick={() => void startPreviewForThread(threadId)}
          >
            {busy ? 'Starting...' : running ? 'Move here' : 'Start'}
          </button>
        )}
      </div>
      <div class="control-preview-hint">
        {isThisThread
          ? 'This branch, live, with hot reload. No Apply needed.'
          : running
            ? 'Running for another thread. Moving it replaces that one.'
            : 'Serve this branch on its own port, so UI changes show up before Apply.'}
      </div>
    </>
  );
}
