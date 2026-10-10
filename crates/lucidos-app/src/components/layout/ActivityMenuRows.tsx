/**
 * The *activity group*: the Lucidos menu's rows for work in flight, one per
 * job. A tap unfolds the job's detail under its row, the way the Workspaces
 * row unfolds its list, and the menu stays open. It is what the spinning badge
 * on the mark advertises, and the one place progress is told: no apply, build
 * or download raises a progress toast.
 *
 * The same split as `NotificationsMenuRows.tsx` beside it.
 * `activityMenuGroup` is PURE, so every branch is testable through
 * `vnodeToText` with no DOM. `ActivityMenuGroup` is the hook-bearing wrapper
 * that reads the signals, holds which rows are open, and ticks the counters.
 */

import { Fragment, type VNode } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { engineBuildDetail, engineRebuildWedged, engineVersionPending } from '../../store/store';
import {
  liveActivityRows,
  type ActivityBody,
  type ActivityRow,
  type ApplyThread,
} from '../../store/actions/activityRows';
import { activityAction, buildTimerIsLive } from '../../store/actions/backgroundActivity';
import { openApplyPhase } from '../../store/actions/applyProgress';
import { cancelApplyAllBatch } from '../../store/actions/chat-changes';
import { openEngineVersionToast } from '../../store/actions/engine-update';
import { focusPane } from '../../store/actions/pane';
import {
  describedItems,
  GROUP_LABEL,
  housekeepingLine,
  pendingCommitsHeadline,
  type BackgroundActivity,
} from '../../store/backgroundActivity';
import type { PendingCommits } from '../../api/client';
import type { StepProgress } from '../../store/types';
import { ChevronDownIcon, ChevronRightIcon, HourglassIcon } from '../shared/icons';
import { Disclosure } from '../shared/Disclosure';
import { progressFillWidth } from '../shared/progressBar';

/** How often an open menu re-reads the clock while a job counts its time. */
const TICK_MS = 1000;

/** New code in source with no version built behind it, and whether a rebuild
 *  could still deliver it. Not in flight, so it has no spinner. */
export type PendingCode = 'none' | 'pending' | 'wedged';

export interface ActivityGroupProps {
  rows: ActivityRow[];
  pending: PendingCode;
  /** The keys of the rows unfolded now. */
  open: ReadonlySet<string>;
  /** What the engine build brings, or `null` when git could not say. */
  commits: PendingCommits | null;
  onToggle: (key: string) => void;
  onOpenThread: (thread: ApplyThread) => void;
  onCancelApplyAll: () => void;
  onOpenPending: () => void;
}

/** What the pending row says. It states what exists, never what to press:
 *  the toast behind the tap decides whether a Rebuild is on offer. */
export function pendingLabel(pending: Exclude<PendingCode, 'none'>): string {
  return pending === 'wedged' ? 'New code pending, no rebuild can deliver it' : 'New code pending';
}

function progressBar(progress: number | StepProgress): VNode {
  const done = typeof progress === 'number' ? progress : progress.done;
  return (
    <div class="progress-bar brand-menu-activity-progress">
      <div class="progress-bar-fill" style={{ width: progressFillWidth(done) }} />
      {typeof progress !== 'number' && (
        <div
          class="brand-menu-activity-working"
          style={{ left: progressFillWidth(progress.done), width: progressFillWidth(progress.working) }}
        />
      )}
    </div>
  );
}

/** What the build brings, grouped the way the new-version confirm groups it,
 *  from the same shared wording. `null` and zero both say nothing: an unknown
 *  count must not read as "0 commits" while a build runs. */
function commitsBody(commits: PendingCommits | null): VNode | null {
  if (!commits || commits.total === 0 || commits.groups.length === 0) return null;
  return (
    <div class="brand-menu-activity-changes" data-role="activity-changes">
      <div class="brand-menu-activity-headline">{pendingCommitsHeadline(commits.total)}</div>
      {commits.groups.map((group) =>
        group.kind === 'housekeeping' ? (
          <p key={group.kind} class="brand-menu-activity-housekeeping">{housekeepingLine(group.total)}</p>
        ) : (
          <section key={group.kind} class="brand-menu-activity-group-section">
            <div class="brand-menu-activity-group-title">{GROUP_LABEL[group.kind]}</div>
            <ul>
              {describedItems(group).map((item, i) => <li key={i}>{item}</li>)}
            </ul>
          </section>
        ),
      )}
    </div>
  );
}

function note(text: string): VNode {
  return <p class="brand-menu-activity-note">{text}</p>;
}

function backgroundBody(activity: BackgroundActivity, commits: PendingCommits | null): VNode {
  const primary = activityAction(activity.action);
  const secondary = activityAction(activity.secondaryAction);
  // A build names what it brings when git can count it, and otherwise says
  // what follows it.
  const counted = activity.kind === 'engine-build' ? commitsBody(commits) : null;
  const text = counted ?? (activity.note ? note(activity.note) : null);
  return (
    <>
      {activity.progress != null && progressBar(activity.progress)}
      {activity.queued && note(activity.queued)}
      {text}
      {(primary || secondary) && (
        <div class="brand-menu-activity-actions">
          {secondary && (
            <button type="button" role="menuitem" class="action-btn action-btn-secondary" onClick={secondary.onClick}>
              {secondary.label}
            </button>
          )}
          {primary && (
            <button type="button" role="menuitem" class="action-btn" onClick={primary.onClick}>{primary.label}</button>
          )}
        </div>
      )}
    </>
  );
}

/** The thread an apply is on, as a link to the event its phase started at. */
function threadLink(thread: ApplyThread, onOpenThread: (thread: ApplyThread) => void): VNode {
  return (
    <button
      type="button"
      role="menuitem"
      class="brand-menu-activity-link"
      data-role="activity-thread-link"
      onClick={() => onOpenThread(thread)}
    >
      <span class="brand-menu-activity-phase">{thread.phase}</span>
      <span class="brand-menu-activity-thread">{thread.title}</span>
      <ChevronRightIcon size="1rem" />
    </button>
  );
}

function activityBody(body: ActivityBody, props: ActivityGroupProps): VNode {
  switch (body.kind) {
    case 'background':
      return backgroundBody(body.activity, props.commits);
    case 'apply-thread':
      return threadLink(body.thread, props.onOpenThread);
    case 'apply-all':
      return (
        <>
          {body.position && (
            <div class="brand-menu-activity-position">
              Change {body.position.index} of {body.position.total}
              {body.timeLeft && ` · ${body.timeLeft}`}
            </div>
          )}
          {body.progress && progressBar(body.progress)}
          {body.thread && threadLink(body.thread, props.onOpenThread)}
          {!body.canceling && (
            <div class="brand-menu-activity-actions">
              {/* Stops the whole batch: it aborts the in-flight hardening or
                  merge and leaves the rest pending. */}
              <button type="button" role="menuitem" class="action-btn action-btn-danger" onClick={props.onCancelApplyAll}>
                Cancel
              </button>
            </div>
          )}
        </>
      );
  }
}

function activityRow(row: ActivityRow, props: ActivityGroupProps): VNode {
  const open = props.open.has(row.key);
  return (
    <Fragment key={row.key}>
      <button
        type="button"
        class={`brand-menu-item brand-menu-activity-row${open ? ' is-expanded' : ''}`}
        role="menuitem"
        aria-expanded={open}
        aria-label={row.detail ? `${row.label}, ${row.detail}` : row.label}
        onClick={() => props.onToggle(row.key)}
      >
        <span class="brand-menu-activity-mark" aria-hidden="true">
          {row.queued ? <HourglassIcon /> : <span class="mini-spinner" />}
        </span>
        <span class="brand-menu-activity-label">{row.label}</span>
        {row.detail && <span class="brand-menu-activity-detail" aria-hidden="true">{row.detail}</span>}
        <span class="brand-menu-activity-chevron" aria-hidden="true"><ChevronDownIcon /></span>
      </button>
      <Disclosure open={open}>
        <div class="brand-menu-activity-body" role="group" aria-label={row.label} data-role="activity-body">
          {activityBody(row.body, props)}
        </div>
      </Disclosure>
    </Fragment>
  );
}

/** The group, or nothing at all when nothing runs and nothing is pending. The
 *  separator comes from here, under the rows, so an idle menu draws neither. */
export function activityMenuGroup(props: ActivityGroupProps) {
  const { rows, pending, onOpenPending } = props;
  if (rows.length === 0 && pending === 'none') return null;
  return (
    <>
      <div class="brand-menu-activity-group" role="group" aria-label="Activity">
        {rows.map((row) => activityRow(row, props))}
        {pending !== 'none' && (
          <button
            key="pending-code"
            type="button"
            class={`brand-menu-item brand-menu-activity-row${pending === 'wedged' ? ' is-wedged' : ''}`}
            role="menuitem"
            onClick={onOpenPending}
          >
            <span class="brand-menu-activity-mark" aria-hidden="true"><span class="brand-menu-activity-dot" /></span>
            <span class="brand-menu-activity-label">{pendingLabel(pending)}</span>
            <ChevronRightIcon size="1rem" />
          </button>
        )}
      </div>
      <div class="brand-menu-separator" role="separator" />
    </>
  );
}

/** The group as the signals stand, inside the menu's `<Overlay>`. The open
 *  rows and the ticker live only while the menu is open. */
export function ActivityMenuGroup({ onClose }: { onClose: () => void }) {
  const [now, setNow] = useState(() => Date.now());
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const rows = liveActivityRows(now);
  // A build counts seconds, and an apply's phase counts minutes.
  const ticking = buildTimerIsLive() || rows.some((row) => row.body.kind !== 'background');
  useEffect(() => {
    if (!ticking) return;
    const handle = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(handle);
  }, [ticking]);

  const pending: PendingCode = !engineVersionPending.value
    ? 'none'
    : engineRebuildWedged.value ? 'wedged' : 'pending';

  return activityMenuGroup({
    rows,
    pending,
    open,
    commits: engineBuildDetail.value?.pendingCommits ?? null,
    onToggle: (key) => {
      const next = new Set(open);
      if (!next.delete(key)) next.add(key);
      setOpen(next);
    },
    onOpenThread: (thread) => {
      onClose();
      openApplyPhase(thread.threadId, thread.changeId ?? undefined, thread.reading);
    },
    onCancelApplyAll: () => void cancelApplyAllBatch(),
    onOpenPending: () => {
      onClose();
      // `showToast` freezes a new toast over the pane focused at that moment.
      // The mark lives in the thread pane, so the toast lands there too.
      focusPane('thread');
      openEngineVersionToast();
    },
  });
}
