import { signal, useSignal } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { cancelThreadEventWait } from '../../api/client';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import {
  eventConditionDoor,
  focusedThreadId,
  showToast,
  threadMap,
} from '../../store/store';
import type { EventSubscription, EventWaitSummary, ThreadState } from '../../store/thread-events';
import {
  awaitedSubject,
  displayedWaitDeadline,
  formatRemaining,
  groupSubscriptions,
  plainEventMeaning,
  secondsRemaining,
  waitSubscriptionLabel,
} from '../../store/thread-events';
import { errorDetail } from '../../utils/errorDetail';
import { viewportIsMobile } from '../../utils/viewport';
import { EventWaitClockIcon } from '../shared/icons';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { useEscapeStep } from '../../hooks/useEscapeStep';
import type { EventCondition } from '../../store/store';
import { eventConditionBody, eventConditionTitle } from './eventConditionBody';
import { ThreadStatusIcon } from '../shared/ThreadStatusIcon';
import { threadVisualStatus } from '../shared/threadVisualStatus';
import type { HeaderActionSpec } from '../layout/headerActions';
import type { CacheKeepAliveStatus } from '../../generated/thread-event-wire';
import { KeepAliveSection, shownKeepAliveStatus } from './CacheKeepAlive';

const waitingPanelAnchor = signal<HTMLElement | null>(null);

export const waitingPanelCondition = signal<EventCondition | null>(null);

function openWaitingPanel(anchor: HTMLElement | null): void {
  waitingPanelCondition.value = null;
  waitingPanelAnchor.value = anchor;
}

export function closeWaitingPanel(): void {
  waitingPanelAnchor.value = null;
  waitingPanelCondition.value = null;
}

function backToWaitingList(): void {
  waitingPanelCondition.value = null;
}

const COUNTDOWN_TICK_MS = 1000;

export interface SubThreadWait {
  threads: ThreadState[];
  unresolved: number;
}

const NO_SUB_THREADS: SubThreadWait = { threads: [], unresolved: 0 };

export function awaitedSubThreads(
  parentId: string,
  threads: ReadonlyMap<string, ThreadState>,
  awaitedCount: number,
): SubThreadWait {
  if (awaitedCount <= 0) return NO_SUB_THREADS;
  const found: ThreadState[] = [];
  for (const thread of threads.values()) {
    if (thread.meta.parentThreadId !== parentId || !thread.meta.awaitedByParent) continue;
    found.push(thread);
  }
  found.sort((a, b) => a.meta.createdAt.localeCompare(b.meta.createdAt));
  return { threads: found, unresolved: Math.max(0, awaitedCount - found.length) };
}

export function waitingIndicatorAction(): HeaderActionSpec | null {
  const id = focusedThreadId.value;
  const meta = id ? threadMap.value.get(id)?.meta : undefined;
  const waits = meta?.liveEventWaits ?? [];
  const subThreads = id
    ? awaitedSubThreads(id, threadMap.value, meta?.awaitedChildrenCount ?? 0)
    : NO_SUB_THREADS;
  const summary = waitingIndicatorSummary(waits, subThreads);
  if (!summary) return null;
  return {
    key: 'waiting-indicator',
    dataRole: 'waiting-indicator',
    label: summary.menuLabel,
    tooltip: summary.tooltip,
    icon: () => <EventWaitClockIcon />,
    render: (attrs) => waitingIndicatorBody({
      waits,
      subThreads,
      attrs,
      onClick: (e) => {
        if (waitingPanelAnchor.value) closeWaitingPanel();
        else openWaitingPanel(e.currentTarget as HTMLElement);
      },
    }),
    onMenuClick: openWaitingPanel,
  };
}

export function WaitingPanelHost() {
  const panelRef = useRef<HTMLDivElement>(null);
  const id = focusedThreadId.value;
  const thread = id ? threadMap.value.get(id) : undefined;
  const meta = thread?.meta;
  const waits = meta?.liveEventWaits ?? [];
  const anchor = waitingPanelAnchor.value;
  const subThreads = anchor && id
    ? awaitedSubThreads(id, threadMap.value, meta?.awaitedChildrenCount ?? 0)
    : NO_SUB_THREADS;
  const isOpen = anchor !== null && isWaitingForAnything(waits, subThreads);
  const pos = useAnchoredPosition(
    isOpen ? anchor : null,
    panelRef,
    '.thread-pane',
    viewportIsMobile.value ? 'container-center' : 'start',
  );
  const drilled = waitingPanelCondition.value;
  const condition = drilled
    && waits.some((w) => w.on.some((sub) => sub.event_type === drilled.eventType))
    ? drilled
    : null;
  useLayoutEffect(() => {
    if (drilled && !condition) backToWaitingList();
  }, [drilled, condition]);

  useEscapeStep(isOpen && condition ? backToWaitingList : null);

  const drilledFrom = useRef<string | null>(null);
  useLayoutEffect(() => {
    const panel = panelRef.current;
    const from = drilledFrom.current;
    drilledFrom.current = condition?.eventType ?? null;
    if (!panel || from === drilledFrom.current) return;
    const target = condition
      ? panel.querySelector<HTMLElement>('[data-role="surface-back"]')
      : panel.querySelector<HTMLElement>(`[data-role="event-wait-condition"][data-event-type="${CSS.escape(from ?? '')}"]`);
    target?.focus();
  }, [condition]);

  return (
    <Overlay
      open={isOpen}
      onClose={closeWaitingPanel}
      anchor={anchor}
      backdrop={false}
      portal
      panelClass="surface anchored-popover waiting-panel"
      panelStyle={pos
        ? {
            top: `${pos.top}px`,
            left: `${pos.left}px`,
            '--anchored-popover-fit': `${pos.maxWidth}px`,
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
            keepAlive: shownKeepAliveStatus(thread),
            condition,
            onBack: backToWaitingList,
            onClose: closeWaitingPanel,
          })
        : null}
    </Overlay>
  );
}

function isWaitingForAnything(waits: EventWaitSummary[], subThreads: SubThreadWait): boolean {
  return waits.length > 0 || subThreadCount(subThreads) > 0;
}

function subThreadCount(subThreads: SubThreadWait): number {
  return subThreads.threads.length + subThreads.unresolved;
}

export function waitingIndicatorSummary(
  waits: EventWaitSummary[],
  subThreads: SubThreadWait,
): { tooltip: string; ariaLabel: string; menuLabel: string } | null {
  if (!isWaitingForAnything(waits, subThreads)) return null;
  const children = subThreadCount(subThreads);
  const soleReason = waits.length === 1 && children === 0 ? waits[0].reason : null;
  const counted = [
    waits.length > 0 ? plural(waits.length, 'event') : null,
    children > 0 ? plural(children, 'sub-thread') : null,
  ]
    .filter((part): part is string => part !== null)
    .join(', ');
  const spoken = soleReason ? awaitedSubject(soleReason) : counted;
  return {
    tooltip: soleReason ?? counted,
    ariaLabel: `Waiting for ${spoken}. Click to expand.`,
    menuLabel: `Waiting for ${spoken}`,
  };
}

export function waitingIndicatorBody({
  waits,
  subThreads,
  onClick,
  attrs,
}: {
  waits: EventWaitSummary[];
  subThreads: SubThreadWait;
  onClick: (e: MouseEvent) => void;
  attrs?: Record<string, string>;
}) {
  const summary = waitingIndicatorSummary(waits, subThreads);
  if (!summary) return null;
  return (
    <button
      {...attrs}
      type="button"
      class="icon-btn header-icon"
      data-role="waiting-indicator"
      data-tooltip={summary.tooltip}
      aria-label={summary.ariaLabel}
      onClick={onClick}
      data-row-item
    >
      <EventWaitClockIcon />
    </button>
  );
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? '' : 's'}`;
}

export function waitingPanelBody({
  threadId,
  waits,
  subThreads,
  keepAlive = null,
  condition = null,
  onBack = backToWaitingList,
  onClose,
}: {
  threadId: string;
  waits: EventWaitSummary[];
  subThreads: SubThreadWait;
  keepAlive?: CacheKeepAliveStatus | null;
  condition?: EventCondition | null;
  onBack?: () => void;
  onClose: () => void;
}) {
  const closeLabel = 'Close what this thread is waiting for';
  if (condition) {
    return (
      <>
        <SurfaceHead
          back={{ label: 'Waiting for', onClick: onBack }}
          title={eventConditionTitle(condition)}
          onClose={onClose}
          closeLabel={closeLabel}
        />
        <div
          class="anchored-popover-body"
          data-role="waiting-condition"
          data-surface-step={`condition:${condition.eventType}`}
        >
          {eventConditionBody(condition)}
        </div>
      </>
    );
  }
  const labelled = waits.length > 0 && subThreadCount(subThreads) > 0;
  return (
    <>
      <SurfaceHead title="Waiting for" onClose={onClose} closeLabel={closeLabel} />
      <div class="anchored-popover-body" data-surface-step="list">
        {waits.length > 0 ? (
          <section class="waiting-panel-section" data-role="waiting-subscriptions">
            {labelled ? <span class="waiting-panel-section-label">Events</span> : null}
            <ul class="event-wait-list">
              {waits.map((wait) => (
                <EventWaitRow key={wait.wait_id} threadId={threadId} wait={wait} />
              ))}
            </ul>
          </section>
        ) : null}
        {subThreadCount(subThreads) > 0 ? (
          <section class="waiting-panel-section" data-role="waiting-sub-threads">
            {labelled ? <span class="waiting-panel-section-label">Sub-threads</span> : null}
            <ul class="waiting-panel-child-list">
              {subThreads.threads.map((child) => (
                <SubThreadRow key={child.meta.id} child={child} onOpen={onClose} />
              ))}
              {subThreads.unresolved > 0 ? (
                <li class="waiting-panel-child-more" data-role="waiting-sub-threads-more">
                  {`and ${subThreads.unresolved} more`}
                </li>
              ) : null}
            </ul>
          </section>
        ) : null}
        {keepAlive ? (
          <KeepAliveSection threadId={threadId} status={keepAlive} />
        ) : null}
      </div>
    </>
  );
}

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

export function subscriptionLine(on: EventSubscription[]): ComponentChildren[] {
  return groupSubscriptions(on).flatMap((g, i) => {
    const glue = [<span key={`glue${i}`}>{i === 0 ? 'watching for ' : ' or '}</span>];
    const label = waitSubscriptionLabel(g);
    const door = eventConditionDoor(g);
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
          data-event-type={g.event_type}
          onClick={() => { waitingPanelCondition.value = door.condition; }}
        >
          {label}
        </button>
      ) : (
        <span key={`sub${i}`} data-tooltip={plainEventMeaning(g.event_type)}>{label}</span>
      ),
    ];
  });
}

export function countdownText(seconds: number): string {
  return seconds > 0 ? `${formatRemaining(seconds)} left` : formatRemaining(seconds);
}

function EventWaitRow({ threadId, wait }: { threadId: string; wait: EventWaitSummary }) {
  const [now, setNow] = useState(() => Date.now());
  const stopping = useSignal(false);
  const deadline = displayedWaitDeadline(wait);

  useEffect(() => {
    const handle = setInterval(() => setNow(Date.now()), COUNTDOWN_TICK_MS);
    return () => clearInterval(handle);
  }, []);

  const onStop = async () => {
    if (stopping.value) return;
    stopping.value = true;
    try {
      await cancelThreadEventWait(threadId, wait.wait_id);
    } catch (e) {
      showToast('Could not stop waiting: ' + errorDetail(e), 'error');
      stopping.value = false;
    }
  };

  return (
    <li class="event-wait-item">
      <div class="event-wait-main">
        <span class="event-wait-reason">{awaitedSubject(wait.reason)}</span>
        <code class="event-wait-subscription">{subscriptionLine(wait.on)}</code>
      </div>
      <div class="event-wait-foot">
        <span class="event-wait-meta">
          <span class="event-wait-countdown">
            {deadline && countdownText(secondsRemaining(deadline, now))}
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
