import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { fetchThreadContext, declineSuccessorOffer, type SuccessorImplementer, type ThreadContext } from '../../api/client';
import {
  codingAgentPendingModel, codingAgentPendingReasoningEffort, effectiveThreadStatus, focusedThreadId, showToast,
  successorCardThreadId, successorOffer, threadMap, type SuccessorOffer,
} from '../../store/store';
import { getThreadEventsBump } from '../../store/threadActivity';
import { continueInSuccessor } from '../../store/actions/successor';
import { startFreshDraft } from '../../store/actions/compose';
import type { ThreadState } from '../../store/thread-events';
import { displayModelName } from '../../store/thread-events/exchange';
import { errorDetail } from '../../utils/errorDetail';
import { formatTokens } from '../../utils/formatTokens';
import { Disclosure } from '../shared/Disclosure';
import { OptionIndicator } from './QuestionCard';

type Choice = 'continue' | 'fresh' | 'stay';

/** The events that change what the card says: a thread's context size, and a
 *  *Stay here* that hides it. The engine's verdict also reads whether the
 *  thread holds an open change, so the change state is part of the read key. */
const CARD_EVENTS: ReadonlySet<string> = new Set(['ContextCaptured', 'CodingAgentCompacted', 'SuccessorOfferDeclined']);

/** When to re-read the focused thread's context: once per idle, after its
 *  newest model call, compaction, decline or change of its change state.
 *  `null` while a turn runs, so a busy turn reads nothing. */
function contextReadKey(thread: ThreadState | undefined): string | null {
  if (!thread || thread.meta.channel !== 'claude_code' || effectiveThreadStatus(thread) !== 'idle') return null;
  let newest = -1;
  for (const [seq, event] of thread.events) if (CARD_EVENTS.has(event.type) && seq > newest) newest = seq;
  return `${thread.meta.id}:${newest}:${thread.meta.codingAgentChangeState.kind}`;
}

/** The ratio to a fresh thread, as the card says it: `6x`, `1.4x`. */
function ratioLabel(ratio: number): string {
  return ratio >= 2 ? `${Math.round(ratio)}x` : `${ratio.toFixed(1)}x`;
}

const SUCCESSOR_QUESTION = 'Carry on in a new thread?';

/** What the card says about the thread, from the engine's reading. */
export function successorFinding(context: ThreadContext): string {
  const size = context.input_tokens === null ? 'This thread is large' : `This thread holds ${formatTokens(context.input_tokens)} tokens`;
  const ratio = context.card ? ` (about ${ratioLabel(context.card.fresh_ratio)} a fresh thread)` : '';
  return `${size}, and every reply rereads all of it${ratio}.`;
}

/** The model and effort picked under the card, as the successor will run them.
 *  `undefined` while nothing is picked, so the successor carries the old pair. */
function aimedImplementer(model: string | null, effort: string | null): SuccessorImplementer | undefined {
  if (!model && !effort) return undefined;
  return { ...(model && { coding_agent_model: model }), ...(effort && { reasoning_effort: effort }) };
}

function aimedLabel(model: string | null, effort: string | null): string | null {
  const parts = [model && displayModelName(model), effort].filter(Boolean);
  return parts.length > 0 ? parts.join(' · ') : null;
}

function CardOption({ label, desc, busyDesc, busy, disabled, onPick }: {
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

/** A card on top of the composer offering a *successor thread* to a large
 *  coding-agent thread at a clean idle (`successorOffer`). It is drawn from the
 *  question card's option classes. */
export function SuccessorCard() {
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

  const offer = successorOffer(thread, context, Date.now());
  useEffect(() => {
    const shown = offer ? threadId : null;
    successorCardThreadId.value = shown;
    return () => { if (successorCardThreadId.value === shown) successorCardThreadId.value = null; };
  }, [offer, threadId]);

  // The last card shown, so it keeps its words while it rolls shut.
  const shown = useRef<{ threadId: string; offer: SuccessorOffer; finding: string } | null>(null);
  if (threadId && offer && context) shown.current = { threadId, offer, finding: successorFinding(context) };
  const last = shown.current;

  const model = codingAgentPendingModel.value;
  const effort = codingAgentPendingReasoningEffort.value;
  const aimed = aimedLabel(model, effort);

  async function choose(target: { threadId: string; offer: SuccessorOffer }, choice: Choice): Promise<void> {
    if (choice === 'fresh') {
      const source = threadMap.value.get(target.threadId);
      if (source) startFreshDraft(source.meta);
      return;
    }
    setBusy(choice);
    try {
      if (choice === 'continue') await continueInSuccessor(target.threadId, aimedImplementer(model, effort));
      else await declineSuccessorOffer(target.threadId);
    } catch (err) {
      const what = choice === 'continue' ? 'continue in a new thread' : 'keep this thread';
      showToast(`Could not ${what}: ${errorDetail(err)}`, 'error');
    } finally {
      setBusy(null);
    }
  }

  return (
    <Disclosure open={offer !== null}>
      {last && (
        <div class="question-body successor-card protected-surface" data-offer={last.offer}>
          <div class="question-text">
            <p>{last.finding}</p>
            <p>{SUCCESSOR_QUESTION}</p>
          </div>
          <div class="question-options">
            <CardOption
              label="Continue with context"
              desc={`Carries the branch and what was learned. A message you type goes along as its first request.${aimed ? ` It runs on ${aimed}.` : ''}`}
              busyDesc={last.offer === 'warm' ? 'Writing the handoff…' : 'Starting the new thread…'}
              busy={busy === 'continue'}
              disabled={busy !== null}
              onPick={() => void choose(last, 'continue')}
            />
            <CardOption
              label="Start fresh"
              desc="A plain thread on the same repo, no carry-over. Anything you typed moves to a new draft, unsent."
              busyDesc=""
              busy={false}
              disabled={busy !== null}
              onPick={() => void choose(last, 'fresh')}
            />
            <CardOption
              label="Stay here"
              desc="Hides this card until the thread has grown well past this size."
              busyDesc="Hiding this card…"
              busy={busy === 'stay'}
              disabled={busy !== null}
              onPick={() => void choose(last, 'stay')}
            />
          </div>
        </div>
      )}
    </Disclosure>
  );
}
