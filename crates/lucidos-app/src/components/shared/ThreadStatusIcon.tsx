import type { ThreadMeta, ThreadStatus, ThreadState } from '../../store/thread-events';
import { effectiveThreadStatus } from '../../store/store';
import { PauseIcon } from './icons';

/** 'changes' = static dot (CC has pending changes); 'question' = "?" badge
 *  (CC paused on AskUserQuestion). 'waiting' = pulsing dot, used only when
 *  the thread has no own state of its own to surface (otherwise own state
 *  wins, even with active children or a live subscription). */
export type VisualStatus = ThreadStatus | 'changes' | 'question';

/** `waiting` covers BOTH ways a thread can be finished-but-not-done: it is
 *  waiting on child threads it spawned, or on an *event wait* it registered.
 *  A child counts while it is mid-turn, and while it idles on its own event
 *  wait, since that child has not finished either (ADR 0254).
 *  Neither holds the thread's turn, so both land here with a backend
 *  `status` of `idle`, and both mean the same thing to the reader: something
 *  else will wake this, do not treat it as finished. They deliberately share
 *  one dot rather than splitting into two, since the distinction between them
 *  is detail the waiting indicator carries.
 *
 *  The two causes rank differently against `changes`, because the Apply gate
 *  treats them differently. A live event wait outranks it: the thread wakes on
 *  its own branch, so `availableThreadActions` withholds Apply and Discard. A
 *  running sub-thread ranks below it: the child writes its own worktree, so a
 *  parent with a proposed change reads "Changes to review" and offers Apply
 *  (ADR 0249). `is_blocking`, `is_attention_needing` and `displaySection` never
 *  see either, so a parked thread stays archivable when it has no change to
 *  resolve (ADR 0049). `VisualStatus` is the right home for the fact because
 *  it is already a derived axis: `changes` and `question` are not
 *  `ThreadStatus` values either. */
export function resolveVisualStatus(
  status: ThreadStatus,
  waitsOnSubThreads: boolean,
  codingAgentProposed: boolean,
  hasLiveEventWaits: boolean,
): VisualStatus {
  if (status === 'failed') return 'failed';
  if (status === 'running') return 'running';
  if (status === 'waiting_for_user_answer') return 'question';
  // The user's own version switch interrupted this turn, and the engine is
  // bringing it back. It outranks `changes` for the same reason `failed` does:
  // it describes what happened to the turn, which the user needs before they
  // decide whether to review anything.
  //
  // THIS function is where that precedence lives, and the only place. The
  // backend used to state it a second time, writing `waiting` instead of the
  // verdict whenever a change was pending. That cost the verdict outright, so
  // an interrupted thread with a change came back reading Running. See
  // `docs/plans/2026-08-22-a-restart-verdict-survives-a-pending-change.md`.
  if (status === 'paused') return 'paused';
  // A live event wait outranks `changes`: the thread is not finished, so its
  // change is not final and cannot be resolved yet. Reading it as "Changes to
  // review" invited an Apply that would merge a branch still being worked on.
  if (hasLiveEventWaits) return 'waiting';
  if (codingAgentProposed) return 'changes';
  if (waitsOnSubThreads) return 'waiting';
  return 'idle';
}

/** The meta facts `visualStatusFor` reads. */
type VisualStatusFacts = Pick<
  ThreadMeta,
  'activeChildrenCount' | 'waitingChildrenCount' | 'codingAgentProposed' | 'liveEventWaitCount'
>;

/** `resolveVisualStatus` fed from a thread's meta, for a surface holding a
 *  status snapshot of its own. `undefined` meta is a thread the client has not
 *  loaded, which resolves on the status alone. Every surface that paints a
 *  dot goes through here, so none of them can drop one of the inputs. */
export function visualStatusFor(status: ThreadStatus, meta: VisualStatusFacts | undefined): VisualStatus {
  if (!meta) return resolveVisualStatus(status, false, false, false);
  return resolveVisualStatus(
    status,
    meta.activeChildrenCount + (meta.waitingChildrenCount ?? 0) > 0,
    meta.codingAgentProposed,
    meta.liveEventWaitCount > 0,
  );
}

/** The single source of truth for a thread's status dot. Every surface that
 *  paints the dot — the drawer row, the desktop panel header, the mobile
 *  thread title bar — calls THIS with the same live thread from `threadMap`,
 *  so the dot can never disagree between the drawer and the panel (the
 *  invariant: same thread → same status, everywhere). Don't reconstruct the
 *  `resolveVisualStatus(effectiveThreadStatus(t), …)` triple at a call site;
 *  call this instead. */
export function threadVisualStatus(thread: ThreadState): VisualStatus {
  return visualStatusFor(effectiveThreadStatus(thread), thread.meta);
}

/** User-facing label + one-line explanation for each status dot, shown in its
 *  hover tooltip. Single source of truth so the dot and its explanation can
 *  never drift. (idle has no dot, so it needs no entry.) */
const STATUS_INFO: { status: VisualStatus; label: string; desc: string }[] = [
  { status: 'running', label: 'Running', desc: 'Actively working on a response.' },
  // One description for both causes, because one dot covers both (see
  // `resolveVisualStatus`). Naming only children would be a lie on a thread
  // that is watching for an event, and the per-wait detail is one tap away in
  // the waiting indicator. A thread with a change and only children reads
  // `changes` instead, so this dot never sits on an applicable change.
  { status: 'waiting', label: 'Waiting', desc: 'Not finished: it is waiting for a child thread, or for an event it subscribed to. While it waits on an event, any proposed change waits with it.' },
  { status: 'question', label: 'Waiting for your answer', desc: 'Paused until you answer its question.' },
  { status: 'changes', label: 'Changes to review', desc: 'A coding agent proposed changes to open and Apply.' },
  { status: 'paused', label: 'Paused', desc: 'Your switch to a new version interrupted this turn. It resumes on its own, so there is nothing to do.' },
  { status: 'failed', label: 'Failed', desc: 'The last response failed.' },
];

/** Tooltip for a status dot: its status name as the bold title, the one-line
 *  explanation as the body — consumed by the global tooltip system
 *  (`data-tooltip-title` / `data-tooltip`). */
export function statusTooltip(status: VisualStatus): { title: string; text: string } {
  const current = STATUS_INFO.find((s) => s.status === status);
  if (!current) return { title: '', text: '' };
  return { title: current.label, text: current.desc };
}

interface Props {
  status: VisualStatus | null;  // null = not loaded yet
}

export function ThreadStatusIcon({ status }: Props) {
  if (status === null) return (
    <span class="thread-status thread-status-loading">
      <span class="progress-dot progress-dot-loading" />
    </span>
  );
  // idle has no dot to hover, so skip the tooltip (it'd attach to a 0-size span).
  const tip = status === 'idle' ? null : statusTooltip(status);
  return (
    <span
      class={`thread-status thread-status-${status}`}
      data-tooltip={tip?.text}
      data-tooltip-title={tip?.title}
    >
      {status === 'running' && (
        <span class="mini-spinner" />
      )}
      {status === 'waiting' && (
        <span class="progress-dot progress-dot-waiting" />
      )}
      {status === 'changes' && (
        <span class="progress-dot progress-dot-changes" />
      )}
      {status === 'question' && (
        <span class="thread-status-question-badge" aria-label="Waiting for your answer" />
      )}
      {status === 'paused' && (
        <span class="thread-status-paused-icon" aria-label="Paused by your version switch, resuming on its own">
          <PauseIcon />
        </span>
      )}
      {status === 'failed' && (
        <span class="progress-dot progress-dot-failed" aria-label="Last response failed" />
      )}
{/* idle = no icon — it's the default state for every finished thread */}
    </span>
  );
}
