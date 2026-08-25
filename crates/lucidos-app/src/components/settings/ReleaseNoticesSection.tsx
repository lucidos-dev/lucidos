import { useEffect } from 'preact/hooks';
import { releaseNoticeView } from '../../store/store';
import {
  acknowledgeReleaseNotice,
  loadReleaseNotices,
  takeReleaseNoticeAction,
} from '../../store/actions/releaseNotices';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { LoadableError } from '../shared/LoadableError';
import type { ReleaseNotice, ReleaseNoticeView } from '../../api/client';

/** Where a notice sits for this workspace.
 *
 *  `owed` is the one the modal would show. `queued` is behind it and must wait,
 *  which is the ordering rule surviving outside the modal. `resolved` is read,
 *  and stays here so an instruction is never lost. */
export type NoticeRowState = 'owed' | 'queued' | 'resolved';

export interface NoticeRow {
  notice: ReleaseNotice;
  state: NoticeRowState;
}

/**
 * The panel's rows: what is still owed first, in the order it must be worked
 * through, then what has been read, newest first.
 *
 * Unresolved leads regardless of release, because it is the part that asks
 * something of the reader. Underneath, newest first matches the release list
 * this section sits above.
 */
export function releaseNoticeRows(view: ReleaseNoticeView): NoticeRow[] {
  const outstanding = view.notices
    .filter((n) => !n.resolved)
    .map((notice) => ({
      notice,
      state: (notice.id === view.next_id ? 'owed' : 'queued') as NoticeRowState,
    }));
  const resolved = view.notices
    .filter((n) => n.resolved)
    .reverse()
    .map((notice) => ({ notice, state: 'resolved' as NoticeRowState }));
  return [...outstanding, ...resolved];
}

/** One row. A button is live except on a queued notice, whose turn has not
 *  come. The modal keeps the order by drawing no later notice at all, and this
 *  is that same rule where every row is visible at once.
 *
 *  An unresolved row ALWAYS offers Got it, and any row with an action keeps
 *  that button, answered or not. Two separate conditions, and each is load
 *  bearing.
 *
 *  `action_label` is optional. Without the first, a notice carrying no action
 *  could be answered nowhere but the modal, which Escape closes for the page's
 *  life. The *What's New badge* points here, so the reader would arrive at a
 *  dot with nothing to press. The second keeps an answered notice's button, for
 *  the reader who took one action and came back for the rest. */
function NoticeRow({ notice, state, blockedBy }: NoticeRow & { blockedBy?: string }) {
  return (
    <div class="release-notice-row" data-state={state}>
      <div class="release-notice-row-head">
        <span class="release-notice-row-title">{notice.title}</span>
        {state !== 'resolved' && <span class="release-notice-row-mark">New</span>}
        <span class="release-notice-row-since">Since Lucidos {notice.since}</span>
      </div>
      <div
        class="markdown-content release-notice-body"
        dangerouslySetInnerHTML={{ __html: renderMarkdown(notice.body) }}
      />
      {(notice.action_label || state !== 'resolved') && (
        <div class="release-notice-row-actions">
          {notice.action_label && (
            <button
              type="button"
              class="action-btn"
              disabled={state === 'queued'}
              onClick={() => void takeReleaseNoticeAction(notice)}
            >
              {notice.action_label}
            </button>
          )}
          {/* The same word the modal uses, for the same act: it answers this
              notice and nothing else. */}
          {state !== 'resolved' && (
            <button
              type="button"
              class="action-btn"
              disabled={state === 'queued'}
              onClick={() => void acknowledgeReleaseNotice(notice)}
            >
              Got it
            </button>
          )}
          {state === 'queued' && blockedBy && (
            <span class="release-notice-row-hint">Work through "{blockedBy}" first.</span>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Settings > System > What's New, above the release list: what this release
 * needs the reader to know or do.
 *
 * This is where a notice lives once its modal is gone. Tap one action, or close
 * the modal on the way to doing something else, and the rest are here rather
 * than lost.
 *
 * Renders nothing at all when the workspace has no notices, which is the
 * ordinary case: most releases carry none.
 */
export function ReleaseNoticesSection() {
  const loadable = releaseNoticeView.value;

  useEffect(() => {
    void loadReleaseNotices();
  }, []);

  if (loadable.status === 'failed') {
    return <LoadableError error={loadable.error} noun="the release notices" />;
  }
  if (loadable.status !== 'loaded' || loadable.data.notices.length === 0) return null;

  const rows = releaseNoticeRows(loadable.data);
  const owed = rows.find((r) => r.state === 'owed')?.notice.title;

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="whats-new:notices">
        What you need to do
      </div>
      <div class="release-notice-list">
        {rows.map((row) => (
          <NoticeRow key={row.notice.id} {...row} blockedBy={owed} />
        ))}
      </div>
    </div>
  );
}
