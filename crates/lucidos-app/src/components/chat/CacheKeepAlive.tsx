import { useEffect, useState } from 'preact/hooks';
import { fetchCacheWarmth } from '../../api/client';
import type { WarmLineKeepAlive } from '../../api/client/successor';
import type { CacheKeepAliveStatus } from '../../generated/thread-event-wire';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { effectiveThreadStatus, isMidTurn, isRenderedThreadIdle, threadMap } from '../../store/store';
import type { ThreadState } from '../../store/thread-events';
import { secondsRemaining } from '../../store/thread-events';
import { CACHE_KEEP_ALIVE_EXPLAINER } from '../../utils/engineEventExplainers';
import { formatShortTime } from '../../utils/formatTime';
import { formatTokens } from '../../utils/formatTokens';
import { openCacheKeepAliveSetting, openCacheWarmLineSetting } from '../../store/actions/menu';
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

type WarmLineKept = Extract<WarmLineKeepAlive, { state: 'kept' }>;
type WarmLineStopped = Extract<WarmLineKeepAlive, { state: 'stopped' }>;
type WarmLineNotKept = Extract<WarmLineKeepAlive, { state: 'not_kept' }>;

/** One read of the cache-warm line: when the cache goes cold, and what the
 *  keep-alive does meanwhile. */
interface WarmRead {
  coldAt: number;
  keepAlive: WarmLineKeepAlive;
}

/** What the pill says now. While copy beats run it names the give-up time. Once
 *  that has passed, or when nothing beats, it counts down to cold. */
export type WarmLineView =
  | { kind: 'kept'; keepAlive: WarmLineKept }
  | { kind: 'countdown'; keepAlive: WarmLineStopped | WarmLineNotKept; seconds: number };

export function warmLineView({ coldAt, keepAlive }: WarmRead, now: number): WarmLineView | null {
  if (keepAlive.state === 'kept') {
    if (now < Date.parse(keepAlive.until)) return { kind: 'kept', keepAlive };
    const { beats, cost_usd, context_tokens, until } = keepAlive;
    return countdown({ state: 'stopped', at: until, beats, cost_usd, context_tokens }, coldAt, now);
  }
  return countdown(keepAlive, coldAt, now);
}

function countdown(keepAlive: WarmLineStopped | WarmLineNotKept, coldAt: number, now: number): WarmLineView | null {
  const seconds = Math.ceil((coldAt - now) / 1000);
  return seconds > 0 ? { kind: 'countdown', keepAlive, seconds } : null;
}

/** The quiet line that closes the feed of a thread waiting on the user while
 *  its prompt cache is warm. That is an idle thread, or one parked on a
 *  question. The engine says how long is left and what the keep-alive does. A
 *  read that says cold draws nothing. A read that fails draws nothing either,
 *  and the next status or event change reads again. A copy beat is a
 *  `ContextCaptured` on the thread, so it reads again too.
 *
 *  It takes values derived from the thread, never the thread itself. The SSE
 *  path mutates a thread in place, and `@preact/signals` skips a render whose
 *  props are all identical. Handed the thread, the line kept counting down
 *  through a running turn. */
export function CacheWarmLine({ threadId, eligible, eventCount }: { threadId: string; eligible: boolean; eventCount: number }) {
  const { loadable } = useLoadableFetch(
    async (): Promise<WarmRead | null> => {
      if (!eligible) return null;
      try {
        const warmth = await fetchCacheWarmth(threadId);
        if (warmth.remaining_secs === null) return null;
        return { coldAt: Date.now() + warmth.remaining_secs * 1000, keepAlive: warmth.keep_alive };
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
  return <WarmLinePill read={loadable.data} />;
}

function WarmLinePill({ read }: { read: WarmRead }) {
  const keptUntil = read.keepAlive.state === 'kept' ? Date.parse(read.keepAlive.until) : 0;
  const view = warmLineView(read, useNow(Math.max(read.coldAt, keptUntil)));
  if (view === null) return null;
  const kept = view.kind === 'kept';
  const title = kept ? 'Cache kept warm' : 'Cache warm';
  return (
    <div class="cache-keep-alive-divider" data-role="cache-warm" data-state={view.keepAlive.state}>
      <span>
        <strong>{title}</strong>
        {kept ? ` · until ${clock(view.keepAlive.until)}` : ` · ${cacheWarmText(view.seconds)}`}
        <Explainer title={title}>
          {(close) => (
            <>
              <WarmLineExplainer keepAlive={view.keepAlive} />
              <WarmLineSettingLinks keepAlive={view.keepAlive} close={close} />
            </>
          )}
        </Explainer>
      </span>
    </div>
  );
}

const WARM_CACHE_TEXT =
  "The model still holds this thread's context in its prompt cache. Reply before the time runs out and the next turn reads it cheaply";

/** The explainer's text: what the cache is, then what the keep-alive does. */
function WarmLineExplainer({ keepAlive }: { keepAlive: WarmLineView['keepAlive'] }) {
  switch (keepAlive.state) {
    case 'kept':
      return (
        <>
          <p>
            This thread waits for your answer, and Lucidos keeps its prompt cache warm meanwhile. Just
            before the cache would expire, it asks a throwaway copy of the session for one word. The card
            and the conversation never see it.
          </p>
          <p>
            {`So far: ${pings(keepAlive.beats)}, ${usd(keepAlive.cost_usd)}. It stops around ${clock(keepAlive.until)}. A reply after that pays to reread ${rereadWhat(keepAlive.context_tokens)} once.`}
          </p>
        </>
      );
    case 'stopped':
      return (
        <>
          <p>{`${WARM_CACHE_TEXT}.`}</p>
          <p>
            {`Lucidos stopped keeping it warm at ${clock(keepAlive.at)}: past break-even. ${pings(keepAlive.beats)}, ${usd(keepAlive.cost_usd)}. More pings would cost more than rereading ${rereadWhat(keepAlive.context_tokens)} once.`}
          </p>
        </>
      );
    case 'not_kept':
      return (
        <>
          <p>{`${WARM_CACHE_TEXT}, instead of paying to reread the whole conversation.`}</p>
          <p>{`Nothing keeps it warm while you are away: ${NOT_KEPT_REASON[keepAlive.reason]}`}</p>
        </>
      );
  }
}

/** Each reason ends on a full stop. */
const NOT_KEPT_REASON: Record<WarmLineNotKept['reason'], string> = {
  off: 'the cache keep-alive is off.',
  not_claude_code: 'Lucidos only keeps Claude Code sessions warm.',
  session_restarted: 'this session outlived a restart, so a copy of it would not read the cache.',
  not_parked: 'this thread is not waiting on a question or permission card.',
};

function SettingLink({ label, close, open }: { label: string; close: () => void; open: () => void }) {
  return (
    <button
      class="accent-link"
      onClick={() => {
        close();
        open();
      }}
    >
      {label}
    </button>
  );
}

const KEEP_ALIVE_SETTING = 'Settings → Coding Agents → Cache keep-alive';

/** The paragraphs linking the settings behind the line: the keep-alive, and the
 *  switch that hides the line. Each ends on its link, with no full stop to wrap. */
function WarmLineSettingLinks({ keepAlive, close }: { keepAlive: WarmLineView['keepAlive']; close: () => void }) {
  const keepAliveApplies = !(keepAlive.state === 'not_kept' && keepAlive.reason === 'not_claude_code');
  return (
    <>
      {keepAliveApplies && (
        <p>
          {keepAliveSettingLead(keepAlive)}{' '}
          <SettingLink label={KEEP_ALIVE_SETTING} close={close} open={openCacheKeepAliveSetting} />
        </p>
      )}
      <p>
        Hide this line in{' '}
        <SettingLink label="Settings → System → Debugging" close={close} open={openCacheWarmLineSetting} />
      </p>
    </>
  );
}

/** What the keep-alive setting changes, as the lead-in to its link. */
function keepAliveSettingLead(keepAlive: WarmLineView['keepAlive']): string {
  switch (keepAlive.state) {
    case 'kept':
      return 'Change how long it keeps warm in';
    case 'stopped':
      return 'Change when it gives up in';
    case 'not_kept':
      return keepAlive.reason === 'off' ? 'Turn it on in' : 'Set up the keep-alive in';
  }
}
