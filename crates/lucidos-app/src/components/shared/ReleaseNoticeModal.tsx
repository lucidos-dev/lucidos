import { Overlay } from './Overlay';
import { SurfaceHead } from './Surface';
import { renderMarkdown } from '../../utils/renderMarkdown';
import {
  acknowledgeReleaseNotice,
  dismissReleaseNoticeModal,
  takeReleaseNoticeAction,
} from '../../store/actions/releaseNotices';
import {
  modalActionLabel,
  owedReleaseNotice,
  owedReleaseNoticeCount,
  releaseNoticeDismissed,
} from '../../store/releaseNotices';

/**
 * The one *release notice* this workspace still owes an answer to.
 *
 * A stepper, not a list. Got it answers the notice on screen and the next one
 * takes its place, so several are read in one sitting and always in order. A
 * later notice cannot be acted on early, because it is not drawn yet.
 *
 * Escape, the X and an outside click all close WITHOUT answering, so the notice
 * returns on the next open. That is the whole reason the dismissal is a
 * page-local signal rather than a write.
 */
export function ReleaseNoticeModal() {
  const notice = owedReleaseNotice();
  if (!notice || releaseNoticeDismissed.value) return null;

  const remaining = owedReleaseNoticeCount();
  const actionLabel = modalActionLabel(notice);

  return (
    <Overlay
      open
      onClose={dismissReleaseNoticeModal}
      overlayClass="protected-surface"
      panelClass="surface surface-raised confirm-dialog release-notice protected-surface"
      panelRole="dialog"
      ariaModal
    >
      <SurfaceHead
        title={notice.title}
        onClose={dismissReleaseNoticeModal}
        closeLabel="Close, and show this notice again later"
      />
      <div class="surface-body dialog-body" tabIndex={-1}>
        <div class="release-notice-meta">
          <span>Since Lucidos {notice.since}</span>
          {/* Only worth saying when there is a queue behind this one. "1 of 1"
              invents a sequence the reader is not in. */}
          {remaining > 1 && <span class="release-notice-step">1 of {remaining}</span>}
        </div>
        <div
          class="markdown-content release-notice-body"
          dangerouslySetInnerHTML={{ __html: renderMarkdown(notice.body) }}
        />
        {notice.action_deferred && notice.action_label && (
          <p class="release-notice-deferred">
            {notice.action_label} comes with a later notice, so you do it once.
          </p>
        )}
      </div>
      <div class="surface-foot">
        <button
          class={`action-btn${actionLabel ? ' action-btn-secondary' : ''}`}
          data-role="release-notice-ack"
          onClick={() => void acknowledgeReleaseNotice(notice)}
        >
          Got it
        </button>
        {actionLabel && (
          <button
            class="action-btn"
            data-role="release-notice-action"
            onClick={() => void takeReleaseNoticeAction(notice)}
          >
            {actionLabel}
          </button>
        )}
      </div>
    </Overlay>
  );
}
