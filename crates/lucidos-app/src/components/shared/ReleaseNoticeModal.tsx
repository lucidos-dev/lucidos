import { Overlay } from './Overlay';
import { renderMarkdown } from '../../utils/renderMarkdown';
import {
  acknowledgeReleaseNotice,
  dismissReleaseNoticeModal,
  takeReleaseNoticeAction,
} from '../../store/actions/releaseNotices';
import {
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

  return (
    <Overlay
      open
      onClose={dismissReleaseNoticeModal}
      panelClass="confirm-dialog release-notice"
      panelRole="dialog"
      ariaModal
    >
      <div class="release-notice-meta">
        <span>Since Lucidos {notice.since}</span>
        {/* Only worth saying when there is a queue behind this one. "1 of 1"
            invents a sequence the reader is not in. */}
        {remaining > 1 && <span class="release-notice-step">1 of {remaining}</span>}
      </div>
      <h2 class="confirm-title">{notice.title}</h2>
      <div
        class="markdown-content release-notice-body"
        dangerouslySetInnerHTML={{ __html: renderMarkdown(notice.body) }}
      />
      <div class="confirm-actions">
        <div class="confirm-actions-right">
          <button
            class="confirm-btn confirm-btn-cancel"
            onClick={() => void acknowledgeReleaseNotice(notice)}
          >
            Got it
          </button>
          {notice.action_label && (
            <button
              class="confirm-btn confirm-btn-ok"
              onClick={() => void takeReleaseNoticeAction(notice)}
            >
              {notice.action_label}
            </button>
          )}
        </div>
      </div>
    </Overlay>
  );
}
