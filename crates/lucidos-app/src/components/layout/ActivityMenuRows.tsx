
import { Fragment, type VNode } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { engineBuildDetail, engineVersionPending, engineBuildDeferred, engineVersionReady } from '../../store/store';
import { liveActivityRows, type ActivityBody, type ActivityRow, type ApplyThread } from '../../store/actions/activityRows';
import { activityAction, buildTimerIsLive } from '../../store/actions/backgroundActivity';
import { openApplyPhase } from '../../store/actions/applyProgress';
import { cancelApplyAllBatch } from '../../store/actions/chat-changes';
import { LANE_PLACE_LABEL, LANE_WAITING_FOR_LANDER_LABEL } from '../../store/actions/landingLane';
import { fixMainNow, cancelLaneBatch, closeMainCheck, removeFromLandingLane } from '../../store/actions/laneControls';
import {
  LANE_GATHERING_LABEL,
  LANE_WAITING_FOR_ORIGIN_LABEL,
  LANE_STEP_LABEL,
  liveLandingLaneView,
  type LaneEntry,
  type LaneView,
  type MainCheckLine,
} from '../../store/actions/landingLaneRow';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { openEngineVersionToast, showEngineBuildLog, triggerRebuild } from '../../store/actions/engine-update';
import { focusPane } from '../../store/actions/pane';
import {
  describedItems,
  GROUP_LABEL,
  housekeepingLine,
  pendingCommitsHeadline,
  type BackgroundActivity,
} from '../../store/backgroundActivity';
import { formatElapsed } from '../../utils/elapsed';
import type { DeferredBuild, PendingCommits } from '../../api/client';
import type { StepProgress } from '../../store/types';
import { ChevronDownIcon, ChevronRightIcon, HourglassIcon } from '../shared/icons';
import { Disclosure } from '../shared/Disclosure';
import { progressFillWidth } from '../shared/progressBar';

const TICK_MS = 1000;

export type PendingCode = 'none' | 'pending';

export interface ActivityGroupProps {
  rows: ActivityRow[];
  pending: PendingCode;
  deferred?: DeferredBuild | null;
  versionReady?: boolean;
  onBuildNow?: () => void;
  open: ReadonlySet<string>;
  commits: PendingCommits | null;
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
  onFixMain?: () => void;
  onOpenPending: () => void;
  onShowBuildLog: () => void;
}

export function pendingLabel(): string {
  return 'New code pending';
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

function backgroundBody(activity: BackgroundActivity, props: ActivityGroupProps): VNode {
  const { commits } = props;
  const primary = activityAction(activity.action);
  const secondary = activity.kind === 'engine-build'
    ? { label: 'Show build log', onClick: props.onShowBuildLog }
    : activityAction(activity.secondaryAction);
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

function threadLink(thread: ApplyThread, onOpenThread: (thread: ApplyThread) => void): VNode {
  return linkRow(thread.phase, thread.title, () => onOpenThread(thread));
}

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
      const phase = `${line.flakyOnly ? 'Main flaky' : 'Main red'}: ${line.summary}`;
      const fix = line.fixThread;
      return (
        <div key="main-check" class="brand-menu-activity-entry" data-role="main-check">
          {fix
            ? linkRow(phase, fix.title, () => props.onOpenFixThread(fix.threadId))
            : <div class="brand-menu-activity-position">{phase}</div>}
          {line.repair && (
            <div class="brand-menu-activity-position">
              {line.repair.state === 'running' ? 'Repair running'
                : line.repair.state === 'waiting' ? `Repair waits for ${line.repair.waiting_on.join(', ') || 'idle capacity'}`
                : 'No repair scheduled'}
            </div>
          )}
          {line.repair && line.repair.state !== 'running' && (
            <button type="button" role="menuitem" class="action-btn action-btn-secondary" onClick={props.onFixMain}>
              Fix now
            </button>
          )}
          {!line.closing && !line.flakyOnly && (
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
  if (body.waitingForOrigin) {
    lines.push(
      <div key="waiting-for-origin" class="brand-menu-activity-position" data-role="lane-waiting-for-origin">
        {LANE_WAITING_FOR_ORIGIN_LABEL}: {body.waitingForOrigin}
      </div>,
    );
  }
  if (body.multipleLanders) {
    lines.push(
      <div key="multiple-landers" class="brand-menu-activity-position">
        Multiple landers detected. Set one workspace to Here and the others to Another host in Settings → Coding Agents.
      </div>,
    );
  }
  if (body.mainCheck) lines.push(mainCheckStatus(body.mainCheck, props));
  return lines;
}

function laneEntryRow(phase: string, entry: LaneEntry, props: ActivityGroupProps): VNode {
  if (!entry.remote) return linkRow(phase, entry.title, () => props.onOpenLaneEntry(entry));
  return (
    <div class="brand-menu-activity-link">
      <span class="brand-menu-activity-phase">{phase}</span>
      <span class="brand-menu-activity-thread">{entry.title}</span>
    </div>
  );
}

function laneMember(entry: LaneEntry, props: ActivityGroupProps): VNode {
  return (
    <Fragment key={entry.changeId}>
      {laneEntryRow(LANE_PLACE_LABEL['in-batch'], entry, props)}
    </Fragment>
  );
}

function laneWaiting(entry: LaneEntry, props: ActivityGroupProps, waitingForLander: boolean): VNode {
  return (
    <div key={entry.changeId} class="brand-menu-activity-entry" data-role="lane-waiting">
      {laneEntryRow(waitingForLander ? LANE_WAITING_FOR_LANDER_LABEL : LANE_PLACE_LABEL.waiting, entry, props)}
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
      {body.waiting.map((entry) => laneWaiting(entry, props, !!body.waitingForLander))}
      {batch && !body.canceling && (
        <div class="brand-menu-activity-actions">
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
      return backgroundBody(body.activity, props);
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

export function activityMenuGroup(props: ActivityGroupProps) {
  const { rows, pending, onOpenPending } = props;
  if (rows.length === 0 && pending === 'none' && !props.deferred) return null;
  return (
    <>
      <div class="brand-menu-activity-group" role="group" aria-label="Activity">
        {rows.map((row) => activityRow(row, props))}
        {props.deferred && (
          <div key="deferred-build" class="brand-menu-activity-entry">
            <div class="brand-menu-item brand-menu-activity-row">
              <span class="brand-menu-activity-mark" aria-hidden="true"><HourglassIcon /></span>
              <span class="brand-menu-activity-label">{props.versionReady ? 'Newer version waits' : 'New version waits'}</span>
            </div>
            <div class="brand-menu-activity-position">
              {props.deferred.coding_threads} coding {props.deferred.coding_threads === 1 ? 'thread' : 'threads'} active · {formatElapsed(props.deferred.deferred_ms)}
            </div>
            {props.deferred.threads.map((thread) => (
              <button key={thread.id} type="button" role="menuitem" class="brand-menu-activity-link"
                data-role="deferred-thread" onClick={() => props.onOpenFixThread(thread.id)}>
                {thread.title}{thread.state === 'waiting_for_user_answer' ? ' (waits for your answer)' : ''}
              </button>
            ))}
            <div class="brand-menu-activity-position">It builds by itself when they finish.</div>
            <button type="button" role="menuitem" class="brand-menu-activity-link"
              data-role="build-now" onClick={props.onBuildNow}>Build now</button>
          </div>
        )}
        {pending !== 'none' && !props.deferred && (
          <button
            key="pending-code"
            type="button"
            class="brand-menu-item brand-menu-activity-row"
            role="menuitem"
            onClick={onOpenPending}
          >
            <span class="brand-menu-activity-mark" aria-hidden="true"><span class="brand-menu-activity-dot" /></span>
            <span class="brand-menu-activity-label">{pendingLabel()}</span>
            <ChevronRightIcon size="1rem" />
          </button>
        )}
      </div>
      <div class="brand-menu-separator" role="separator" />
    </>
  );
}

export function ActivityMenuGroup({ onClose }: { onClose: () => void }) {
  const [now, setNow] = useState(() => Date.now());
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set());
  const rows = liveActivityRows(now);
  const deferred = engineBuildDeferred.value;
  const ticking = buildTimerIsLive() || deferred !== null
    || rows.some((row) => row.body.kind === 'apply-all' || row.body.kind === 'apply-thread');
  useEffect(() => {
    if (!ticking) return;
    const handle = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(handle);
  }, [ticking]);

  const pending: PendingCode = !engineVersionPending.value
    ? 'none'
    : 'pending';
  const openThread = (threadId: string) => {
    onClose();
    focusThreadOrBootstrap(threadId);
  };

  return activityMenuGroup({
    rows,
    pending,
    deferred: deferred ? { ...deferred, deferred_ms: deferred.deferred_ms + Math.max(0, now - deferred.anchoredAt) } : null,
    versionReady: engineVersionReady.value,
    onBuildNow: () => { onClose(); void triggerRebuild('join'); },
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
    onFixMain: () => void fixMainNow(),
    onOpenPending: () => {
      onClose();
      focusPane('thread');
      openEngineVersionToast();
    },
    onShowBuildLog: () => {
      onClose();
      showEngineBuildLog();
    },
  });
}
