import { useEffect, useState } from 'preact/hooks';
import { releaseNoticeView } from '../../store/store';
import {
  acknowledgeReleaseNotice,
  loadReleaseNotices,
  takeReleaseNoticeAction,
} from '../../store/actions/releaseNotices';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { LoadableError } from '../shared/LoadableError';
import { CheckIcon, ChevronDownIcon, ChevronRightIcon } from '../shared/icons';
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
 * The page's rows: what is still owed first, in the order it must be worked
 * through, then what has been read, newest first.
 *
 * Unresolved leads regardless of release, because it is the part that asks
 * something of the reader. Underneath, newest first matches the order every
 * other release-keyed list in Settings uses.
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

export interface NoticeSplit {
  /** Still owed, in the order they must be worked through. */
  owed: NoticeRow[];
  /** Answered, newest first. */
  answered: NoticeRow[];
}

/**
 * The two halves the page draws differently.
 *
 * A one-time instruction already carried out is a record, not a task. The two
 * must not sit together under one heading claiming work. Owed rows lead the
 * page, and answered ones fold away behind a disclosure.
 */
export function releaseNoticeSplit(view: ReleaseNoticeView): NoticeSplit {
  const rows = releaseNoticeRows(view);
  return {
    owed: rows.filter((r) => r.state !== 'resolved'),
    answered: rows.filter((r) => r.state === 'resolved'),
  };
}

/** One row. Only the OWED notice can be acted on: a queued one is waiting its
 *  turn, and an answered one is done. The modal keeps that order by drawing no
 *  later notice at all, and this is the same rule where every row is visible at
 *  once.
 *
 *  An unresolved row ALWAYS offers Got it, and any row with an action keeps that
 *  button, answered or not. Two separate conditions, and each is load bearing.
 *
 *  `action_label` is optional. Without the first, a notice carrying no action
 *  could be answered nowhere but the modal, which Escape closes for the page's
 *  life. The *System attention badge* points at this tab, so the reader would
 *  arrive at a dot with nothing to press. The second keeps an answered notice's
 *  button as a record of what it offered, greyed rather than gone. */
function NoticeRow({ notice, state, blockedBy }: NoticeRow & { blockedBy?: string }) {
  return (
    <div class="release-notice-row" data-state={state}>
      <div class="release-notice-row-head">
        {/* Decorative: "Already answered" above already says it, and the tick
            would otherwise be read out on every row under that heading. */}
        {state === 'resolved' && (
          <span class="release-notice-row-check" aria-hidden="true">
            <CheckIcon size="0.875rem" />
          </span>
        )}
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
          {/* Before the buttons, so the reading order matches the row: the
              explanation on the left, the controls it explains on the right. */}
          {state === 'queued' && blockedBy && (
            <span class="release-notice-row-hint">Work through "{blockedBy}" first.</span>
          )}
          {notice.action_label && (
            <button
              type="button"
              class="action-btn"
              disabled={state !== 'owed'}
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
              disabled={state !== 'owed'}
              onClick={() => void acknowledgeReleaseNotice(notice)}
            >
              Got it
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Settings > System > Release Notices: what the releases you have installed
 * need you to know or do.
 *
 * Its own tab rather than a block on What's New, because the two answer
 * different questions. What's New says what CHANGED, release by release. A
 * notice says what you have to do about it, once.
 *
 * This is also where a notice lives once its modal is gone. Tap one action, or
 * close the modal on the way to doing something else, and the rest are here
 * rather than lost.
 *
 * ONE heading, the page's own. The owed notices ARE the page, so they need no
 * heading. The answered ones fold under a disclosure that says what they are.
 * Reasoning: docs/plans/2026-08-25-release-notices-own-system-subpanel.md.
 */
export function ReleaseNoticesPage() {
  const loadable = releaseNoticeView.value;
  // Shut on arrival: the point of the fold is that a workspace owing nothing
  // opens on one quiet line. Page-local, like every other settings disclosure.
  const [showAnswered, setShowAnswered] = useState(false);

  useEffect(() => {
    void loadReleaseNotices();
  }, []);

  // Ahead of everything, deliberately. A dead engine answers nothing, and
  // "nothing to do" is the one wrong thing to say about an unknown list.
  //
  // No `.content-view.active.settings-panel` wrapper on any return here, and
  // none on the sibling System panels either: `SettingsView` already draws one
  // around the whole subview. A second would pad and size the page twice.
  if (loadable.status === 'failed') {
    return <LoadableError error={loadable.error} noun="the release notices" />;
  }

  const loaded = loadable.status === 'loaded' ? loadable.data : null;
  const { owed, answered } = loaded
    ? releaseNoticeSplit(loaded)
    : { owed: [], answered: [] };
  const blockedBy = owed.find((r) => r.state === 'owed')?.notice.title;

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="release-notices:list">
        Release notices
      </div>
      {loaded?.notices.length === 0 && (
        <div class="empty-state">
          Nothing to do. A notice lands here when an upgrade needs something
          from you, which most releases do not.
        </div>
      )}
      {owed.length > 0 && (
        <div class="release-notice-list">
          {owed.map((row) => (
            <NoticeRow key={row.notice.id} {...row} blockedBy={blockedBy} />
          ))}
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
          {showAnswered && (
            <div class="release-notice-list">
              {answered.map((row) => (
                <NoticeRow key={row.notice.id} {...row} />
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}
