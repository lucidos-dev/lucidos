import { useEffect, useState } from 'preact/hooks';
import { releaseNoticeView } from '../../store/store';
import { loadReleaseNotices } from '../../store/actions/releaseNotices';
import { releaseNoticeAnswer } from '../../store/releaseNotices';
import { Disclosure } from '../shared/Disclosure';
import { ReleaseNoticeActions, ReleaseNoticeCard } from '../shared/ReleaseNoticeCard';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf } from '../shared/Skeleton';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { ChevronDownIcon, ChevronRightIcon } from '../shared/icons';

export function ReleaseNoticesPage() {
  usePanelRefresh('release notices', loadReleaseNotices);
  const loadable = releaseNoticeView.value;
  const showLoading = useDelayedLoading(loadable);
  const [showAnswered, setShowAnswered] = useState(false);

  useEffect(() => {
    void loadReleaseNotices();
  }, []);

  if (loadable.status === 'failed') {
    return <LoadableError error={loadable.error} noun="the release notices" />;
  }

  const loaded = loadable.status === 'loaded' ? loadable.data : null;
  const releases = loaded?.releases ?? [];
  const owed = releases.filter((release) => !release.resolved);
  const answered = releases.filter((release) => release.items.some((item) => item.resolved)).reverse();
  const answer = loaded && releaseNoticeAnswer(loaded);

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="release-notices:list">
        Release notices
      </div>
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={<ListSkeletonOf count={1} containerClass="release-notice-list" row={() => <ReleaseNoticeCard show="owed" />} />}
      >
        {loaded?.releases.length === 0 && (
          <div class="empty-state">
            Nothing to do. A notice lands here when an upgrade needs something
            from you, which most releases do not.
          </div>
        )}
        {owed.length > 0 && (
          <div class="release-notice-list">
            {owed.map((release) => <ReleaseNoticeCard key={release.since} release={release} show="owed" />)}
            <div class="release-notice-actions">
              <ReleaseNoticeActions answer={answer} prompt={loaded?.action_prompt ?? null} />
            </div>
          </div>
        )}
        {answered.length > 0 && (
          <>
            <button
              type="button"
              class="settings-disclosure-toggle release-notice-answered-toggle"
              aria-expanded={showAnswered}
              onClick={() => setShowAnswered(!showAnswered)}
            >
              {showAnswered ? <ChevronDownIcon size="1rem" /> : <ChevronRightIcon size="1rem" />}
              Already answered ({answered.length})
            </button>
            <Disclosure open={showAnswered}>
              <div class="release-notice-list">
                {answered.map((release) => <ReleaseNoticeCard key={release.since} release={release} show="answered" />)}
              </div>
            </Disclosure>
          </>
        )}
      </LoadingFade>
    </div>
  );
}
