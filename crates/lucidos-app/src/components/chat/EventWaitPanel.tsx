import { useSignal } from '@preact/signals';
import { useEffect, useRef, useState } from 'preact/hooks';
import { cancelThreadEventWait } from '../../api/client';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { focusedThreadId, showToast, threadMap } from '../../store/store';
import type { EventWaitSummary } from '../../store/thread-events';
import { describeWaitSubscription, formatRemaining, secondsRemaining } from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { CloseIcon, EventWaitClockIcon } from '../shared/icons';
import { Overlay } from '../shared/Overlay';

/** How often the countdown re-renders. One second is the granularity the text
 *  actually shows below a minute, and above that the text is coarse enough that
 *  the extra ticks are invisible; a slower interval would make a `4m 12s` row
 *  visibly stutter. */
const COUNTDOWN_TICK_MS = 1000;

/** The **subscription indicator**: what this thread is watching for, readable
 *  at any time without scrolling the transcript.
 *
 *  This is the primary surface for event waits, and the transcript card is the
 *  historical record. The split exists because a wait survives an interruption
 *  (S6b): a thread can be watching for something while reading as `idle` and
 *  showing an ordinary reply, so "is anything subscribed" cannot be answered by
 *  the status dot or by wherever the transcript happens to be scrolled.
 *
 *  Reads `meta.liveEventWaits` (projected in `handleEvent`) so the render path
 *  is O(1), exactly like `TodoListIndicator` next to it. */
export function EventWaitIndicator() {
  const open = useSignal(false);
  // useState (not useRef) so the dismiss hook re-runs once the button mounts
  // and we have a real anchor to exclude from the outside-click test.
  const [anchorEl, setAnchorEl] = useState<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const id = focusedThreadId.value;
  const waits = id ? threadMap.value.get(id)?.meta.liveEventWaits ?? [] : [];
  const isOpen = open.value && waits.length > 0;
  // Clamp into the thread pane that owns the indicator: on desktop that keeps
  // the panel out of the content pane, and on a phone the pane IS the viewport,
  // so the same clamp is what stops it running off the right edge. Passing a
  // null anchor while closed makes the hook recompute from scratch on open,
  // rather than reusing coordinates measured before the last scroll.
  const pos = useAnchoredPosition(isOpen ? anchorEl : null, panelRef, '.thread-pane');

  return (
    <>
      {eventWaitIndicatorBody({
        waits,
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
        onClose={() => (open.value = false)}
        anchor={anchorEl}
        backdrop={false}
        portal
        panelClass="prompt-bar-popover event-wait-panel"
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
        dataRole="event-wait-panel"
        panelRef={panelRef}
      >
        {isOpen
          ? eventWaitPanelBody({
              threadId: id ?? '',
              waits,
              onClose: () => (open.value = false),
            })
          : null}
      </Overlay>
    </>
  );
}

export function eventWaitIndicatorBody({
  waits,
  onClick,
  buttonRef,
}: {
  waits: EventWaitSummary[];
  onClick: () => void;
  buttonRef?: (el: HTMLButtonElement | null) => void;
}) {
  if (waits.length === 0) return null;
  const label =
    waits.length === 1
      ? waits[0].reason
      : `${waits.length} subscriptions`;
  return (
    <button
      type="button"
      class="icon-btn header-icon"
      data-role="event-wait-indicator"
      data-tooltip={label}
      aria-label={`Watching for an event: ${label}. Click to expand.`}
      onClick={onClick}
      data-row-item
      ref={buttonRef}
    >
      <EventWaitClockIcon />
    </button>
  );
}

/** One panel row per live wait. The countdown lives in component-local state,
 *  never in a signal: a per-second store write would re-flush `threadMap` (and
 *  therefore every subscribed component, most expensively `ChatExchange`) once
 *  a second for every subscribed thread.
 *
 *  Returns the panel's CONTENTS, not its box: the box is the `<Overlay>` panel
 *  itself, which is what `useAnchoredPosition` measures and positions. */
export function eventWaitPanelBody({
  threadId,
  waits,
  onClose,
}: {
  threadId: string;
  waits: EventWaitSummary[];
  onClose: () => void;
}) {
  return (
    <>
      <div class="prompt-bar-popover-head">
        <span class="prompt-bar-popover-title">Subscriptions</span>
        <button
          type="button"
          class="icon-btn prompt-bar-popover-close"
          aria-label="Close subscriptions"
          onClick={onClose}
        >
          <CloseIcon />
        </button>
      </div>
      <div class="prompt-bar-popover-body">
        <ul class="event-wait-panel-list">
          {waits.map((wait) => (
            <EventWaitRow key={wait.wait_id} threadId={threadId} wait={wait} />
          ))}
        </ul>
      </div>
    </>
  );
}

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
    <li class="event-wait-panel-row">
      <div class="event-wait-panel-main">
        <span class="event-wait-panel-reason">{wait.reason}</span>
        <code class="event-wait-panel-subscription">{describeWaitSubscription(wait.on)}</code>
      </div>
      <div class="event-wait-panel-foot">
        <span class="event-wait-panel-meta">
          <span class="event-wait-panel-countdown">
            {formatRemaining(secondsRemaining(wait.expires_at, now))}
          </span>
        </span>
        <button
          type="button"
          class="action-btn action-btn-danger event-wait-panel-stop"
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
