import { Overlay } from './Overlay';
import { SurfaceHead } from './Surface';
import {
  dismissReleaseNoticeModal,
  openReleaseNoticesPanel,
} from '../../store/actions/releaseNotices';
import { owedReleaseNoticeTitles, releaseNoticeDismissed } from '../../store/releaseNotices';

/**
 * Announces the *release notices* this workspace still owes, and points at the
 * Release Notices panel, where they are read and answered.
 *
 * Not the place to answer them. An action starts a thread, and a modal would
 * have to close to show it, taking the notices behind it along. The panel
 * stays put.
 *
 * Every way of closing answers nothing, so the modal returns on the next open
 * until each notice is answered.
 */
export function ReleaseNoticeModal() {
  const titles = owedReleaseNoticeTitles();
  if (titles.length === 0 || releaseNoticeDismissed.value) return null;

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
        title={titles.length === 1 ? '1 release notice' : `${titles.length} release notices`}
        onClose={dismissReleaseNoticeModal}
        closeLabel="Close, and show the release notices again later"
      />
      <div class="surface-body dialog-body" tabIndex={-1}>
        <p>This upgrade changed things you may need to act on.</p>
        <ul class="release-notice-titles">
          {titles.map((title) => <li key={title}>{title}</li>)}
        </ul>
      </div>
      <div class="surface-foot">
        <button
          class="action-btn action-btn-secondary"
          data-role="release-notice-later"
          onClick={dismissReleaseNoticeModal}
        >
          Later
        </button>
        <button
          class="action-btn"
          data-role="release-notice-open"
          onClick={openReleaseNoticesPanel}
        >
          Read them
        </button>
      </div>
    </Overlay>
  );
}
