import { useEffect, useState } from 'preact/hooks';
import { fetchCacheWarmth, switchCacheKeepAlive } from '../../api/client';
import type { CacheWarmth, WarmthTtl } from '../../api/client/successor';
import type { CacheKeepAliveNotKeptReason, CacheKeepAliveStatus, CapSource, PlannedWake } from '../../generated/thread-event-wire';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { effectiveThreadStatus, isMidTurn, isRenderedThreadIdle, showToast, threadMap } from '../../store/store';
import type { ThreadState } from '../../store/thread-events';
import { formatShortTime } from '../../utils/formatTime';
import { formatTokens } from '../../utils/formatTokens';
import { openCacheKeepAliveSetting, openCacheCardsSetting } from '../../store/actions/menu';
import { Disclosure } from '../shared/Disclosure';
import { Explainer } from '../shared/Explainer';
import { ChevronRightIcon } from '../shared/icons';
import { eventNameChip, eventRowBody } from './EventRow';

type Status<S extends CacheKeepAliveStatus['state']> = Extract<CacheKeepAliveStatus, { state: S }>;

export function shownKeepAliveStatus(thread: ThreadState | undefined): CacheKeepAliveStatus | null {
  const status = thread?.meta.cacheKeepAliveStatus;
  if (!status || status.state === 'ended' || isMidTurn(effectiveThreadStatus(thread))) return null;
  return status;
}

function usd(cost: number): string {
  return `$${cost.toFixed(2)}`;
}

function signedUsd(cost: number): string {
  const cents = Math.round(cost * 100) / 100;
  return cents < 0 ? `\u2212${usd(-cents)}` : usd(cents);
}

function pings(count: number): string {
  return `${count} ping${count === 1 ? '' : 's'}`;
}

function clock(iso: string): string {
  return formatShortTime(new Date(iso));
}

function rereadWhat(contextTokens: number): string {
  return contextTokens > 0 ? `${formatTokens(contextTokens)} tokens` : 'the whole conversation';
}

const NOT_KEPT_REASON: Record<CacheKeepAliveNotKeptReason, string> = {
  codex: 'Codex sessions get no keep-alive: their cache cannot be kept warm this way.',
  off: 'The cache keep-alive is off.',
  no_wake_children: 'These sub-threads finish without waking this thread, so nothing would read a warm cache.',
  children_awaiting_user: 'The sub-threads are waiting on you. Pings start again if your answer comes while the cache is still warm.',
  uncacheable_provider: 'Lucidos cannot safely prefill this prompt cache.',
  provider_paused: 'Pings on this provider are paused after a failed ping.',
  prefill_paused: 'Prefill pings are temporarily paused.',
  pings_paused: "Pings to this model's provider kept missing the cache, so they pause for a while.",
  cache_expired: 'The cache went cold before a ping reached it, so a ping would pay to rewrite it.',
  restarted: 'Lucidos restarted since the last request, so a ping would not match the cache.',
  past_give_up_time: 'The wait passed its give-up time before the first ping was due, so no ping went out.',
  wake_past_reread_cap: 'The wake is expected too late for pings to cost clearly less than rereading the cache, so none go out.',
  nothing_pending:
    'This thread waits only for your next message. Lucidos keeps a cache warm while a thread waits on a sub-thread, an event or a card.',
  unattended_trigger: 'Nobody has written in this trigger thread, so nothing would read a warm cache.',
  unknown_ttl: 'Lucidos does not know how long this cache lives, so it cannot time a ping.',
  model_changed: 'The model or reasoning effort changed, so this cache cannot be kept warm.',
  beat_missed: 'A ping missed the cache, so this wait is not kept warm.',
};

function notKeptReason(status: Status<'not_kept'>): string {
  if (status.reason === 'pings_paused' && status.until !== undefined) {
    return `Pings to this model's provider kept missing the cache, so they pause until ${clock(status.until)}.`;
  }
  return `${NOT_KEPT_REASON[status.reason]}${planSentence(status.cap_source)}`;
}

function capSourcePhrase(source: CapSource): string {
  switch (source.kind) {
    case 'half_reread':
      return 'pings worth half of one reread';
    case 'reread_cap':
      return 'pings worth one reread';
    case 'timeout':
      return "the wait's timeout";
    case 'estimate':
      return source.by === 'agent' ? "the agent's expected wake" : "Lucidos's wake estimate";
    case 'learned': {
      const from = `${waitsLikeThis(source.samples)} asked ${hourWindow(source)}`;
      return source.pings === 0 ? `learned: no pings at this hour, from ${from}` : `${pings(source.pings)}, learned from ${from}`;
    }
  }
}

function hourWindow(source: Extract<CapSource, { kind: 'learned' }>): string {
  const today = new Date();
  const at = (hour: number) =>
    formatShortTime(
      new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate(), hour) - source.utc_offset_mins * 60_000),
    );
  return `${at(source.from_hour)} to ${at(source.to_hour)}`;
}

function waitsLikeThis(n: number): string {
  return n === 1 ? '1 wait like this' : `${n.toLocaleString('en-US')} waits like this`;
}

function planSentence(source: CapSource | undefined): string {
  return source ? ` Its plan: ${capSourcePhrase(source)}.` : '';
}

function capEnd(lead: string, iso: string, source: CapSource | undefined): string {
  return source ? `${lead} ${clock(iso)}, the end of its plan: ${capSourcePhrase(source)}.` : `${lead} ${clock(iso)}.`;
}

function expectedWakeSentence(wake: PlannedWake | undefined): string {
  if (!wake) return '';
  const who = wake.source === 'agent' ? 'The agent expects' : 'Lucidos estimates';
  return ` ${who} a wake about ${clock(wake.at)}.`;
}

export function keepAliveSentence(status: CacheKeepAliveStatus, open = true): string | null {
  switch (status.state) {
    case 'warming': {
      if (!open) return `Spent ${usd(status.cost_usd)} on ${pings(status.beats)}.`;
      const math = keepAliveMath(status);
      const expected = math ? ` ${expectedSaving(math)}` : '';
      return `Spent ${usd(status.cost_usd)} so far.${expected} ${capEnd('Warm until', status.give_up_at, status.cap_source)}${expectedWakeSentence(status.expected_wake)}`;
    }
    case 'stopped': {
      const end = status.cap_source ? capEnd('Stopped at', status.at, status.cap_source) : `Stopped at ${clock(status.at)}, the end of its plan.`;
      return `Spent ${usd(status.cost_usd)} on ${pings(status.beats)}. ${end}`;
    }
    case 'ended': {
      const saved = status.saved_usd === undefined ? '' : ` Saved about ${usd(status.saved_usd)}.`;
      const early = status.beats === 0 ? ' Ended before the first ping.' : '';
      return `Spent ${usd(status.cost_usd)} on ${pings(status.beats)}, ${clock(status.since)} to ${clock(status.at)}.${saved}${early}${planSentence(status.cap_source)}`;
    }
    case 'turned_off': {
      const early = status.beats === 0 ? ', before the first ping' : '';
      return `Spent ${usd(status.cost_usd)} on ${pings(status.beats)}. Turned off at ${clock(status.at)}${early}.${planSentence(status.cap_source)}`;
    }
    case 'not_kept':
      return null;
  }
}

function explainerSentence(status: RunStatus): string {
  if (status.state !== 'warming') return keepAliveSentence(status)!;
  return `${status.beats} of ${pings(status.budget_beats)} sent, ${usd(status.cost_usd)} so far. ${capEnd('Warm until', status.give_up_at, status.cap_source)}${expectedWakeSentence(status.expected_wake)}`;
}

interface MathRow {
  when: string;
  chance: number;
  keepWarm: number;
  noKeepAlive: number;
}

type RunStatus = Status<'warming' | 'stopped' | 'ended' | 'turned_off'>;

interface KeepAliveScore {
  keepWarm: number;
  noKeepAlive: number;
}

export interface KeepAliveMath {
  rows: MathRow[];
  keepWarm: number;
  noKeepAlive: number;
  samples: number;
  contextTokens: number;
  pingUsd: number;
  rereadUsd: number;
  score: KeepAliveScore | null;
  landed: number | null;
  hours: string | null;
}

function minutes(secs: number): string {
  return String(Number((secs / 60).toFixed(1)));
}

function readWarmCache(status: RunStatus): status is Status<'ended'> {
  return status.state === 'ended' && (status.saved_usd !== undefined || status.beats === 0);
}

function keepAliveScore(status: RunStatus, rereadUsd: number): KeepAliveScore | null {
  if (status.state === 'warming') return null;
  if (readWarmCache(status)) return { keepWarm: status.cost_usd, noKeepAlive: status.saved_usd ?? 0 };
  return { keepWarm: status.cost_usd + rereadUsd, noKeepAlive: rereadUsd };
}

function landedRow(status: RunStatus, score: KeepAliveScore | null, cap: number): number | null {
  if (!score) return null;
  if (status.state === 'stopped') return cap + 1;
  return readWarmCache(status) && status.beats <= cap ? status.beats : null;
}

export function keepAliveMath(status: CacheKeepAliveStatus): KeepAliveMath | null {
  if (status.state === 'not_kept') return null;
  const { odds, ping_usd: pingUsd, reread_usd: rereadUsd, beat_every_secs: interval } = status;
  const contextTokens = status.context_tokens ?? 0;
  if (!odds || odds.samples === 0 || interval === undefined || pingUsd === undefined || rereadUsd === undefined || contextTokens <= 0) return null;
  const at = (k: number) => minutes(k * interval);
  const cap = odds.slots.length - 1;
  const rows: MathRow[] = [
    ...odds.slots.map((waits, k) => ({
      when: `${at(k)}–${at(k + 1)} min`,
      chance: waits / odds.samples,
      keepWarm: k * pingUsd,
      noKeepAlive: k === 0 ? 0 : rereadUsd,
    })),
    {
      when: `after ${at(cap + 1)} min`,
      chance: odds.after_cap / odds.samples,
      keepWarm: cap * pingUsd + rereadUsd,
      noKeepAlive: rereadUsd,
    },
  ];
  const expected = (cost: (row: MathRow) => number) => rows.reduce((sum, row) => sum + row.chance * cost(row), 0);
  const score = keepAliveScore(status, rereadUsd);
  return {
    rows,
    keepWarm: expected((row) => row.keepWarm),
    noKeepAlive: expected((row) => row.noKeepAlive),
    samples: odds.samples,
    contextTokens,
    pingUsd,
    rereadUsd,
    score,
    landed: landedRow(status, score, cap),
    hours: status.cap_source?.kind === 'learned' ? hourWindow(status.cap_source) : null,
  };
}

function expectedSaving(math: KeepAliveMath): string {
  const saving = math.noKeepAlive - math.keepWarm;
  const verdict = saving >= 0 ? `Expected to save ${usd(saving)}` : `Expected to cost ${usd(-saving)} more`;
  return `${verdict}, from ${oddsSource(math)}.`;
}

function oddsSource(math: KeepAliveMath): string {
  const last = `your last ${math.samples === 1 ? 'wait like this' : waitsLikeThis(math.samples)}`;
  return math.hours ? `${last} asked ${math.hours}` : last;
}

function expectedSummary(math: KeepAliveMath): string {
  const saving = math.noKeepAlive - math.keepWarm;
  const verdict = saving >= 0 ? `saves about ${usd(saving)}` : `costs about ${usd(-saving)} more`;
  return `Expected cost: keep warm ${usd(math.keepWarm)}, no keep-alive ${usd(math.noKeepAlive)}, so keeping it warm ${verdict}.`;
}

function scoreSummary({ keepWarm, noKeepAlive }: KeepAliveScore): string {
  const net = Math.round((noKeepAlive - keepWarm) * 100) / 100;
  const verdict = net > 0 ? `saved ${usd(net)}` : net < 0 ? `cost ${usd(-net)} more` : 'made no difference';
  return `Final cost: keep warm ${usd(keepWarm)}, no keep-alive ${usd(noKeepAlive)}, so keeping it warm ${verdict}.`;
}

function mathFootnote(math: KeepAliveMath): string {
  const tokens = formatTokens(math.contextTokens);
  const perMillion = (price: number) => `$${((price / math.contextTokens) * 1e6).toFixed(2)}/M`;
  return (
    `A ping reads ${tokens} tokens: ${tokens} × ${perMillion(math.pingUsd)} = ${usd(math.pingUsd)}. ` +
    `The reread writes them again: ${tokens} × ${perMillion(math.rereadUsd)} = ${usd(math.rereadUsd)}. ` +
    `Chances come from ${oddsSource(math)}.`
  );
}

function percent(share: number): string {
  const whole = Math.round(share * 100);
  return whole === 0 && share > 0 ? '<1%' : `${whole}%`;
}

function KeepAliveMathFold({ math }: { math: KeepAliveMath }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <p>{math.score ? scoreSummary(math.score) : expectedSummary(math)}</p>
      <div class="keep-alive-math-fold">
        <button type="button" class="keep-alive-math-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
          <ChevronRightIcon size="1rem" />
          {open ? 'Hide the math' : 'Show the math'}
        </button>
        <Disclosure open={open}>
          <table class="keep-alive-math" data-role="keep-alive-math">
            <thead>
              <tr>
                <th>Reply comes</th>
                <th>Chance</th>
                <th>Keep warm</th>
                <th>No keep-alive</th>
                <th>Saved</th>
              </tr>
            </thead>
            <tbody>
              {math.rows.map((row, k) => (
                <tr key={row.when} data-landed={k === math.landed ? 'true' : undefined}>
                  <td>{row.when}</td>
                  <td>{percent(row.chance)}</td>
                  <td>{usd(row.keepWarm)}</td>
                  <td>{usd(row.noKeepAlive)}</td>
                  <td>{signedUsd(row.noKeepAlive - row.keepWarm)}</td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr>
                <td>Expected</td>
                <td>100%</td>
                <td>{usd(math.keepWarm)}</td>
                <td>{usd(math.noKeepAlive)}</td>
                <td>{signedUsd(math.noKeepAlive - math.keepWarm)}</td>
              </tr>
            </tfoot>
          </table>
          <p class="keep-alive-math-note">{mathFootnote(math)}</p>
        </Disclosure>
      </div>
    </>
  );
}

function rereadSentence(status: RunStatus): string | null {
  if (status.state === 'ended' && status.saved_usd !== undefined) return null;
  const tokens = status.context_tokens ?? 0;
  if (status.reread_usd === undefined) return tokens > 0 ? `A reply after the cache goes cold rereads ${rereadWhat(tokens)}.` : null;
  return `Rereading ${rereadWhat(tokens)} costs ${usd(status.reread_usd)}.`;
}

function pingsBeforeWake(status: Status<'warming'>): number | null {
  if (!status.expected_wake || status.beat_every_secs === undefined) return null;
  const secs = (Date.parse(status.expected_wake.at) - Date.parse(status.since)) / 1000;
  return Math.max(0, Math.floor(secs / status.beat_every_secs));
}

function warmingComparison(status: Status<'warming'>, pingUsd: number, rereadUsd: number): string {
  const wakePings = pingsBeforeWake(status);
  const budget = status.budget_beats;
  if (wakePings === null) {
    return `The whole plan of ${pings(budget)} costs at most ${usd(budget * pingUsd)}, against ${usd(rereadUsd)} to reread.`;
  }
  if (wakePings === 0) return 'The expected wake comes before the first ping, so the cache is still warm without one.';
  if (wakePings > budget) {
    return `The plan stops after ${pings(budget)}, before the expected wake, so the reply rereads anyway: keeping it warm costs about ${usd(budget * pingUsd)} more.`;
  }
  const keepWarm = wakePings * pingUsd;
  const saving = Math.round((rereadUsd - keepWarm) * 100) / 100;
  const verdict = saving >= 0 ? `saves about ${usd(saving)}` : `costs about ${usd(-saving)} more`;
  return `Keeping it warm to the expected wake takes ${pings(wakePings)}, about ${usd(keepWarm)}, against ${usd(rereadUsd)} to reread. That ${verdict}.`;
}

function priceSentences(status: RunStatus): string[] {
  const reread = rereadSentence(status);
  const tokens = status.context_tokens ?? 0;
  if (status.ping_usd === undefined) return reread ? [reread] : [];
  const ping = `A ping reads ${rereadWhat(tokens)} for ${usd(status.ping_usd)}.`;
  if (status.state === 'warming' && status.reread_usd !== undefined) {
    return [ping, warmingComparison(status, status.ping_usd, status.reread_usd)];
  }
  return reread ? [ping, reread] : [ping];
}

function isOpenRun(status: CacheKeepAliveStatus, thread: ThreadState | undefined): boolean {
  const live = thread?.meta.cacheKeepAliveStatus;
  if (!thread || !live || effectiveThreadStatus(thread) === 'running') return false;
  if (status.state === 'warming') return live.state === 'warming' && live.since === status.since;
  if (status.state === 'turned_off') return live.state === 'turned_off' && live.at === status.at;
  return false;
}

function switchKeepAlive(threadId: string, state: 'off' | 'on'): void {
  switchCacheKeepAlive(threadId, state).catch((e: unknown) => {
    const verb = state === 'off' ? 'turn the cache keep-alive off' : 'turn the cache keep-alive back on';
    showToast(`Could not ${verb} for thread ${threadId}: ${e instanceof Error ? e.message : String(e)}`, 'error');
  });
}

function KeepAliveSwitch({ threadId, status }: { threadId: string; status: CacheKeepAliveStatus }) {
  const state = status.state === 'warming' ? 'off' : status.state === 'turned_off' ? 'on' : null;
  if (state === null) return null;
  return (
    <div class="keep-alive-switch">
      <button class="action-btn action-btn-secondary" data-role="cache-keep-alive-switch" onClick={() => switchKeepAlive(threadId, state)}>
        {state === 'off' ? 'Turn off for this thread' : 'Turn back on'}
      </button>
    </div>
  );
}

function KeepAliveMeter({ status }: { status: Status<'warming'> }) {
  return (
    <div class="keep-alive-meter" aria-hidden="true">
      <span style={{ width: `${Math.min(100, (status.beats / Math.max(1, status.budget_beats)) * 100)}%` }} />
    </div>
  );
}

function keptWarm(status: CacheKeepAliveStatus): boolean {
  return status.state === 'warming' || status.state === 'ended';
}

function cacheTitle(status: CacheKeepAliveStatus): string {
  return keptWarm(status) ? 'Cache kept warm' : 'Cache keep-alive';
}

function sectionHeadline(status: CacheKeepAliveStatus): string {
  return keepAliveSentence(status) ?? 'Not kept';
}

const TURNS_BACK_ON = 'It turns back on when the thread runs again.';

function sectionDetail(status: CacheKeepAliveStatus): string | null {
  switch (status.state) {
    case 'warming':
    case 'ended':
      return null;
    case 'stopped':
      return rereadSentence(status);
    case 'turned_off':
      return TURNS_BACK_ON;
    case 'not_kept':
      return notKeptReason(status);
  }
}

function useColdAt(threadId: string, status: CacheKeepAliveStatus): ColdAt | undefined {
  const { loadable } = useLoadableFetch(
    async (): Promise<ColdAt | undefined> => {
      if (status.state === 'warming') return undefined;
      try {
        return coldAtOf(await fetchCacheWarmth(threadId));
      } catch (e) {
        console.warn(`[KeepAliveSection] cache-warmth read failed for thread "${threadId}"`, e);
        throw e;
      }
    },
    [threadId, status],
  );
  return loadable.status === 'loaded' ? loadable.data : undefined;
}

type ColdAt = WarmUntil | null | 'unknown';

interface WarmUntil {
  at: number;
  ttl: WarmthTtl | null;
}

function coldAtOf(warmth: CacheWarmth): ColdAt {
  switch (warmth.warmth) {
    case 'warm':
      return { at: Date.now() + warmth.remaining_secs * 1000, ttl: warmth.ttl };
    case 'cold':
      return null;
    case 'unknown':
      return 'unknown';
  }
}

const LIFETIME_WORDS: Record<WarmthTtl['lifetime'], string> = { '5m': '5-minute', '30m': '30-minute', '1h': '1-hour', '12h': '12-hour' };

function warmUntil({ at, ttl }: WarmUntil): string {
  const time = `Warm until ${formatShortTime(new Date(at))}`;
  if (!ttl) return time;
  const says = ttl.evidence === 'reported' ? 'the provider reported' : 'the request asked for';
  return `${time}, on the ${LIFETIME_WORDS[ttl.lifetime]} lifetime ${says}`;
}

function warmthSentence(coldAt: ColdAt | undefined, now: number): string {
  if (coldAt === undefined) return '';
  if (coldAt === 'unknown') return ' Its lifetime is unknown.';
  return coldAt !== null && now < coldAt.at ? ` ${warmUntil(coldAt)}.` : ' The cache is cold.';
}

export function KeepAliveSection({ threadId, status }: { threadId: string; status: CacheKeepAliveStatus }) {
  const coldAt = useColdAt(threadId, status);
  const now = useNow(typeof coldAt === 'object' && coldAt !== null ? coldAt.at : 0);
  const detail = sectionDetail(status);
  const headline = `${sectionHeadline(status)}${warmthSentence(coldAt, now)}`;
  return (
    <section class="waiting-panel-section" data-role="waiting-prompt-cache" data-state={status.state}>
      <span class="waiting-panel-section-label">
        Prompt cache
        <KeepAliveExplainerButton status={status} title="Prompt cache" />
      </span>
      <div class="waiting-panel-cache-line">{headline}</div>
      {detail ? <div class="waiting-panel-cache-detail">{detail}</div> : null}
      {status.state === 'warming' ? <KeepAliveMeter status={status} /> : null}
      <KeepAliveSwitch threadId={threadId} status={status} />
    </section>
  );
}

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
        {eventNameChip({ kind: 'chip', name: 'CacheKeepAliveStatusChanged', label: cacheTitle(status).toLowerCase(), sentenceStart: true })}
        <KeepAliveExplainerButton status={status} title={cacheTitle(status)} />
      </>
    ),
    stateLabel,
    tone: open && status.state === 'warming' ? 'live' : status.state === 'stopped' ? 'halted' : 'none',
    detail: (
      <>
        {keepAliveSentence(status, open)}
        {open && status.state === 'warming' ? <KeepAliveMeter status={status} /> : null}
      </>
    ),
    actions: open ? <KeepAliveSwitch threadId={threadId} status={status} /> : undefined,
  });
}

const COUNTDOWN_TICK_MS = 1000;

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

export function cacheWarmLineEligible(thread: ThreadState): boolean {
  return isRenderedThreadIdle(thread);
}

interface WarmRead {
  coldAt: WarmUntil | 'unknown';
  keepAlive: CacheKeepAliveStatus;
}

export function CacheWarmLine({ threadId, eligible, eventCount }: { threadId: string; eligible: boolean; eventCount: number }) {
  const { loadable } = useLoadableFetch(
    async (): Promise<WarmRead | null> => {
      if (!eligible) return null;
      try {
        const warmth = await fetchCacheWarmth(threadId);
        const coldAt = coldAtOf(warmth);
        if (warmth.keep_alive === null || coldAt === null) return null;
        return { coldAt, keepAlive: warmth.keep_alive };
      } catch (e) {
        console.warn(`[CacheWarmLine] cache-warmth read failed for thread "${threadId}"`, e);
        return null;
      }
    },
    [threadId, eligible, eventCount],
    { keepLoadedWhileRefetching: true },
  );
  if (!eligible || loadable.status !== 'loaded' || loadable.data === null || loadable.data.keepAlive.state !== 'not_kept') return null;
  return <WarmLineCard read={loadable.data} />;
}

function WarmLineCard({ read }: { read: WarmRead }) {
  const { coldAt, keepAlive } = read;
  const unknown = coldAt === 'unknown';
  const now = useNow(unknown ? 0 : coldAt.at);
  if (!unknown && now >= coldAt.at) return null;
  return (
    <div class="cache-warm-card">
      {eventRowBody({
        kind: 'cache',
        state: keepAlive.state,
        role: 'cache-warm',
        subject: (
          <>
            {eventNameChip({ kind: 'chip', name: 'CacheKeepAliveStatusChanged', label: 'prompt cache', sentenceStart: true })}
            <KeepAliveExplainerButton status={keepAlive} title="Prompt cache" />
          </>
        ),
        stateLabel: 'Not kept',
        detail: unknown ? 'Lifetime unknown' : warmUntil(coldAt),
      })}
    </div>
  );
}

function KeepAliveExplainerButton({ status, title }: { status: CacheKeepAliveStatus; title: string }) {
  return (
    <Explainer title={title} wide={keepAliveMath(status) !== null}>
      {(close) => (
        <>
          <p>{cacheIntro(status)}</p>
          <KeepAliveRunFacts status={status} />
          <KeepAliveSettingLinks status={status} close={close} />
        </>
      )}
    </Explainer>
  );
}

function cacheIntro(status: CacheKeepAliveStatus): string {
  const keeps =
    status.state === 'not_kept'
      ? 'A turn while it is warm reads the context cheaply'
      : 'Lucidos pings it just before the cache expires, so the next turn reads the context cheaply';
  return `The model holds this thread's context in its prompt cache for a while. ${keeps} instead of paying to reread it.`;
}

function KeepAliveRunFacts({ status }: { status: CacheKeepAliveStatus }) {
  if (status.state === 'not_kept') return <p>{`Nothing keeps it warm while you are away. ${notKeptReason(status)}`}</p>;
  const math = keepAliveMath(status);
  const prices = math ? [] : priceSentences(status);
  return (
    <>
      <p>{explainerSentence(status)}</p>
      {status.state === 'warming' ? <KeepAliveMeter status={status} /> : null}
      {math ? <KeepAliveMathFold math={math} /> : null}
      {prices.length > 0 ? <p>{prices.join(' ')}</p> : null}
      {status.state === 'turned_off' ? <p>{TURNS_BACK_ON}</p> : null}
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

function KeepAliveSettingLinks({ status, close }: { status: CacheKeepAliveStatus; close: () => void }) {
  if (status.state === 'not_kept' && status.reason === 'off') {
    return (
      <p>
        Turn it on in <SettingLink label={KEEP_ALIVE_SETTING} close={close} open={openCacheKeepAliveSetting} />
      </p>
    );
  }
  return (
    <p>
      Hide cache cards in <SettingLink label={KEEP_ALIVE_SETTING} close={close} open={openCacheCardsSetting} />
    </p>
  );
}
