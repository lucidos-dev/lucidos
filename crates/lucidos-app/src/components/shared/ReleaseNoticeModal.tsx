import { Overlay } from './Overlay';
import { ReleaseNoticeActions, ReleaseNoticeCard, releaseNoticeHeading } from './ReleaseNoticeCard';
import { SurfaceHead } from './Surface';
import { dismissReleaseNoticeModal } from '../../store/actions/releaseNotices';
import { releaseNoticeAnswer, releaseNoticeModalView } from '../../store/releaseNotices';

export function ReleaseNoticeModal() {
  const view = releaseNoticeModalView();
  if (!view) return null;
  const { releases, action_prompt: prompt } = view;
  const answer = releaseNoticeAnswer(view);
  if (!answer) return null;

  const owed = releases.filter((release) => !release.resolved);
  const title = owed.length === 1
    ? releaseNoticeHeading(owed[0].since, answer.shown_ids.length)
    : 'Release notices';

  return (
    <Overlay
      open
      onClose={dismissReleaseNoticeModal}
      overlayClass="protected-surface"
      panelClass="surface surface-raised confirm-dialog release-notice protected-surface"
      panelRole="dialog"
      ariaModal
      panelProps={{ 'aria-label': title }}
    >
      <SurfaceHead
        title={title}
        onClose={dismissReleaseNoticeModal}
        closeLabel="Close, and show the release notices again later"
      />
      <div class="surface-body dialog-body release-notice-groups" tabIndex={-1}>
        {owed.map((release) => (
          <ReleaseNoticeCard key={release.since} release={release} showHeading={owed.length > 1} show="owed"
            onNavigate={dismissReleaseNoticeModal} />
        ))}
      </div>
      <div class="surface-foot">
        <button type="button" class="action-btn action-btn-secondary"
          data-role="release-notice-later" onClick={dismissReleaseNoticeModal}>
          Later
        </button>
        <ReleaseNoticeActions answer={answer} prompt={prompt} />
      </div>
    </Overlay>
  );
}
