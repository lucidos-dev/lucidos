import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import {
  fetchThreadContext, declineSuccessorOffer, withdrawCeilingHandoff,
  type CeilingHandoff, type SuccessorImplementer, type ThreadContext,
} from '../../api/client';
import {
  effectiveThreadStatus, focusedThreadId, showToast, successorCardPick, successorCardThreadId, successorOffer,
  threadMap, type SuccessorOffer,
} from '../../store/store';
import { getThreadEventsBump } from '../../store/threadActivity';
import { draftPresentThreadIds } from '../../store/composeDrafts';
import { continueInSuccessor } from '../../store/actions/successor';
import { startFreshDraft } from '../../store/actions/compose';
import type { StoredEvent, ThreadState } from '../../store/thread-events';
import { displayModelName } from '../../store/thread-events/exchange';
import { isAuxiliaryCapture } from '../../store/thread-events/exchange-grouping';
import { errorDetail } from '../../utils/errorDetail';
import { formatTokens } from '../../utils/formatTokens';
import { Disclosure } from '../shared/Disclosure';
import { OptionIndicator } from './QuestionCard';

type Choice = 'continue' | 'fresh' | 'stay' | 'return';

/** The events that change what the card says: a thread's context size, a
 *  *Stay here* that hides it, and a ceiling handoff recorded or withdrawn.
 *  The engine's verdict also reads whether the thread holds an open change,
 *  so the change state is part of the read key. */
const CARD_EVENTS: ReadonlySet<string> = new Set([
  'ContextCaptured', 'CodingAgentCompacted', 'SuccessorOfferDeclined', 'SessionHandedOff', 'SessionHandoffWithdrawn',
]);

/** Whether `event` can change the card, as `MAIN_SESSION_CAPTURE` in
 *  `engine/successor.rs` reads it. An auxiliary or sub-agent capture leaves
 *  the size alone, and a re-read rolls the card shut and open. */
function movesCard(event: StoredEvent): boolean {
  if (!CARD_EVENTS.has(event.type)) return false;
  if (event.type !== 'ContextCaptured') return true;
  return !isAuxiliaryCapture(event) && !event.parent_tool_use_id && event.usage?.input_tokens != null;
}

/** When to re-read the focused thread's context: once per idle, after its
 *  newest model call, compaction, decline or change of its change state.
 *  `null` while a turn runs, so a busy turn reads nothing. */
function contextReadKey(thread: ThreadState | undefined): string | null {
  if (!thread || thread.meta.channel !== 'claude_code' || effectiveThreadStatus(thread) !== 'idle') return null;
  let newest = -1;
  for (const [seq, event] of thread.events) if (seq > newest && movesCard(event)) newest = seq;
  return `${thread.meta.id}:${newest}:${thread.meta.codingAgentChangeState.kind}`;
}

/** The ratio to a fresh session, as the card says it: `6x`, `1.4x`. */
function ratioLabel(ratio: number): string {
  return ratio >= 2 ? `${Math.round(ratio)}x` : `${ratio.toFixed(1)}x`;
}

const SUCCESSOR_QUESTION = 'Carry on in a fresh session?';

/** What the card says about the thread, from the engine's reading. */
export function successorFinding(context: ThreadContext): string {
  const size = context.input_tokens === null ? 'This thread is large' : `This thread holds ${formatTokens(context.input_tokens)} tokens`;
  const ratio = context.card ? ` (about ${ratioLabel(context.card.fresh_ratio)} a fresh session)` : '';
  return `${size}, and every reply rereads all of it${ratio}.`;
}

/** What the card says about a handoff at the *context ceiling*. */
export function ceilingFinding(handoff: CeilingHandoff): string {
  const { context_tokens: size, ceiling } = handoff.crossing;
  const from = handoff.opening === 'brief' ? 'a brief of that session' : "that session's handoff narrative";
  return `The last session handed off at this thread's ${formatTokens(ceiling)} context ceiling, holding `
    + `${formatTokens(size)} tokens. Your message goes to the fresh session, which starts from ${from}.`;
}

/** What the card shows: the offer of a successor session, or the handoff the
 *  context ceiling already made. */
type CardShown =
  | { kind: 'offer'; offer: SuccessorOffer; finding: string }
  | { kind: 'ceiling'; handoff: CeilingHandoff };

/** The model and effort picked under the card, as the successor session will
 *  run them. `undefined` while nothing is picked, so it keeps the current pair. */
function aimedImplementer(model: string | null, effort: string | null): SuccessorImplementer | undefined {
  if (!model && !effort) return undefined;
  return { ...(model && { coding_agent_model: model }), ...(effort && { reasoning_effort: effort }) };
}

function aimedLabel(model: string | null, effort: string | null): string | null {
  const parts = [model && displayModelName(model), effort].filter(Boolean);
  return parts.length > 0 ? parts.join(' · ') : null;
}

export function CardOption({ label, desc, busyDesc, busy, disabled, onPick }: {
  label: string;
  desc: ComponentChildren;
  busyDesc: string;
  busy: boolean;
  disabled: boolean;
  onPick: () => void;
}) {
  return (
    <button type="button" class="question-option" disabled={disabled} aria-label={`Choose: ${label}`} onClick={onPick}>
      {busy
        ? <span class="mini-spinner question-option-sending" aria-hidden="true" />
        : <OptionIndicator multiSelect={false} selected={false} />}
      <span class="question-option-label">{label}</span>
      <span class="question-option-desc">{busy ? busyDesc : desc}</span>
    </button>
  );
}

/** A card on top of the composer, once the user starts writing in it. It
 *  offers a *successor session* to a large coding-agent thread at a clean idle
 *  (`successorOffer`). While a handoff at the *context ceiling* waits for the
 *  next input, it says so instead: the message goes to the fresh session.
 *  Drawn from the question card's option classes. `onSend` is the composer's
 *  own Send. */
export function SuccessorCard({ onSend }: { onSend: () => void }) {
  const threadId = focusedThreadId.value;
  const thread = threadId ? threadMap.value.get(threadId) : undefined;
  const eventsBump = threadId ? getThreadEventsBump(threadId) : 0;
  const readKey = useMemo(() => contextReadKey(thread), [thread, eventsBump, thread?.meta.status, thread?.meta.codingAgentChangeState.kind]);
  // The latest reading per thread, so switching back to one needs no refetch.
  const [readings, setReadings] = useState<ReadonlyMap<string, { key: string; context: ThreadContext }>>(new Map());
  const [busy, setBusy] = useState<Choice | null>(null);
  const [, flip] = useState(0);

  useEffect(() => {
    if (!readKey || !threadId || readings.get(threadId)?.key === readKey) return;
    let live = true;
    fetchThreadContext(threadId).then(
      (context) => {
        if (live) setReadings((prev) => new Map(prev).set(threadId, { key: readKey, context }));
      },
      (err) => {
        // Best-effort carve-out (frontend.md), so no toast: the user did not
        // ask for this card, and one that cannot be measured is simply
        // absent. The next turn end reads again.
        console.warn('[SuccessorCard] context read failed', err);
      },
    );
    return () => { live = false; };
  }, [readKey]);

  // Only a reading of the thread as it stands now: an older one may predate a
  // compaction, and would open the card just to shut it.
  const reading = threadId ? readings.get(threadId) : undefined;
  const context = reading && reading.key === readKey ? reading.context : null;

  // The cache going cold flips the card once, at the engine's `cold_at`.
  const coldAt = context?.card?.cold_at;
  useEffect(() => {
    const wait = coldAt === undefined ? 0 : Date.parse(coldAt) - Date.now();
    if (!(wait > 0)) return;
    const timer = setTimeout(() => flip((n) => n + 1), wait + 50);
    return () => clearTimeout(timer);
  }, [coldAt]);

  // The card waits until the user starts a message, and shuts if they clear it.
  const writing = threadId !== null && draftPresentThreadIds.value.has(threadId);
  const ceiling = writing ? context?.ceiling_handoff ?? null : null;
  const offer = writing && !ceiling ? successorOffer(thread, context, Date.now()) : null;
  // Keyed on whether the card shows, not on its offer: a warm card turning
  // cold stays open and keeps its pick.
  const cardShows = offer !== null;
  useEffect(() => {
    const shown = cardShows ? threadId : null;
    successorCardThreadId.value = shown;
    return () => {
      if (successorCardThreadId.value === shown) successorCardThreadId.value = null;
      if (shown !== null && successorCardPick.value?.threadId === shown) successorCardPick.value = null;
    };
  }, [cardShows, threadId]);

  // The last card shown, so it keeps its words while it rolls shut.
  const shown = useRef<{ threadId: string; card: CardShown } | null>(null);
  if (threadId && ceiling) shown.current = { threadId, card: { kind: 'ceiling', handoff: ceiling } };
  else if (threadId && offer && context) {
    shown.current = { threadId, card: { kind: 'offer', offer, finding: successorFinding(context) } };
  }
  const last = shown.current;

  const pick = successorCardPick.value;
  const model = pick && pick.threadId === threadId ? pick.model : null;
  const effort = pick && pick.threadId === threadId ? pick.effort : null;
  const aimed = aimedLabel(model, effort);

  async function choose(target: string, choice: Choice): Promise<void> {
    if (choice === 'fresh') {
      const source = threadMap.value.get(target);
      if (source) startFreshDraft(source.meta);
      return;
    }
    setBusy(choice);
    try {
      if (choice === 'continue') await continueInSuccessor(target, aimedImplementer(model, effort));
      else if (choice === 'return') await withdrawCeilingHandoff(target);
      else await declineSuccessorOffer(target);
    } catch (err) {
      showToast(`Could not ${CHOICE_FAILED[choice]}: ${errorDetail(err)}`, 'error');
    } finally {
      setBusy(null);
    }
  }

  const startFresh = last && (
    <CardOption
      label="Start fresh"
      desc="A plain thread on the same repo, no carry-over. Anything you typed moves to a new draft, unsent."
      busyDesc=""
      busy={false}
      disabled={busy !== null}
      onPick={() => void choose(last.threadId, 'fresh')}
    />
  );

  return (
    <Disclosure open={offer !== null || ceiling !== null}>
      {last?.card.kind === 'offer' && (
        <div class="question-body successor-card protected-surface" data-offer={last.card.offer}>
          <div class="question-text">
            <p>{last.card.finding}</p>
            <p>{SUCCESSOR_QUESTION}</p>
          </div>
          <div class="question-options">
            <CardOption
              label="Continue with context"
              desc={`A fresh session in this thread, carrying what was learned. A message you type goes along as its first request.${aimed ? ` It runs on ${aimed}.` : ''}`}
              busyDesc={last.card.offer === 'warm' ? 'Writing the handoff…' : 'Starting the fresh session…'}
              busy={busy === 'continue'}
              disabled={busy !== null}
              onPick={() => void choose(last.threadId, 'continue')}
            />
            {startFresh}
            <CardOption
              label="Stay here"
              desc="Hides this card until the thread has grown well past this size."
              busyDesc="Hiding this card…"
              busy={busy === 'stay'}
              disabled={busy !== null}
              onPick={() => void choose(last.threadId, 'stay')}
            />
          </div>
        </div>
      )}
      {last?.card.kind === 'ceiling' && (
        <div class="question-body successor-card protected-surface" data-offer="ceiling">
          <div class="question-text">
            <p>{ceilingFinding(last.card.handoff)}</p>
          </div>
          <div class="question-options">
            <CardOption
              label="Send to the fresh session"
              desc={`What Send does. The fresh session reads the ${last.card.handoff.opening === 'brief' ? 'brief' : 'narrative'}, then answers you.`}
              busyDesc=""
              busy={false}
              disabled={busy !== null}
              onPick={onSend}
            />
            {startFresh}
            {last.card.handoff.raises_to !== null && (
              <CardOption
                label="Return to the old session"
                desc={`Withdraws the handoff and raises this thread's ceiling to ${formatTokens(last.card.handoff.raises_to)}. Every reply rereads all ${formatTokens(last.card.handoff.crossing.context_tokens)}, and a cold cache rewrites it first.`}
                busyDesc="Returning to the old session…"
                busy={busy === 'return'}
                disabled={busy !== null}
                onPick={() => void choose(last.threadId, 'return')}
              />
            )}
          </div>
        </div>
      )}
    </Disclosure>
  );
}

/** What a failed choice could not do, as its toast says it. */
const CHOICE_FAILED: Record<Exclude<Choice, 'fresh'>, string> = {
  continue: 'continue in a fresh session',
  stay: 'keep this thread',
  return: 'return to the old session',
};
