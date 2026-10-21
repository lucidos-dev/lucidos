import { useEffect, useState } from 'preact/hooks';
import { fetchCacheWarmth } from '../../api/client';
import type { CacheKeepAliveStatus } from '../../generated/thread-event-wire';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { effectiveThreadStatus, isMidTurn, isRenderedThreadIdle, threadMap } from '../../store/store';
import type { ThreadState } from '../../store/thread-events';
import { secondsRemaining } from '../../store/thread-events';
import { CACHE_KEEP_ALIVE_EXPLAINER } from '../../utils/engineEventExplainers';
import { formatShortTime } from '../../utils/formatTime';
import { formatTokens } from '../../utils/formatTokens';
import { openCacheWarmLineSetting } from '../../store/actions/menu';
import { Explainer } from '../shared/Explainer';
import { eventNameChip, eventRowBody } from './EventRow';

type Warming = Extract<CacheKeepAliveStatus, { state: 'warming' }>;
type Stopped = Extract<CacheKeepAliveStatus, { state: 'stopped' }>;
type NotKept = Extract<CacheKeepAliveStatus, { state: 'not_kept' }>;

/** The thread's *cache keep-alive status* while it waits, or null. A running
 *  thread reads its cache anyway, so a status left over from its wait is not
 *  shown. */
export function shownKeepAliveStatus(thread: ThreadState | undefined): CacheKeepAliveStatus | null {
  const status = thread?.meta.cacheKeepAliveStatus;
  if (!status || isMidTurn(effectiveThreadStatus(thread))) return null;
  return status;
}

function usd(cost: number): string {
  return `$${cost.toFixed(2)}`;
}

function pings(count: number): string {
  return `${count} ping${count === 1 ? '' : 's'}`;
}

function clock(iso: string): string {
  return formatShortTime(new Date(iso));
}

/** The context a wake-up would reread, as a phrase after "reread". */
function rereadWhat(contextTokens: number): string {
  return contextTokens > 0 ? `${formatTokens(contextTokens)} tokens` : 'the whole conversation';
}

/** How often a ping goes out, in words: `4 minutes`, `55 minutes`. */
export function beatIntervalText(seconds: number): string {
  const minutes = Math.max(1, Math.round(seconds / 60));
  return `${minutes} minute${minutes === 1 ? '' : 's'}`;
}

/** The divider's countdown: `next in 3m`, `next in 40s`, or `next ping due`. */
export function nextBeatText(seconds: number): string {
  if (seconds <= 0) return 'next ping due';
  return seconds < 60 ? `next in ${seconds}s` : `next in ${Math.ceil(seconds / 60)}m`;
}

/** What the warm cache saves a reread for: a sub-thread's report, or the
 *  wake-up of a thread waiting on an event. */
export function wakerText(subThreadCount: number): string {
  if (subThreadCount === 0) return 'the wake-up';
  return subThreadCount === 1 ? "the sub-thread's report" : "a sub-thread's report";
}

/** The panel section's two lines for one status. */
export function keepAliveSectionText(
  status: CacheKeepAliveStatus,
  subThreadCount: number,
): { line: string; detail: string } {
  switch (status.state) {
    case 'warming':
      return warmingText(status, subThreadCount);
    case 'stopped':
      return stoppedText(status, subThreadCount);
    case 'not_kept':
      return notKeptText(status);
  }
}

function warmingText(s: Warming, subThreadCount: number) {
  return {
    line: `Kept warm: ${s.beats} of ${pings(s.budget_beats)}, ${usd(s.cost_usd)} so far`,
    detail: `A ping every ${beatIntervalText(s.beat_every_secs)}, so ${wakerText(subThreadCount)} doesn't pay to reread ${rereadWhat(s.context_tokens)}. Stops around ${clock(s.give_up_at)}.`,
  };
}

function stoppedText(s: Stopped, subThreadCount: number) {
  return {
    line: `Stopped keeping the cache warm at ${clock(s.at)}: past break-even.`,
    detail: `${pings(s.beats)}, ${usd(s.cost_usd)}. More pings would cost more than rereading ${rereadWhat(s.context_tokens)} once, so ${wakerText(subThreadCount)} pays that reread instead.`,
  };
}

function notKeptText(s: NotKept) {
  switch (s.reason) {
    case 'no_wake_children':
      return {
        line: "Not keeping the cache warm: these sub-threads won't wake this thread.",
        detail: 'They finish without waking it, so nothing would read a warm cache.',
      };
    case 'children_awaiting_user':
      return {
        line: 'Not keeping the cache warm: the sub-threads are waiting on you.',
        detail: 'Your answer can take any time. Pings start again if it comes while the cache is still warm.',
      };
  }
}

/** The waiting panel's "Prompt cache" section. It is always labelled, since it
 *  says something different in kind from the lists above it. */
export function KeepAliveSection({ status, subThreadCount }: { status: CacheKeepAliveStatus; subThreadCount: number }) {
  const { line, detail } = keepAliveSectionText(status, subThreadCount);
  return (
    <section class="waiting-panel-section" data-role="waiting-prompt-cache" data-state={status.state}>
      <span class="waiting-panel-section-label">Prompt cache</span>
      <div class="waiting-panel-cache-line">{line}</div>
      <div class="waiting-panel-cache-detail">{detail}</div>
      {status.state === 'warming' ? (
        <div class="waiting-panel-cache-meter" aria-hidden="true">
          <span style={{ width: `${Math.min(100, (status.beats / Math.max(1, status.budget_beats)) * 100)}%` }} />
        </div>
      ) : null}
    </section>
  );
}

/** How often the countdown re-renders. Under a minute the text counts seconds. */
const COUNTDOWN_TICK_MS = 1000;

/** Whether `status` is the run the thread's wait is in now, so its card may
 *  count down to the next ping. A card of a closed run, or one read while the
 *  thread runs, has no next ping. `since` names the wait a run keeps warm. */
function isOpenRun(status: Warming, shown: CacheKeepAliveStatus | null): boolean {
  return shown?.state === 'warming' && shown.since === status.since;
}

/** The card's tail after the ping count and cost. */
function keepAliveTail(status: Warming | Stopped, open: boolean) {
  if (status.state === 'stopped') return ` · stopped at ${clock(status.at)}`;
  if (!open) return null;
  return <> · {status.next_beat_at ? <NextBeat at={status.next_beat_at} /> : 'first ping soon'}</>;
}

/** The transcript card of one *keep-alive run*: the pings the engine sent to
 *  keep a waiting thread's cache warm. The card stays after the run ends. It
 *  reads the thread itself, for it must tick only while its run is open. */
export function KeepAliveRow({ threadId, status }: { threadId: string; status: CacheKeepAliveStatus }) {
  if (status.state === 'not_kept') return null;
  const open = status.state === 'warming' && isOpenRun(status, shownKeepAliveStatus(threadMap.value.get(threadId)));
  const state = status.state === 'stopped' ? 'Stopped' : open ? 'Warming' : 'Ended';
  return eventRowBody({
    kind: 'cache',
    state: status.state,
    role: 'cache-keep-alive',
    subject: eventNameChip({
      kind: 'chip',
      name: 'CacheKeepAliveStatusChanged',
      label: 'cache kept warm',
      meaning: CACHE_KEEP_ALIVE_EXPLAINER,
      sentenceStart: true,
    }),
    stateLabel: state,
    tone: open ? 'live' : status.state === 'stopped' ? 'halted' : 'none',
    detail: <>{`${pings(status.beats)} · ${usd(status.cost_usd)}`}{keepAliveTail(status, open)}</>,
  });
}

/** A local clock that re-renders once a second, for a countdown. With `until`
 *  it stops ticking once that time has passed. */
function useNow(until = Infinity): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const handle = setInterval(() => {
      const tick = Date.now();
      setNow(tick);
      if (tick >= until) clearInterval(handle);
    }, COUNTDOWN_TICK_MS);
    return () => clearInterval(handle);
  }, [until]);
  return now;
}

/** The countdown, on a local clock over the pushed `next_beat_at`. The next
 *  status event brings the next one. */
function NextBeat({ at }: { at: string }) {
  return <>{nextBeatText(secondsRemaining(at, useNow()))}</>;
}

/** The cache-warm line's countdown: `4m left`, or `40s left` under a minute. */
export function cacheWarmText(seconds: number): string {
  return seconds < 60 ? `${seconds}s left` : `${Math.ceil(seconds / 60)}m left`;
}

/** Whether the cache-warm line may show: the thread is idle or parked on a
 *  question with no answer or follow-up in flight, and no keep-alive run is
 *  counting down, which says more. */
export function cacheWarmLineEligible(thread: ThreadState): boolean {
  return isRenderedThreadIdle(thread) && shownKeepAliveStatus(thread) === null;
}

/** The quiet line that closes the feed of a thread waiting on the user while
 *  its prompt cache is warm. That is an idle thread, or one parked on a
 *  question. The engine says how long is left. A read that says cold draws
 *  nothing. A read that fails draws nothing either, and the next status or
 *  event change reads again.
 *
 *  It takes values derived from the thread, never the thread itself. The SSE
 *  path mutates a thread in place, and `@preact/signals` skips a render whose
 *  props are all identical. Handed the thread, the line kept counting down
 *  through a running turn. */
export function CacheWarmLine({ threadId, eligible, eventCount }: { threadId: string; eligible: boolean; eventCount: number }) {
  const { loadable } = useLoadableFetch(
    async () => {
      if (!eligible) return null;
      try {
        const { remaining_secs } = await fetchCacheWarmth(threadId);
        return remaining_secs === null ? null : Date.now() + remaining_secs * 1000;
      } catch (e) {
        // Best-effort hint the user never asked for: a toast on every failed
        // read would be noise. The line is absent, and the next status or
        // event change reads again.
        console.warn(`[CacheWarmLine] cache-warmth read failed for thread "${threadId}"`, e);
        return null;
      }
    },
    [threadId, eligible, eventCount],
    // An event while idle re-reads the same thing. Leaving idle clears the
    // figure, since the ineligible pass loads null.
    { keepLoadedWhileRefetching: true },
  );
  if (!eligible || loadable.status !== 'loaded' || loadable.data === null) return null;
  return <WarmCountdown until={loadable.data} />;
}

function WarmCountdown({ until }: { until: number }) {
  const seconds = Math.ceil((until - useNow(until)) / 1000);
  if (seconds <= 0) return null;
  return (
    <div class="cache-keep-alive-divider" data-role="cache-warm">
      <span>
        <strong>Cache warm</strong>
        {` · ${cacheWarmText(seconds)}`}
        <Explainer title="Cache warm">
          {(close) => (
            <>
              <p>
                The model still holds this thread's context in its prompt cache. Reply
                before the time runs out and the next turn reads it cheaply, instead of
                paying to reread the whole conversation.
              </p>
              <p>
                You can hide this line in{' '}
                <button
                  class="accent-link"
                  onClick={() => {
                    close();
                    openCacheWarmLineSetting();
                  }}
                >
                  Settings → System → Debugging
                </button>
                .
              </p>
            </>
          )}
        </Explainer>
      </span>
    </div>
  );
}
