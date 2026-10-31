import { useEffect, useState } from 'preact/hooks';
import { fetchCacheWarmth, switchCacheKeepAlive } from '../../api/client';
import type { CacheWarmth } from '../../api/client/successor';
import type { CacheKeepAliveNotKeptReason, CacheKeepAliveStatus, PlannedWake } from '../../generated/thread-event-wire';
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

/** An amount that may be negative, with a true minus sign: `−$0.72`. */
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

/** The context a wake-up would reread, as a phrase after "reread". */
function rereadWhat(contextTokens: number): string {
  return contextTokens > 0 ? `${formatTokens(contextTokens)} tokens` : 'the whole conversation';
}

/** Each reason ends on a full stop. `pings_paused` names its end in
 *  `notKeptReason`. */
const NOT_KEPT_REASON: Record<CacheKeepAliveNotKeptReason, string> = {
  codex: 'Codex sessions get no keep-alive: their cache cannot be kept warm this way.',
  off: 'The cache keep-alive is off.',
  no_wake_children: 'These sub-threads finish without waking this thread, so nothing would read a warm cache.',
  children_awaiting_user: 'The sub-threads are waiting on you. Pings start again if your answer comes while the cache is still warm.',
  uncacheable_provider: 'Lucidos cannot safely prefill this prompt cache.',
  provider_paused: 'Pings on this provider are paused after a failed ping.',
  // Historical: the engine no longer emits it, but stored events still carry it.
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

/** Why nothing keeps the cache warm, with the time paused pings resume. */
function notKeptReason(status: Status<'not_kept'>): string {
  if (status.reason === 'pings_paused' && status.until !== undefined) {
    return `Pings to this model's provider kept missing the cache, so they pause until ${clock(status.until)}.`;
  }
  return NOT_KEPT_REASON[status.reason];
}

/** Who expects the wake a run plans from, and when. Empty with none: the
 *  run then pings until the reread cap. */
function expectedWakeSentence(wake: PlannedWake | undefined): string {
  if (!wake) return '';
  const who = wake.source === 'agent' ? 'The agent expects' : 'Lucidos estimates';
  return ` ${who} a wake about ${clock(wake.at)}.`;
}

/** The sentences of a run, on the card and in the panel. A run that is not
 *  open drops the budget. `null` for a status that names no run. */
export function keepAliveSentence(status: CacheKeepAliveStatus, open = true): string | null {
  switch (status.state) {
    case 'warming': {
      if (!open) return `Spent ${usd(status.cost_usd)} on ${pings(status.beats)}.`;
      const math = keepAliveMath(status);
      const expected = math ? ` ${expectedSaving(math)}` : '';
      return `Spent ${usd(status.cost_usd)} so far.${expected} Warm until ${clock(status.give_up_at)}.${expectedWakeSentence(status.expected_wake)}`;
    }
    case 'stopped':
      return `Spent ${usd(status.cost_usd)} on ${pings(status.beats)}. Stopped at ${clock(status.at)}, the end of its plan.`;
    case 'ended': {
      const saved = status.saved_usd === undefined ? '' : ` Saved about ${usd(status.saved_usd)}.`;
      const early = status.beats === 0 ? ' Ended before the first ping.' : '';
      return `Spent ${usd(status.cost_usd)} on ${pings(status.beats)}, ${clock(status.since)} to ${clock(status.at)}.${saved}${early}`;
    }
    case 'turned_off': {
      const early = status.beats === 0 ? ', before the first ping' : '';
      return `Spent ${usd(status.cost_usd)} on ${pings(status.beats)}. Turned off at ${clock(status.at)}${early}.`;
    }
    case 'not_kept':
      return null;
  }
}

/** The explainer's sentence of a run: a warming one counts against its budget. */
function explainerSentence(status: RunStatus): string {
  if (status.state !== 'warming') return keepAliveSentence(status)!;
  return `${status.beats} of ${pings(status.budget_beats)} sent, ${usd(status.cost_usd)} so far. Warm until ${clock(status.give_up_at)}.${expectedWakeSentence(status.expected_wake)}`;
}

/** One row of the keep-alive math: when the reply comes, how likely that is,
 *  and what each choice costs then. */
interface MathRow {
  when: string;
  chance: number;
  keepWarm: number;
  noKeepAlive: number;
}

type RunStatus = Status<'warming' | 'stopped' | 'ended' | 'turned_off'>;

/** What a closed run actually cost against what no keep-alive would have cost. */
interface KeepAliveScore {
  keepWarm: number;
  noKeepAlive: number;
}

/** The expected cost of a run against no keep-alive, slot by slot. A closed
 *  run adds its final `score` and the row it `landed` in. */
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
}

/** Minutes as the math labels them: `4.5`, `9`. */
function minutes(secs: number): string {
  return String(Number((secs / 60).toFixed(1)));
}

/** Whether an ended run's reply read a warm cache: the engine claims a saving,
 *  or the run never pinged and the cache was warm with or without it. */
function readWarmCache(status: RunStatus): status is Status<'ended'> {
  return status.state === 'ended' && (status.saved_usd !== undefined || status.beats === 0);
}

/** The final score of a closed run, `null` while it is open. A reply that read
 *  a warm cache paid the pings and saved the reread. Any other reply pays the
 *  reread whatever the pings did: a stopped or turned-off run, or an ended one
 *  on a cold cache. */
function keepAliveScore(status: RunStatus, rereadUsd: number): KeepAliveScore | null {
  if (status.state === 'warming') return null;
  if (readWarmCache(status)) return { keepWarm: status.cost_usd, noKeepAlive: status.saved_usd ?? 0 };
  return { keepWarm: status.cost_usd + rereadUsd, noKeepAlive: rereadUsd };
}

/** The table row a closed run landed in: the slot of its pings when its reply
 *  read a warm cache, and the last row for a run that stopped. `null` where no
 *  row fits, such as a run the user turned off. */
function landedRow(status: RunStatus, score: KeepAliveScore | null, cap: number): number | null {
  if (!score) return null;
  if (status.state === 'stopped') return cap + 1;
  return readWarmCache(status) && status.beats <= cap ? status.beats : null;
}

/** The keep-alive math of a run, from the cap odds the engine counted when it
 *  chose the cap. Slot `k` ended after `k` pings with the cache warm. Without
 *  a keep-alive any reply after the first slot pays the reread. A reply after
 *  the cap pays every ping and the reread. `null` without odds, an interval
 *  or prices. */
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
  };
}

/** The card's word on the math: what keeping the cache warm should save. */
function expectedSaving(math: KeepAliveMath): string {
  const saving = math.noKeepAlive - math.keepWarm;
  return saving >= 0 ? `Expected to save ${usd(saving)}.` : `Expected to cost ${usd(-saving)} more.`;
}

/** The explainer's one-sentence summary of the math. */
function expectedSummary(math: KeepAliveMath): string {
  const saving = math.noKeepAlive - math.keepWarm;
  const verdict = saving >= 0 ? `saves about ${usd(saving)}` : `costs about ${usd(-saving)} more`;
  return `Expected cost: keep warm ${usd(math.keepWarm)}, no keep-alive ${usd(math.noKeepAlive)}, so keeping it warm ${verdict}.`;
}

/** The explainer's summary of a closed run: what it cost against none. */
function scoreSummary({ keepWarm, noKeepAlive }: KeepAliveScore): string {
  const net = Math.round((noKeepAlive - keepWarm) * 100) / 100;
  const verdict = net > 0 ? `saved ${usd(net)}` : net < 0 ? `cost ${usd(-net)} more` : 'made no difference';
  return `Final cost: keep warm ${usd(keepWarm)}, no keep-alive ${usd(noKeepAlive)}, so keeping it warm ${verdict}.`;
}

/** Where the chances and the prices come from. */
function mathFootnote(math: KeepAliveMath): string {
  const tokens = formatTokens(math.contextTokens);
  const perMillion = (price: number) => `$${((price / math.contextTokens) * 1e6).toFixed(2)}/M`;
  const waits = math.samples === 1 ? 'your last wait' : `your last ${math.samples.toLocaleString('en-US')} waits`;
  return (
    `A ping reads ${tokens} tokens: ${tokens} × ${perMillion(math.pingUsd)} = ${usd(math.pingUsd)}. ` +
    `The reread writes them again: ${tokens} × ${perMillion(math.rereadUsd)} = ${usd(math.rereadUsd)}. ` +
    `Chances come from ${waits} like this.`
  );
}

/** A chance as a whole percent. A share that rounds to none but is not
 *  none reads `<1%`, so a slot past waits did end in never reads empty. */
function percent(share: number): string {
  const whole = Math.round(share * 100);
  return whole === 0 && share > 0 ? '<1%' : `${whole}%`;
}

/** The math behind the expected cost, folded away until asked for. */
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

/** What a reply after the cache goes cold pays, where the status knows. An
 *  ended run that saved the reread already says what it saved. */
function rereadSentence(status: RunStatus): string | null {
  if (status.state === 'ended' && status.saved_usd !== undefined) return null;
  const tokens = status.context_tokens ?? 0;
  if (status.reread_usd === undefined) return tokens > 0 ? `A reply after the cache goes cold rereads ${rereadWhat(tokens)}.` : null;
  return `Rereading ${rereadWhat(tokens)} costs ${usd(status.reread_usd)}.`;
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

/** The per-thread switch (ADR 0554), one button for the card and the panel:
 *  off while pings run, back on once the user turned it off. Nothing for the
 *  other states. */
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

/** The thin meter of pings spent against the reread cap. */
function KeepAliveMeter({ status }: { status: Status<'warming'> }) {
  return (
    <div class="keep-alive-meter" aria-hidden="true">
      <span style={{ width: `${Math.min(100, (status.beats / Math.max(1, status.budget_beats)) * 100)}%` }} />
    </div>
  );
}

/** Whether the run's pings kept the cache warm: only a warming or ended run
 *  says so. */
function keptWarm(status: CacheKeepAliveStatus): boolean {
  return status.state === 'warming' || status.state === 'ended';
}

/** The title of a card's chip and its explainer, and of a warming panel
 *  headline. */
function cacheTitle(status: CacheKeepAliveStatus): string {
  return keptWarm(status) ? 'Cache kept warm' : 'Cache keep-alive';
}

/** The panel section's headline for one status: the card's sentence. Only
 *  pings that run claim warmth here: any other state leaves that to the
 *  engine's warmth read. */
function sectionHeadline(status: CacheKeepAliveStatus): string {
  return keepAliveSentence(status) ?? 'Not kept';
}

const TURNS_BACK_ON = 'It turns back on when the thread runs again.';

/** The panel section's detail, where the headline does not say it all. */
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

/** When the cache of a thread no ping keeps warm goes cold, read from the
 *  engine: a time, `null` once cold, `'unknown'` when no evidence gives its
 *  lifetime, `undefined` until read. A warming run reads nothing, since its
 *  pings keep the cache warm. Every status change reads afresh: an earlier
 *  answer may describe another state. */
function useColdAt(threadId: string, status: CacheKeepAliveStatus): ColdAt | undefined {
  const { loadable } = useLoadableFetch(
    async (): Promise<ColdAt | undefined> => {
      if (status.state === 'warming') return undefined;
      try {
        return coldAtOf(await fetchCacheWarmth(threadId));
      } catch (e) {
        // Best-effort hint, as on the cache-warm line: the headline only drops
        // its warmth words, and the next status change reads again.
        console.warn(`[KeepAliveSection] cache-warmth read failed for thread "${threadId}"`, e);
        throw e;
      }
    },
    [threadId, status],
  );
  return loadable.status === 'loaded' ? loadable.data : undefined;
}

/** When a cache goes cold: a time, `null` once it is cold, or `'unknown'`. */
type ColdAt = number | null | 'unknown';

function coldAtOf(warmth: CacheWarmth): ColdAt {
  switch (warmth.warmth) {
    case 'warm':
      return Date.now() + warmth.remaining_secs * 1000;
    case 'cold':
      return null;
    case 'unknown':
      return 'unknown';
  }
}

/** The headline's warmth sentence: until when the cache stays warm, that it
 *  is cold, or that its lifetime is unknown. Empty until read. */
function warmthSentence(coldAt: ColdAt | undefined, now: number): string {
  if (coldAt === undefined) return '';
  if (coldAt === 'unknown') return ' Its lifetime is unknown.';
  return coldAt !== null && now < coldAt ? ` Warm until ${formatShortTime(new Date(coldAt))}.` : ' The cache is cold.';
}

/** The waiting panel's "Prompt cache" section. It is always labelled, since it
 *  says something different in kind from the lists above it. */
export function KeepAliveSection({ threadId, status }: { threadId: string; status: CacheKeepAliveStatus }) {
  const coldAt = useColdAt(threadId, status);
  const now = useNow(typeof coldAt === 'number' ? coldAt : 0);
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

/** How often a countdown re-renders. Under a minute the text counts seconds. */
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

/** Whether the cache-warm line may read the engine: the thread is idle or
 *  parked on a question with no answer or follow-up in flight. The read says
 *  whether a keep-alive card already speaks for the cache. */
export function cacheWarmLineEligible(thread: ThreadState): boolean {
  return isRenderedThreadIdle(thread);
}

/** One read of the cache-warm line: when the cache goes cold, or that no
 *  evidence gives its lifetime. */
interface WarmRead {
  coldAt: number | 'unknown';
  keepAlive: CacheKeepAliveStatus;
}

/** The card that closes the feed of a thread waiting on the user while its
 *  prompt cache is warm and nothing pings it: an idle thread, or one parked on
 *  a question. The engine says how long is left. Any other status already has
 *  a keep-alive card, so the line draws nothing beside it. A cold cache draws
 *  nothing, and so does a failed read: the next status or event change reads
 *  again. A beat is a `ContextCaptured` on the thread, so it reads again too.
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
        const coldAt = coldAtOf(warmth);
        if (warmth.keep_alive === null || coldAt === null) return null;
        return { coldAt, keepAlive: warmth.keep_alive };
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
  if (!eligible || loadable.status !== 'loaded' || loadable.data === null || loadable.data.keepAlive.state !== 'not_kept') return null;
  return <WarmLineCard read={loadable.data} />;
}

function WarmLineCard({ read }: { read: WarmRead }) {
  const { coldAt, keepAlive } = read;
  const unknown = coldAt === 'unknown';
  const now = useNow(unknown ? 0 : coldAt);
  if (!unknown && now >= coldAt) return null;
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
        detail: unknown ? 'Lifetime unknown' : `Warm until ${formatShortTime(new Date(coldAt))}`,
      })}
    </div>
  );
}

/** The card's, the cache-warm card's and the panel's one ⓘ. It opens the shared
 *  explainer, which says what the cache is, the run's facts and the setting
 *  behind it. The switch stays on the surface that opened it. Its title repeats
 *  that surface's words. */
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

/** What the cache is, and what keeping it warm saves. Only a status with a run
 *  says Lucidos pings it. */
function cacheIntro(status: CacheKeepAliveStatus): string {
  const keeps =
    status.state === 'not_kept'
      ? 'A turn while it is warm reads the context cheaply'
      : 'Lucidos pings it just before the cache expires, so the next turn reads the context cheaply';
  return `The model holds this thread's context in its prompt cache for a while. ${keeps} instead of paying to reread it.`;
}

/** The run's sentence and the meter, then the math or the reread price. Or
 *  the reason nothing keeps the cache warm. */
function KeepAliveRunFacts({ status }: { status: CacheKeepAliveStatus }) {
  if (status.state === 'not_kept') return <p>{`Nothing keeps it warm while you are away. ${notKeptReason(status)}`}</p>;
  const math = keepAliveMath(status);
  const reread = math ? null : rereadSentence(status);
  return (
    <>
      <p>{explainerSentence(status)}</p>
      {status.state === 'warming' ? <KeepAliveMeter status={status} /> : null}
      {math ? <KeepAliveMathFold math={math} /> : null}
      {reread ? <p>{reread}</p> : null}
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

/** The paragraph linking the setting behind the status, ending on its link
 *  with no full stop to wrap. The keep-alive is on or off, so an off status
 *  links the switch. Any other status links the cache cards switch, which
 *  shows only while the keep-alive is on. */
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
