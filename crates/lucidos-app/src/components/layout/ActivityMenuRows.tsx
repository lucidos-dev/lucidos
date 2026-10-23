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
import { liveActivityRows, type ActivityBody, type ActivityRow, type ApplyThread } from '../../store/actions/activityRows';
import { activityAction, buildTimerIsLive } from '../../store/actions/backgroundActivity';
import { openApplyPhase } from '../../store/actions/applyProgress';
import { cancelApplyAllBatch } from '../../store/actions/chat-changes';
import { LANE_PLACE_LABEL } from '../../store/actions/landingLane';
import { cancelLaneBatch, closeMainCheck, removeFromLandingLane } from '../../store/actions/laneControls';
import {
  LANE_GATHERING_LABEL,
  LANE_STEP_LABEL,
  liveLandingLaneView,
  type LaneEntry,
  type LaneView,
  type MainCheckLine,
} from '../../store/actions/landingLaneRow';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
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
  /** What the landing lane's row unfolds to. */
  lane: LaneView;
  onToggle: (key: string) => void;
  onOpenThread: (thread: ApplyThread) => void;
  onCancelApplyAll: () => void;
  onOpenLaneThread: (threadId: string) => void;
  onOpenLaneEntry: (entry: LaneEntry) => void;
  onRemoveFromLane: (changeId: string) => void;
  onCancelLaneBatch: (batchId: string) => void;
  onOpenFixThread: (threadId: string) => void;
  onCloseMainCheck: (checkId: string) => void;
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

/** A thread as a link: what is happening on one line, its title under it. */
function linkRow(phase: string, title: string, onClick: () => void): VNode {
  return (
    <button
      type="button"
      role="menuitem"
      class="brand-menu-activity-link"
      data-role="activity-thread-link"
      onClick={onClick}
    >
      <span class="brand-menu-activity-phase">{phase}</span>
      <span class="brand-menu-activity-thread">{title}</span>
      <ChevronRightIcon size="1rem" />
    </button>
  );
}

/** The thread an apply is on, as a link to the event its phase started at. */
function threadLink(thread: ApplyThread, onOpenThread: (thread: ApplyThread) => void): VNode {
  return linkRow(thread.phase, thread.title, () => onOpenThread(thread));
}

/** The main check's line: passing, running with its run, or red with its
 *  fix thread and Close. */
function mainCheckStatus(line: MainCheckLine, props: ActivityGroupProps): VNode {
  switch (line.kind) {
    case 'passing':
      return <div key="main-check" class="brand-menu-activity-position" data-role="main-check">Main check: passing</div>;
    case 'running':
      return line.runUrl ? (
        <a
          key="main-check"
          role="menuitem"
          class="brand-menu-activity-position"
          data-role="main-check"
          href={line.runUrl}
          target="_blank"
          rel="noopener noreferrer"
        >
          Main check: running
        </a>
      ) : (
        <div key="main-check" class="brand-menu-activity-position" data-role="main-check">Main check: running</div>
      );
    case 'red': {
      const phase = `Main red: ${line.summary}`;
      const fix = line.fixThread;
      return (
        <div key="main-check" class="brand-menu-activity-entry" data-role="main-check">
          {fix
            ? linkRow(phase, fix.title, () => props.onOpenFixThread(fix.threadId))
            : <div class="brand-menu-activity-position">{phase}</div>}
          {!line.closing && (
            /* Closes the red for good. The fix thread stays an ordinary
               thread, and what landed meanwhile starts the next check. */
            <button
              type="button"
              role="menuitem"
              class="action-btn action-btn-secondary"
              onClick={() => props.onCloseMainCheck(line.checkId)}
            >
              Close
            </button>
          )}
        </div>
      );
    }
  }
}

/** The lane's status lines, in order: the open batch's step on its lane
 *  thread, how many changes wait for the next batch, or that the lane gathers
 *  an Apply All press, then the main check. */
function laneStatusLines(body: LaneView, props: ActivityGroupProps): VNode[] {
  const lines: VNode[] = [];
  const { batch } = body;
  if (batch) {
    const thread = batch.laneThread;
    lines.push(
      <Fragment key="step">
        {linkRow(LANE_STEP_LABEL[batch.step], thread.title, () => props.onOpenLaneThread(thread.threadId))}
      </Fragment>,
    );
    if (body.waiting.length > 0) {
      lines.push(
        <div key="waiting" class="brand-menu-activity-position">
          {body.waiting.length} waiting for the next batch
        </div>,
      );
    }
  } else if (body.gathering && body.waiting.length > 0) {
    lines.push(
      <div key="gathering" class="brand-menu-activity-position">
        {LANE_GATHERING_LABEL}
      </div>,
    );
  }
  if (body.mainCheck) lines.push(mainCheckStatus(body.mainCheck, props));
  return lines;
}

/** A member of the open batch: it leaves only with its batch. */
function laneMember(entry: LaneEntry, props: ActivityGroupProps): VNode {
  return (
    <Fragment key={entry.changeId}>
      {linkRow(LANE_PLACE_LABEL['in-batch'], entry.title, () => props.onOpenLaneEntry(entry))}
    </Fragment>
  );
}

/** A waiting change, with Remove at its trailing edge. */
function laneWaiting(entry: LaneEntry, props: ActivityGroupProps): VNode {
  return (
    <div key={entry.changeId} class="brand-menu-activity-entry" data-role="lane-waiting">
      {linkRow(LANE_PLACE_LABEL.waiting, entry.title, () => props.onOpenLaneEntry(entry))}
      <button
        type="button"
        role="menuitem"
        class="action-btn action-btn-danger"
        aria-label={`Remove ${entry.title} from the landing lane`}
        onClick={() => props.onRemoveFromLane(entry.changeId)}
      >
        Remove
      </button>
    </div>
  );
}

function landingLaneBody(body: LaneView, props: ActivityGroupProps): VNode {
  const { batch } = body;
  return (
    <>
      {laneStatusLines(body, props)}
      {batch?.members.map((entry) => laneMember(entry, props))}
      {body.waiting.map((entry) => laneWaiting(entry, props))}
      {batch && !body.canceling && (
        <div class="brand-menu-activity-actions">
          {/* Fails the whole batch and stops its lane thread. Every member
              stays pending, and nothing queues it again until Apply. */}
          <button
            type="button"
            role="menuitem"
            class="action-btn action-btn-danger"
            onClick={() => props.onCancelLaneBatch(batch.batchId)}
          >
            Cancel
          </button>
        </div>
      )}
    </>
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
          {body.count && (
            <div class="brand-menu-activity-position">
              {body.count.done} of {body.count.total} done
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
    case 'landing-lane':
      return landingLaneBody(props.lane, props);
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
  // A build counts seconds, and an apply's phase counts minutes. The lane
  // row counts no time.
  const ticking = buildTimerIsLive()
    || rows.some((row) => row.body.kind === 'apply-all' || row.body.kind === 'apply-thread');
  useEffect(() => {
    if (!ticking) return;
    const handle = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(handle);
  }, [ticking]);

  const pending: PendingCode = !engineVersionPending.value
    ? 'none'
    : engineRebuildWedged.value ? 'wedged' : 'pending';
  // A fix thread or lane thread may be archived, so the page never loaded it.
  const openThread = (threadId: string) => {
    onClose();
    focusThreadOrBootstrap(threadId);
  };

  return activityMenuGroup({
    rows,
    pending,
    open,
    commits: engineBuildDetail.value?.pendingCommits ?? null,
    lane: liveLandingLaneView(),
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
    onOpenLaneThread: openThread,
    onOpenLaneEntry: (entry) => {
      if (!entry.threadId) return;
      onClose();
      openApplyPhase(entry.threadId, entry.changeId, null);
    },
    onRemoveFromLane: (changeId) => void removeFromLandingLane(changeId),
    onCancelLaneBatch: (batchId) => void cancelLaneBatch(batchId),
    onOpenFixThread: openThread,
    onCloseMainCheck: (checkId) => void closeMainCheck(checkId),
    onOpenPending: () => {
      onClose();
      // `showToast` freezes a new toast over the pane focused at that moment.
      // The mark lives in the thread pane, so the toast lands there too.
      focusPane('thread');
      openEngineVersionToast();
    },
  });
}
