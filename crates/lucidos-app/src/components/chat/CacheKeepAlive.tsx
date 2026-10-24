import { useEffect, useState } from 'preact/hooks';
import { fetchCacheWarmth, switchCacheKeepAlive } from '../../api/client';
import type { CacheKeepAliveNotKeptReason, CacheKeepAliveStatus } from '../../generated/thread-event-wire';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { effectiveThreadStatus, isMidTurn, isRenderedThreadIdle, showToast, threadMap } from '../../store/store';
import type { ThreadState } from '../../store/thread-events';
import { formatShortTime } from '../../utils/formatTime';
import { formatTokens } from '../../utils/formatTokens';
import { openCacheKeepAliveSetting, openCacheWarmLineSetting } from '../../store/actions/menu';
import { Explainer } from '../shared/Explainer';
import { eventNameChip, eventRowBody } from './EventRow';

type Status<S extends CacheKeepAliveStatus['state']> = Extract<CacheKeepAliveStatus, { state: S }>;

/** The thread's *cache keep-alive status* while it waits, or null. A running
 *  thread reads its cache anyway, so a status left over from its wait is not
 *  shown, and an `ended` one is closed. */
export function shownKeepAliveStatus(thread: ThreadState | undefined): CacheKeepAliveStatus | null {
  const status = thread?.meta.cacheKeepAliveStatus;
  if (!status || status.state === 'ended' || isMidTurn(effectiveThreadStatus(thread))) return null;
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

/** The state as a word, the one set every surface reads. */
const STATE_WORD: Record<CacheKeepAliveStatus['state'], string> = {
  warming: 'warming',
  stopped: 'stopped',
  ended: 'ended',
  turned_off: 'turned off',
  not_kept: 'not kept',
};

/** Each reason ends on a full stop. */
const NOT_KEPT_REASON: Record<CacheKeepAliveNotKeptReason, string> = {
  codex: 'Codex sessions get no keep-alive: their cache cannot be kept warm this way.',
  off: 'The cache keep-alive is off.',
  no_wake_children: 'These sub-threads finish without waking this thread, so nothing would read a warm cache.',
  children_awaiting_user: 'The sub-threads are waiting on you. Pings start again if your answer comes while the cache is still warm.',
  uncacheable_provider: "This model's provider has no prompt cache that a ping keeps warm.",
  restarted: 'Lucidos restarted since the last request, so a ping would not match the cache.',
  nothing_pending:
    'This thread waits only for your next message. Lucidos keeps a cache warm while a thread waits on a sub-thread, an event or a card.',
};

/** The one fact line of a run, on the card, in the panel and in the explainer.
 *  A run that is not open drops the budget. `null` for a status that names no
 *  run. */
export function keepAliveFacts(status: CacheKeepAliveStatus, open = true): string | null {
  switch (status.state) {
    case 'warming':
      return open
        ? `${status.beats} of ${pings(status.budget_beats)} · ${usd(status.cost_usd)} · until ${clock(status.give_up_at)}`
        : `${pings(status.beats)} · ${usd(status.cost_usd)}`;
    case 'stopped':
      return `${pings(status.beats)} · ${usd(status.cost_usd)} · stopped ${clock(status.at)}, past break-even`;
    case 'ended': {
      const saved = status.saved_usd === undefined ? '' : ` · saved about ${usd(status.saved_usd)}`;
      return `${pings(status.beats)} · ${usd(status.cost_usd)} · ${clock(status.since)}–${clock(status.at)}${saved}`;
    }
    case 'turned_off':
      return `${pings(status.beats)} · ${usd(status.cost_usd)} · turned off ${clock(status.at)}`;
    case 'not_kept':
      return null;
  }
}

/** Whether `status` is a run the thread still keeps warm, so its card may carry
 *  the meter and the switch. A card of a closed run, or one read while the
 *  thread runs, is not open. A parked thread is not running: its beats run
 *  beside the turn. `since` names the wait a run keeps warm, and `at` the
 *  moment the user turned it off. */
function isOpenRun(status: CacheKeepAliveStatus, thread: ThreadState | undefined): boolean {
  const live = thread?.meta.cacheKeepAliveStatus;
  if (!thread || !live || effectiveThreadStatus(thread) === 'running') return false;
  if (status.state === 'warming') return live.state === 'warming' && live.since === status.since;
  if (status.state === 'turned_off') return live.state === 'turned_off' && live.at === status.at;
  return false;
}

/** Turn the keep-alive off for one thread, or back on. The status follows on
 *  the event, so only a refusal needs saying. */
function switchKeepAlive(threadId: string, state: 'off' | 'on'): void {
  switchCacheKeepAlive(threadId, state).catch((e: unknown) => {
    const verb = state === 'off' ? 'turn the cache keep-alive off' : 'turn the cache keep-alive back on';
    showToast(`Could not ${verb} for thread ${threadId}: ${e instanceof Error ? e.message : String(e)}`, 'error');
  });
}

/** The per-thread switch (ADR 0554), one button for the card, the panel and the
 *  explainer: off while pings run, back on once the user turned it off. Nothing
 *  for the other states. */
function KeepAliveSwitch({ threadId, status, onSwitch }: { threadId: string; status: CacheKeepAliveStatus; onSwitch?: () => void }) {
  const state = status.state === 'warming' ? 'off' : status.state === 'turned_off' ? 'on' : null;
  if (state === null) return null;
  return (
    <div class="keep-alive-switch">
      <button
        class="action-btn action-btn-secondary"
        data-role="cache-keep-alive-switch"
        onClick={() => {
          onSwitch?.();
          switchKeepAlive(threadId, state);
        }}
      >
        {state === 'off' ? 'Turn off for this thread' : 'Turn back on'}
      </button>
    </div>
  );
}

/** The thin meter of pings spent against the break-even budget. */
function KeepAliveMeter({ status }: { status: Status<'warming'> }) {
  return (
    <div class="keep-alive-meter" aria-hidden="true">
      <span style={{ width: `${Math.min(100, (status.beats / Math.max(1, status.budget_beats)) * 100)}%` }} />
    </div>
  );
}

/** The panel section's two lines for one status: a headline, and a detail
 *  where the facts do not say it all. */
function sectionText(status: CacheKeepAliveStatus): { headline: string; detail: string | null } {
  const facts = keepAliveFacts(status);
  switch (status.state) {
    case 'warming':
    case 'ended':
      return { headline: `Cache kept warm · ${facts}`, detail: null };
    case 'stopped':
      return {
        headline: `Cache warm · stopped ${clock(status.at)}`,
        detail: `${pings(status.beats)}, ${usd(status.cost_usd)}. More pings would cost more than rereading ${rereadWhat(status.context_tokens)} once.`,
      };
    case 'turned_off':
      return { headline: `Cache warm · turned off ${clock(status.at)}`, detail: 'It turns back on when the thread runs again.' };
    case 'not_kept':
      return { headline: 'Cache warm · not kept', detail: NOT_KEPT_REASON[status.reason] };
  }
}

/** The waiting panel's "Prompt cache" section. It is always labelled, since it
 *  says something different in kind from the lists above it. */
export function KeepAliveSection({ threadId, status }: { threadId: string; status: CacheKeepAliveStatus }) {
  const { headline, detail } = sectionText(status);
  return (
    <section class="waiting-panel-section" data-role="waiting-prompt-cache" data-state={status.state}>
      <span class="waiting-panel-section-label">
        Prompt cache
        <KeepAliveExplainerButton threadId={threadId} status={status} />
      </span>
      <div class="waiting-panel-cache-line">{headline}</div>
      {detail ? <div class="waiting-panel-cache-detail">{detail}</div> : null}
      {status.state === 'warming' ? <KeepAliveMeter status={status} /> : null}
      <KeepAliveSwitch threadId={threadId} status={status} />
    </section>
  );
}

/** The transcript card of one *keep-alive run*: the pings the engine sent to
 *  keep a waiting thread's cache warm. The card stays after the run ends. It
 *  reads the thread itself, for its switch and meter belong to an open run. */
export function KeepAliveRow({ threadId, status }: { threadId: string; status: CacheKeepAliveStatus }) {
  if (status.state === 'not_kept') return null;
  const open = isOpenRun(status, threadMap.value.get(threadId));
  const closedWarming = status.state === 'warming' && !open;
  const stateLabel = closedWarming ? 'Ended' : { warming: 'Warming', stopped: 'Stopped', ended: 'Ended', turned_off: 'Turned off' }[status.state];
  return eventRowBody({
    kind: 'cache',
    state: status.state,
    role: 'cache-keep-alive',
    subject: (
      <>
        {eventNameChip({ kind: 'chip', name: 'CacheKeepAliveStatusChanged', label: 'cache kept warm', sentenceStart: true })}
        <KeepAliveExplainerButton threadId={threadId} status={status} />
      </>
    ),
    stateLabel,
    tone: open && status.state === 'warming' ? 'live' : status.state === 'stopped' ? 'halted' : 'none',
    detail: (
      <>
        {keepAliveFacts(status, open)}
        {open && status.state === 'warming' ? <KeepAliveMeter status={status} /> : null}
      </>
    ),
    actions: open ? <KeepAliveSwitch threadId={threadId} status={status} /> : undefined,
  });
}

/** How often the pill re-renders. Under a minute the text counts seconds. */
const COUNTDOWN_TICK_MS = 1000;

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

/** Whether the cache-warm line may show: the thread is idle or parked on a
 *  question with no answer or follow-up in flight, and no keep-alive run is
 *  waiting beside it, which says more. */
export function cacheWarmLineEligible(thread: ThreadState): boolean {
  return isRenderedThreadIdle(thread) && shownKeepAliveStatus(thread) === null;
}

/** One read of the cache-warm line: when the cache goes cold, what the
 *  keep-alive does meanwhile and what one ping costs, and what a reply after
 *  it goes cold pays. */
interface WarmRead {
  coldAt: number;
  keepAlive: CacheKeepAliveStatus;
  rereadUsd: number | null;
  pingUsd: number | null;
}

/** The quiet line that closes the feed of a thread waiting on the user while
 *  its prompt cache is warm. That is an idle thread, or one parked on a
 *  question. The engine says how long is left and what the keep-alive does. A
 *  read that says cold draws nothing. A read that fails draws nothing either,
 *  and the next status or event change reads again. A beat is a
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
        return {
          coldAt: Date.now() + warmth.remaining_secs * 1000,
          keepAlive: warmth.keep_alive,
          rereadUsd: warmth.reread_usd,
          pingUsd: warmth.ping_usd,
        };
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
  return <WarmLinePill threadId={threadId} read={loadable.data} />;
}

/** The pill's words after its title. A run that warms names when it stops, and
 *  every other state names itself and when the cache goes cold. */
function pillTail(status: CacheKeepAliveStatus, coldAt: number): string {
  if (status.state === 'warming') return `until ${clock(status.give_up_at)}`;
  return `${STATE_WORD[status.state]} · cold at ${formatShortTime(new Date(coldAt))}`;
}

function WarmLinePill({ threadId, read }: { threadId: string; read: WarmRead }) {
  const { coldAt, keepAlive } = read;
  const warmingUntil = keepAlive.state === 'warming' ? Date.parse(keepAlive.give_up_at) : 0;
  const now = useNow(Math.max(coldAt, warmingUntil));
  // A warming run ends at its give-up time, when the engine's next status
  // reads again. Any other state ends when the cache goes cold.
  if (now >= (keepAlive.state === 'warming' ? warmingUntil : coldAt)) return null;
  const title = keepAlive.state === 'warming' ? 'Cache kept warm' : 'Cache warm';
  return (
    <div class="cache-keep-alive-divider" data-role="cache-warm" data-state={keepAlive.state}>
      <span>
        <strong>{title}</strong>
        {` · ${pillTail(keepAlive, coldAt)}`}
        <KeepAliveExplainerButton threadId={threadId} status={keepAlive} rereadUsd={read.rereadUsd} pingUsd={read.pingUsd} hideLineLink />
      </span>
    </div>
  );
}

/** The reread a reply after the cache goes cold pays, with its price when the
 *  engine knows one: `118k tokens once, about $0.57`. */
function rereadOnce(contextTokens: number, rereadUsd: number | null): string {
  return rereadUsd === null ? `${rereadWhat(contextTokens)} once` : `${rereadWhat(contextTokens)} once, about ${usd(rereadUsd)}`;
}

/** The card's, the pill's and the panel's one ⓘ. It opens the shared
 *  explainer, which says what the cache is, the run's facts, the switch and the
 *  setting behind it. */
function KeepAliveExplainerButton({
  threadId,
  status,
  rereadUsd = null,
  pingUsd = null,
  hideLineLink = false,
}: {
  threadId: string;
  status: CacheKeepAliveStatus;
  rereadUsd?: number | null;
  pingUsd?: number | null;
  hideLineLink?: boolean;
}) {
  return (
    <Explainer title="Cache kept warm">
      {(close) => (
        <>
          <p>
            {`The model holds this thread's context in its prompt cache for a few minutes. Lucidos pings it just before the cache expires, so the next turn reads the context cheaply instead of paying to reread it${rereadSuffix(status, rereadUsd)}.`}
          </p>
          <KeepAliveRunFacts status={status} pingUsd={pingUsd} />
          <KeepAliveSwitch threadId={threadId} status={status} onSwitch={close} />
          <KeepAliveSettingLinks status={status} close={close} hideLineLink={hideLineLink} />
        </>
      )}
    </Explainer>
  );
}

/** What the reread costs, where the status knows the context: ` (118k tokens,
 *  about $0.57)`. Empty when it does not. */
function rereadSuffix(status: CacheKeepAliveStatus, rereadUsd: number | null): string {
  const tokens = 'context_tokens' in status ? status.context_tokens : 0;
  return tokens > 0 ? ` (${rereadOnce(tokens, rereadUsd)})` : '';
}

/** A price too small for cents still reads as a price. */
function aboutUsd(cost: number): string {
  return cost < 0.005 ? 'under $0.01' : `about ${usd(cost)}`;
}

/** What the run's pings cost, where the engine priced one:
 *  `Each ping costs about $0.03, so all 12 cost about $0.36.` */
function pingPrice(status: Status<'warming'>, pingUsd: number): string {
  return `Each ping costs ${aboutUsd(pingUsd)}, so all ${status.budget_beats} cost ${aboutUsd(pingUsd * status.budget_beats)}.`;
}

/** The run's facts and the meter, or the reason nothing keeps the cache warm. */
function KeepAliveRunFacts({ status, pingUsd }: { status: CacheKeepAliveStatus; pingUsd: number | null }) {
  if (status.state === 'not_kept') return <p>{`Nothing keeps it warm while you are away. ${NOT_KEPT_REASON[status.reason]}`}</p>;
  const { detail } = sectionText(status);
  return (
    <>
      <p>{`${keepAliveFacts(status)}.`}</p>
      {status.state === 'warming' && pingUsd !== null ? <p>{pingPrice(status, pingUsd)}</p> : null}
      {detail && status.state !== 'stopped' ? <p>{detail}</p> : null}
      {status.state === 'warming' ? <KeepAliveMeter status={status} /> : null}
    </>
  );
}

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

const KEEP_ALIVE_SETTING = 'Settings → Models → Cache keep-alive';

/** Reasons no keep-alive setting can change: the thread is never beaten. */
const SETTING_CANNOT_APPLY: ReadonlySet<CacheKeepAliveNotKeptReason> = new Set(['codex', 'uncacheable_provider']);

/** What the keep-alive setting changes, as the lead-in to its link. */
function settingLead(status: CacheKeepAliveStatus): string {
  switch (status.state) {
    case 'warming':
    case 'turned_off':
    case 'ended':
      return 'Change how long it keeps warm in';
    case 'stopped':
      return 'Change when it gives up in';
    case 'not_kept':
      return status.reason === 'off' ? 'Turn it on in' : 'Set up the keep-alive in';
  }
}

/** The paragraphs linking the settings behind the status: the keep-alive, and
 *  for the pill the switch that hides it. Each ends on its link, with no full
 *  stop to wrap. */
function KeepAliveSettingLinks({ status, close, hideLineLink }: { status: CacheKeepAliveStatus; close: () => void; hideLineLink: boolean }) {
  const keepAliveApplies = !(status.state === 'not_kept' && SETTING_CANNOT_APPLY.has(status.reason));
  return (
    <>
      {keepAliveApplies && (
        <p>
          {settingLead(status)} <SettingLink label={KEEP_ALIVE_SETTING} close={close} open={openCacheKeepAliveSetting} />
        </p>
      )}
      {hideLineLink && (
        <p>
          Hide this line in <SettingLink label="Settings → System → Debugging" close={close} open={openCacheWarmLineSetting} />
        </p>
      )}
    </>
  );
}
