import { useSignal } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { cancelThreadEventWait } from '../../api/client';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import {
  effectiveThreadStatus,
  eventConditionDoor,
  focusedThreadId,
  isMidTurn,
  showToast,
  threadMap,
} from '../../store/store';
import type { EventSubscription, EventWaitSummary, ThreadState } from '../../store/thread-events';
import {
  awaitedSubject,
  formatRemaining,
  secondsRemaining,
  waitSubscriptionLabel,
} from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { CloseIcon, EventWaitClockIcon } from '../shared/icons';
import { Overlay } from '../shared/Overlay';
import { ThreadStatusIcon, threadVisualStatus } from '../shared/ThreadStatusIcon';

/** How often the countdown re-renders. One second is the granularity the text
 *  actually shows below a minute. Above that the text is coarse enough that the
 *  extra ticks are invisible, and a slower interval would make a `4m 12s` row
 *  visibly stutter. */
const COUNTDOWN_TICK_MS = 1000;

/** The sub-thread half of a wait: the active children this client can name, and
 *  how many more the server counts. */
export interface SubThreadWait {
  /** Active children resolved from the loaded `threadMap`, oldest first. */
  threads: ThreadState[];
  /** Active children `meta.activeChildrenCount` reports that the loaded map
   *  cannot name, so the panel can say so instead of showing a short list. */
  unresolved: number;
}

const NO_SUB_THREADS: SubThreadWait = { threads: [], unresolved: 0 };

/** The focused thread's active children, for the *waiting indicator*.
 *
 *  **`activeChildrenCount` decides, and the map only names.** The count is the
 *  server's, reconciled in-transaction against ground truth. `threadMap` is a
 *  paginated cache, which may hold all of a parent's children or none.
 *
 *  So a count of zero returns before walking the map at all. That keeps this
 *  O(1) on almost every thread. The prompt row re-renders on every `threadMap`
 *  flush, so scanning every loaded thread each time would cost real work.
 *  Almost no thread is waiting on children. The price is one SSE round trip of
 *  lag on a child seen starting before its parent's aggregate lands.
 *
 *  Active means `running` or `waiting_for_user_answer`, which is exactly the
 *  engine's `active_thread_statuses()` and exactly `isMidTurn`. A child that is
 *  idle while holding its own subscription is NOT active, and neither is one
 *  parked on a proposed change.
 *
 *  The subtraction runs one way. A shortfall becomes an "and N more" row, and a
 *  surplus is listed rather than cut (`docs/code-review-priors.md`). */
export function activeSubThreads(
  parentId: string,
  threads: ReadonlyMap<string, ThreadState>,
  activeChildrenCount: number,
): SubThreadWait {
  if (activeChildrenCount <= 0) return NO_SUB_THREADS;
  const found: ThreadState[] = [];
  for (const thread of threads.values()) {
    if (thread.meta.parentThreadId !== parentId) continue;
    if (!isMidTurn(effectiveThreadStatus(thread))) continue;
    found.push(thread);
  }
  found.sort((a, b) => a.meta.createdAt.localeCompare(b.meta.createdAt));
  return { threads: found, unresolved: Math.max(0, activeChildrenCount - found.length) };
}

/** The **waiting indicator**: what this thread is waiting for, readable at any
 *  time without scrolling the transcript.
 *
 *  Two things park a thread, and the status dot already merges them: a live
 *  *event wait*, and *sub-threads* still working (`resolveVisualStatus`). Both
 *  say the same thing to the reader. This is not finished, and something else
 *  will wake it. So one control carries the detail for both.
 *
 *  For an event wait this is the primary surface, and the transcript card is
 *  the historical record. The split exists because a wait survives an
 *  interruption (S6b). A thread can be watching for something while it reads as
 *  `idle` and shows an ordinary reply. So "is anything subscribed" cannot be
 *  answered by the status dot, nor by the transcript's scroll position.
 *
 *  Reads `meta.liveEventWaits` (projected in `handleEvent`) so the render path
 *  is O(1), exactly like `TodoListIndicator` next to it. See
 *  `docs/plans/2026-08-22-one-waiting-indicator-for-subscriptions-and-sub-threads.md`. */
export function WaitingIndicator() {
  const open = useSignal(false);
  // useState (not useRef) so the dismiss hook re-runs once the button mounts
  // and we have a real anchor to exclude from the outside-click test.
  const [anchorEl, setAnchorEl] = useState<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const id = focusedThreadId.value;
  const meta = id ? threadMap.value.get(id)?.meta : undefined;
  const waits = meta?.liveEventWaits ?? [];
  const subThreads = id
    ? activeSubThreads(id, threadMap.value, meta?.activeChildrenCount ?? 0)
    : NO_SUB_THREADS;
  const isOpen = open.value && isWaitingForAnything(waits, subThreads);
  // Clamp into the thread pane that owns the indicator. On desktop that keeps
  // the panel out of the content pane. On a phone the pane IS the viewport, so
  // the same clamp is what stops it running off the right edge. Passing a
  // null anchor while closed makes the hook recompute from scratch on open,
  // rather than reusing coordinates measured before the last scroll.
  const pos = useAnchoredPosition(isOpen ? anchorEl : null, panelRef, '.thread-pane');

  return (
    <>
      {waitingIndicatorBody({
        waits,
        subThreads,
        onClick: () => (open.value = !open.value),
        buttonRef: setAnchorEl,
      })}
      {/* `portal` because the panel is `position: fixed` and the composer's
          ancestors animate `transform` (the thread reveal, the compose FLIP).
          A transformed ancestor becomes the containing block for a fixed
          descendant, which would resolve the hook's viewport coordinates
          against that ancestor instead. Portaling to <body> keeps the viewport
          as the containing block; the dismiss/anchor/Escape contracts are
          unaffected (see <Overlay>). */}
      <Overlay
        open={isOpen}
        onClose={() => {
          open.value = false;
        }}
        anchor={anchorEl}
        backdrop={false}
        portal
        panelClass="prompt-bar-popover waiting-panel"
        // `--prompt-bar-popover-fit` is the thread pane's usable width, the box
        // the hook clamped this panel's position into. The stylesheet's own
        // value is viewport-based, which on desktop lets the panel run out of
        // the pane and into the content pane.
        panelStyle={pos
          ? {
              top: `${pos.top}px`,
              left: `${pos.left}px`,
              '--prompt-bar-popover-fit': `${pos.maxWidth}px`,
            }
          : { visibility: 'hidden' }}
        panelRole="dialog"
        panelProps={{ 'aria-label': 'What this thread is waiting for' }}
        dataRole="waiting-panel"
        panelRef={panelRef}
      >
        {isOpen
          ? waitingPanelBody({
              threadId: id ?? '',
              waits,
              subThreads,
              onClose: () => {
                open.value = false;
              },
            })
          : null}
      </Overlay>
    </>
  );
}

/** True when the thread is parked on something the panel can show. The button
 *  and the panel share it, so neither can render while the other would be
 *  empty. */
function isWaitingForAnything(waits: EventWaitSummary[], subThreads: SubThreadWait): boolean {
  return waits.length > 0 || subThreadCount(subThreads) > 0;
}

function subThreadCount(subThreads: SubThreadWait): number {
  return subThreads.threads.length + subThreads.unresolved;
}

export function waitingIndicatorBody({
  waits,
  subThreads,
  onClick,
  buttonRef,
}: {
  waits: EventWaitSummary[];
  subThreads: SubThreadWait;
  onClick: () => void;
  buttonRef?: (el: HTMLButtonElement | null) => void;
}) {
  if (!isWaitingForAnything(waits, subThreads)) return null;
  // A lone subscription reads as its own reason, which is the most useful thing
  // the button can say. Anything else counts each kind instead, because two
  // reasons do not fit on a tooltip.
  const children = subThreadCount(subThreads);
  const soleReason = waits.length === 1 && children === 0 ? waits[0].reason : null;
  const counted = [
    waits.length > 0 ? plural(waits.length, 'subscription') : null,
    children > 0 ? plural(children, 'sub-thread') : null,
  ]
    .filter((part): part is string => part !== null)
    .join(', ');
  // The tooltip carries the reason as the model wrote it, and supplies no verb.
  // The aria-label DOES supply one, so it takes the subject instead: a reason
  // opening "waiting for the release build" would otherwise be spoken as
  // "Waiting for waiting for the release build".
  const spoken = soleReason ? awaitedSubject(soleReason) : counted;
  return (
    <button
      type="button"
      class="icon-btn header-icon"
      data-role="waiting-indicator"
      data-tooltip={soleReason ?? counted}
      aria-label={`Waiting for ${spoken}. Click to expand.`}
      onClick={onClick}
      data-row-item
      ref={buttonRef}
    >
      <EventWaitClockIcon />
    </button>
  );
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? '' : 's'}`;
}

/** One section per kind of wait, each rendered only when it has rows.
 *
 *  Returns the panel's CONTENTS, not its box: the box is the `<Overlay>` panel
 *  itself, which is what `useAnchoredPosition` measures and positions. */
export function waitingPanelBody({
  threadId,
  waits,
  subThreads,
  onClose,
}: {
  threadId: string;
  waits: EventWaitSummary[];
  subThreads: SubThreadWait;
  onClose: () => void;
}) {
  return (
    <>
      <div class="prompt-bar-popover-head">
        <span class="prompt-bar-popover-title">Waiting for</span>
        <button
          type="button"
          class="icon-btn prompt-bar-popover-close"
          aria-label="Close what this thread is waiting for"
          onClick={onClose}
        >
          <CloseIcon />
        </button>
      </div>
      <div class="prompt-bar-popover-body">
        {waits.length > 0 ? (
          <section class="waiting-panel-section" data-role="waiting-subscriptions">
            <span class="waiting-panel-section-label">Subscriptions</span>
            <ul class="event-wait-list">
              {waits.map((wait) => (
                <EventWaitRow key={wait.wait_id} threadId={threadId} wait={wait} />
              ))}
            </ul>
          </section>
        ) : null}
        {subThreadCount(subThreads) > 0 ? (
          <section class="waiting-panel-section" data-role="waiting-sub-threads">
            <span class="waiting-panel-section-label">Sub-threads</span>
            <ul class="waiting-panel-child-list">
              {subThreads.threads.map((child) => (
                <SubThreadRow key={child.meta.id} child={child} onOpen={onClose} />
              ))}
              {/* The server counts more active children than this client can
                  name. Say so, rather than show a list that contradicts the
                  thread's own Waiting dot. */}
              {subThreads.unresolved > 0 ? (
                <li class="waiting-panel-child-more" data-role="waiting-sub-threads-more">
                  {`and ${subThreads.unresolved} more`}
                </li>
              ) : null}
            </ul>
          </section>
        ) : null}
      </div>
    </>
  );
}

/** One active child. It links and does not stop: ending a sub-thread is done on
 *  the sub-thread, where its own Stop and its transcript are.
 *
 *  Opening one closes the panel, because the panel describes the thread it was
 *  opened from. Left open it would re-read against the child, and then either
 *  hang there showing the child's own waits or vanish. */
export function SubThreadRow({ child, onOpen }: { child: ThreadState; onOpen: () => void }) {
  const title = child.meta.title?.trim() || 'Untitled thread';
  return (
    <li class="waiting-panel-child">
      <button
        type="button"
        class="waiting-panel-child-link"
        data-role="waiting-sub-thread"
        data-thread-id={child.meta.id}
        onClick={() => {
          onOpen();
          focusThreadOrBootstrap(child.meta.id);
        }}
      >
        <ThreadStatusIcon status={threadVisualStatus(child)} />
        <span class="waiting-panel-child-title">{title}</span>
      </button>
    </li>
  );
}

/** The watched event types on one line, with a filtered one pressable so its
 *  `condition` is one tap away.
 *
 *  Segments rather than `describeWaitSubscription`'s joined string, because a
 *  string cannot carry a button. The joined LOOK is unchanged: still one muted
 *  mono line reading `A or B`, and an entry with no condition stays plain text.
 *
 *  The modal it opens STACKS over this popover on `overlayStack`. Escape or an
 *  outside click closes the modal and leaves the panel where it was. */
export function subscriptionLine(on: EventSubscription[]): ComponentChildren[] {
  return on.flatMap((s, i) => {
    const glue = i === 0 ? [] : [<span key={`glue${i}`}>{' or '}</span>];
    const label = waitSubscriptionLabel(s);
    const door = eventConditionDoor(s);
    return [
      ...glue,
      door ? (
        <button
          key={`sub${i}`}
          type="button"
          class="event-wait-subscription-filter"
          data-role="event-wait-condition"
          aria-label={door.label}
          data-tooltip={door.label}
          onClick={door.open}
        >
          {label}
        </button>
      ) : (
        <span key={`sub${i}`}>{label}</span>
      ),
    ];
  });
}

/** One live subscription. The countdown lives in component-local state, never
 *  in a signal: a per-second store write would re-flush `threadMap` (and
 *  therefore every subscribed component, most expensively `ChatExchange`) once
 *  a second for every subscribed thread. */
function EventWaitRow({ threadId, wait }: { threadId: string; wait: EventWaitSummary }) {
  const [now, setNow] = useState(() => Date.now());
  const stopping = useSignal(false);

  useEffect(() => {
    const handle = setInterval(() => setNow(Date.now()), COUNTDOWN_TICK_MS);
    return () => clearInterval(handle);
  }, []);

  const onStop = async () => {
    if (stopping.value) return;
    stopping.value = true;
    try {
      // Success removes the row via the SSE `EventWaitCanceled`, which the
      // store projection filters out of `meta.liveEventWaits`.
      await cancelThreadEventWait(threadId, wait.wait_id);
    } catch (e) {
      showToast('Could not stop waiting: ' + errorDetail(e), 'error');
      stopping.value = false;
    }
  };

  return (
    <li class="event-wait-item">
      <div class="event-wait-main">
        <span class="event-wait-reason">{wait.reason}</span>
        <code class="event-wait-subscription">{subscriptionLine(wait.on)}</code>
      </div>
      <div class="event-wait-foot">
        <span class="event-wait-meta">
          <span class="event-wait-countdown">
            {formatRemaining(secondsRemaining(wait.expires_at, now))}
          </span>
        </span>
        <button
          type="button"
          class="action-btn action-btn-danger event-wait-stop"
          data-role="event-wait-stop"
          disabled={stopping.value}
          onClick={onStop}
        >
          {stopping.value ? 'Stopping…' : 'Stop waiting'}
        </button>
      </div>
    </li>
  );
}
