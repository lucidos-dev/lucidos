import { SESSION_END_REASONS } from '../../generated/thread-lifecycle';
import { hasVisibleText, isMeaningfulText, mergeAdjacentTextEvents } from '../event-rendering';
import { AWAIT_EVENT_TOOL, waitSubscriptionLabels } from './event-waits';
import { describeCCTool, describeEngineTool, exchangeHasCCContent, exchangeResponseText, exchangeUserMessage, fullCommandForCCTool, fullCommandForEngineTool } from './exchange';
import { toolUseIdOf } from './exchange-grouping';
import { IDLE_ENGINE_RESTART_INTERRUPT_REASON, isSwitchTeardownAbort, isUserStoppedWait } from './thread-event-types';
import type { ExchangeStatus } from '../exchange-status';
import type { ContextAssembledData, ContextCapture, ContextSection, ResponseEvent, Step, StepOutcome } from '../types';
import type { Exchange } from './exchange';
import type { ActorMode, EventSubscription, EventWaitCancelCause, SequencedEvent, ThreadEvent } from './thread-event-types';

/** The two projections' step shapes, as far as the resolvers care. */
type StepLike = { outcome: StepOutcome; description?: string; tool_name?: string };

/** How an exchange ended, as far as its pending steps are concerned.
 *  `null` = no terminator yet (or the agent resumed past one).
 *  `'clean'` = ResponseGenerated / CodingAgentIdled: the turn finished, so a
 *  step still pending did run to completion and merely lacks a recorded
 *  result.
 *  `'unclean'` = ResponseFailed / ResponseAborted / ResponseCanceled: the turn
 *  died, so a step still pending never finished and must not be resolved to a
 *  success. One variable rather than a completion flag plus a kind, so
 *  "complete, with a stale kind left over from an earlier terminator" cannot
 *  be represented: every write states both at once, and the last terminator in
 *  the exchange wins (which is also what makes the superseded-abort case, an
 *  abort followed by a same-request ResponseGenerated, come out right). */
type TerminalKind = null | 'clean' | 'unclean';

/** The outcome a terminated exchange's still-pending steps take. */
function pendingOutcomeFor(terminal: TerminalKind): StepOutcome {
  return terminal === 'unclean' ? 'unfinished' : 'success';
}

/** Mark the last pending step as completed.
 *  Walks backwards so parallel tool calls resolve in LIFO order as results arrive.
 *  Optional `pred` narrows which pending step to resolve (e.g. only "Thinking" steps). */
function resolveLastPendingStep(
  steps: StepLike[],
  pred?: (s: StepLike) => boolean,
): void {
  for (let i = steps.length - 1; i >= 0; i--) {
    if (steps[i].outcome === 'pending' && (!pred || pred(steps[i]))) {
      steps[i].outcome = 'success';
      return;
    }
  }
}

/** Force ALL pending steps to `outcome`, so spinners don't persist on finished
 *  exchanges. Called after a completion event with the outcome that fits how
 *  the turn ended (`pendingOutcomeFor`): a clean end resolves them to a
 *  success, a turn that died marks them `'unfinished'` (a green check on a
 *  tool killed mid-execution is a worse lie than the spinner).
 *  Optional `pred` narrows which pending steps to resolve (mirrors
 *  `resolveLastPendingStep`) — used to finalize ONLY the dead 'Thinking' markers
 *  of a handed-off exchange while its tool steps keep spinning. */
function resolvePendingSteps(
  steps: StepLike[],
  outcome: StepOutcome,
  pred?: (s: StepLike) => boolean,
): void {
  for (const step of steps) {
    if (step.outcome === 'pending' && (!pred || pred(step))) step.outcome = outcome;
  }
}

/** A step row that has not named itself yet: the model is thinking and has not
 *  said what it is about to do. Exported because the renderer needs the same
 *  answer the projection does (`InlineStep` shows the reasoning ticker only
 *  while the row is still unnamed), and two definitions of "is this row still a
 *  Thinking marker" would drift the moment either side changed. */
export const isThinking = (s: StepLike) => s.description === 'Thinking';
const isNotThinking = (s: StepLike) => !isThinking(s);
/** The park's own step, i.e. the one the event-wait row replaces. See the
 *  `EventWaitStarted` arm of `exchangeResponseEvents`. */
const isAwaitEventStep = (s: StepLike) => s.tool_name === AWAIT_EVENT_TOOL;

/** Name the pending `Thinking` row after the action the model just produced, so
 *  one LLM call is ONE row in the transcript rather than a resolved
 *  "Thinking ✓" sitting next to the thing that call decided to do.
 *
 *  The row keeps everything it earned before it could name itself: the context
 *  snapshot the call bound to it, the reasoning it streamed, and the legacy
 *  token/message counters old rows carry instead of a snapshot. It stays
 *  `pending`, because the tool it just named is now the thing that is running.
 *
 *  The snapshot survives the rename by ordering, not by luck: the engine emits
 *  `ThoughtStreamed`, then `ContextCaptured`, then `ToolCalled` within one
 *  iteration of the agentic loop, so a main-LLM capture (which binds to a
 *  `Thinking` row by construction, see `bindSnapshotToStep`) has already landed
 *  here by the time the tool call arrives to take the row over.
 *
 *  Only the FIRST action of a thinking pass takes the row: naming it stops it
 *  matching `isThinking`, so parallel tool calls behind it find nothing to fold
 *  onto and push rows of their own. They must, since a result pairs back by
 *  `tool_use_id` and two calls sharing a row could not both be resolved.
 *
 *  Returns false when there is no pending `Thinking` row (a resumed
 *  coding-agent session fires no `CodingAgentPromptSent`; legacy rows have
 *  none), leaving the caller to push a fresh row.
 *
 *  Same replace-in-place shape as the `EventWaitStarted` arm further down, for
 *  the same reason: two rows for one action reads as two actions. */
function nameThinkingRow<T extends StepLike>(rows: T[], naming: Partial<T>): boolean {
  for (let i = rows.length - 1; i >= 0; i--) {
    if (rows[i].outcome === 'pending' && isThinking(rows[i])) {
      Object.assign(rows[i], naming);
      return true;
    }
  }
  return false;
}

/** `nameThinkingRow` for the ResponseEvent projection, whose array also carries
 *  text / image / event-wait rows. */
function nameThinkingStep(
  events: ResponseEvent[],
  naming: Partial<Extract<ResponseEvent, { type: 'step' }>>,
): boolean {
  const idx = lastPendingStepIndex(events, isThinking);
  if (idx < 0) return false;
  Object.assign(events[idx], naming);
  return true;
}

/** Bag of legacy events. All optional — `synthesizeContextCapture`
 *  produces something useful from any subset (Thinking-only is the
 *  oldest case). */
export interface LegacyContextEvents {
  thinking?: { text?: string; context_tokens?: number; context_messages?: number; trimmed?: boolean };
  tokensMeasured?: { input_tokens?: number };
  /** `total_chars` is deliberately absent: it is a CHARACTER count, and the
   *  only thing it was ever used for was standing in for a token total, which
   *  is a different unit. See `synthesizeContextCapture`. The per-section
   *  `char_count`s carry the same information honestly. */
  assembled?: { sections?: ContextSection[]; tools?: string[]; model?: string };
}

/** Default context_window for legacy rows: 200k. Pre-ContextCaptured
 *  events never persisted the budget; under-reporting on the 1M-context
 *  Opus fork is preferable to faking headroom. */
const LEGACY_CONTEXT_WINDOW = 200_000;

export function synthesizeContextCapture(legacy: LegacyContextEvents): ContextCapture {
  const usage = legacy.tokensMeasured?.input_tokens != null
    ? {
        input_tokens: legacy.tokensMeasured.input_tokens,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
      }
    : undefined;
  return {
    producer: 'main_llm',
    model: legacy.assembled?.model ?? '',
    context_window: LEGACY_CONTEXT_WINDOW,
    sections: legacy.assembled?.sections ?? [],
    tools: legacy.assembled?.tools ?? [],
    // NOT `?? legacy.assembled?.total_chars`: that arm put a CHARACTER count
    // into a token field, roughly 2.5x the truth, and the panel presented it
    // as a token total with no hint it was a different unit. `frontend.md`'s
    // "No Silent Defaults" is exactly this: fall back to unknown, not to a
    // plausible value. `ContextCapturePanel` renders a zero headline as no
    // token figure at all while still showing every section's real char
    // count, so the one signal we genuinely have survives and the one we
    // don't have is absent instead of invented.
    estimated_total_tokens: legacy.thinking?.context_tokens ?? 0,
    usage,
    trimmed: legacy.thinking?.trimmed ?? false,
    legacy: true,
  };
}

/** Convert a `ContextCaptured` ThreadEvent into the store-side
 *  `ContextCapture` shape (mostly identity — clamps optional fields).
 *
 *  Propagates `event_id` + `sections_stripped` so the step-detail modal can
 *  lazy-fetch the full sections via `GET /events/:event_id/context` for
 *  snapshot rows the server stripped (`api/threads.rs ::
 *  strip_context_capture_sections`). Live SSE emissions carry full
 *  sections + tools, so `sections_stripped` is absent there. */
function capturedEventToData(
  snap: Extract<ThreadEvent, { type: 'ContextCaptured' }>,
  eventId?: string,
): ContextCapture {
  return {
    producer: snap.producer,
    model: snap.model,
    context_window: snap.context_window,
    sections: snap.sections ?? [],
    tools: snap.tools ?? [],
    estimated_total_tokens: snap.estimated_total_tokens,
    usage: snap.usage,
    trimmed: snap.trimmed ?? false,
    sections_stripped: snap.sections_stripped,
    event_id: eventId,
  };
}

/** Pick which step a ContextCaptured snapshot binds to. Main-LLM emits
 *  fire after a `Thinking` step, so they bind there — the inline
 *  `tokens / window (pct%)` chip then renders next to the request. CC
 *  has no per-API-call Thinking step (CC manages its own loop), so a
 *  CC snapshot binds to whichever step is on top of the stack —
 *  typically the tool that just finished. Used by both `exchangeSteps`
 *  and `exchangeResponseEvents` so the inline chip and summary
 *  projection agree on which step owns each snapshot. The caller
 *  supplies `assign` because Step and the ResponseEvent step variant
 *  share the `contextCapture` field but live in different unions. */
function bindSnapshotToStep<T>(
  data: ContextCapture,
  items: T[],
  isStep: (item: T) => boolean,
  isThinking: (item: T) => boolean,
  assign: (item: T, snap: ContextCapture) => void,
): void {
  // Coding-agent captures anchor to tool steps; main-LLM captures to thinking.
  const acceptable = data.producer === 'main_llm' ? isThinking : isStep;
  for (let i = items.length - 1; i >= 0; i--) {
    if (acceptable(items[i])) {
      assign(items[i], data);
      return;
    }
  }
}

/** Build Step[] from exchange events (tool calls with outcome tracking).
 *  @param _isLast — kept for caller compatibility; spinners are no longer resolved
 *  on `!isLast` alone. A non-last exchange can still be the one the agentic loop
 *  is actively processing (chat mid-flight injection — the parent's
 *  request_event_id keeps attracting events even after the follow-up MR lands),
 *  so resolution waits for either an in-exchange completion event or `threadIdle`.
 *  @param threadIdle — true when CC is not producing output (see
 *  `isThreadQuiescent` in store.ts). Combined with the in-exchange completion
 *  flag to finalize pending steps. */
export function exchangeSteps(exchange: Exchange, _isLast = true, threadIdle = false): Step[] {
  const steps: Step[] = [];
  let terminal: TerminalKind = null;
  let legacyAcc: LegacyContextEvents = {};
  let lastThinkingIdx = -1;
  const refreshLegacySnapshot = () => {
    if (lastThinkingIdx < 0) return;
    steps[lastThinkingIdx].contextCapture = synthesizeContextCapture(legacyAcc);
  };
  for (const { event } of exchange.steps) {
    switch (event.type) {
      case 'MemorySearched': {
        const results = (event as { results?: number }).results ?? 0;
        steps.push({ description: results > 0 ? 'Memory searched' : 'Memory: no results', outcome: 'success' });
        break;
      }
      case 'ThoughtStreamed': {
        // ThoughtStreamed marks "about to invoke the LLM with this context".
        // Stay pending (spinner) until the LLM produces output (TextStreamed /
        // ToolCalled), the next ThoughtStreamed supersedes us, or the thread
        // goes idle. A back-to-back ThoughtStreamed (e.g. internal retry) means
        // the previous LLM call finished without visible output — resolve it ✓.
        resolveLastPendingStep(steps, isThinking);
        const ctx = event as { context_tokens?: number; context_messages?: number; trimmed?: boolean; text?: string };
        legacyAcc = { thinking: ctx };
        steps.push({
          description: 'Thinking',
          outcome: 'pending',
          context_tokens: ctx.context_tokens,
          context_messages: ctx.context_messages,
          trimmed: ctx.trimmed,
        });
        lastThinkingIdx = steps.length - 1;
        if (ctx.context_tokens != null || ctx.context_messages != null) {
          refreshLegacySnapshot();
        }
        break;
      }
      case 'ContextTokensMeasured': {
        const measured = event as { input_tokens: number };
        legacyAcc.tokensMeasured = measured;
        for (let i = steps.length - 1; i >= 0; i--) {
          if (steps[i].description === 'Thinking') {
            steps[i].context_tokens = measured.input_tokens;
            break;
          }
        }
        refreshLegacySnapshot();
        break;
      }
      case 'ContextAssembled': {
        const ctx = event as { sections: ContextSection[]; tools: string[]; model: string; total_chars: number };
        legacyAcc.assembled = ctx;
        refreshLegacySnapshot();
        break;
      }
      case 'ContextCaptured': {
        bindSnapshotToStep(
          capturedEventToData(
            event as Extract<ThreadEvent, { type: 'ContextCaptured' }>,
            event._eventId,
          ),
          steps,
          () => true,
          (s) => s.description === 'Thinking',
          (s, snap) => { s.contextCapture = snap; },
        );
        break;
      }
      case 'TextStreamed':
        // VISIBLE text ends the thinking pass. A blank chunk does not: it puts
        // nothing on screen, and resolving on it would hand the pending row a
        // checkmark that the tool call arriving next can no longer take over.
        if (hasVisibleText((event as { text?: string }).text)) {
          resolveLastPendingStep(steps, isThinking);
        }
        break;
      case 'ToolCalled': {
        // The call the thinking pass produced NAMES the row it came out of,
        // rather than checking that row off and queueing beneath it. See
        // `nameThinkingRow`.
        const e = event as { name: string; args: unknown; description?: string };
        const naming = { description: e.description || describeEngineTool(e.name, e.args) };
        if (!nameThinkingRow(steps, naming)) steps.push({ ...naming, outcome: 'pending' });
        break;
      }
      case 'ToolResult':
        resolveLastPendingStep(steps, isNotThinking);
        break;
      case 'CodingAgentPromptSent':
        steps.push({ description: 'Thinking', outcome: 'pending' });
        break;
      case 'CodingAgentToolCalled': {
        // Names the pending Thinking row, same as the chat arm above.
        const e = event as { name: string; args: unknown; description?: string };
        const naming = {
          description: e.description || describeCCTool(e.name, e.args),
          tool_use_id: toolUseIdOf(event),
        };
        if (!nameThinkingRow(steps, naming)) steps.push({ ...naming, outcome: 'pending' });
        terminal = null; // CC resumed, not finished yet
        break;
      }
      case 'CodingAgentToolResult': {
        // tool_use_id is unique per call; description is ambiguous for parallel
        // calls (two `Read SKILL.md` of different files share a row label).
        // Fallback handles AskUserQuestion: its CodingAgentToolCalled is
        // suppressed (run_session.rs) so no step carries its id, and the
        // ToolResult must not resolve the resume-marker Thinking spinner
        // queued by agent_question.rs — hence isNotThinking on the walker.
        const id = toolUseIdOf(event);
        let resolved = false;
        if (id) {
          for (const step of steps) {
            if (step.outcome === 'pending' && step.tool_use_id === id) {
              step.outcome = 'success';
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) resolveLastPendingStep(steps, isNotThinking);
        break;
      }
      // Mirror of exchangeResponseEvents: the terminator's KIND decides what a
      // still-pending step becomes. See `TerminalKind`.
      case 'ResponseGenerated': case 'CodingAgentIdled':
        terminal = 'clean';
        break;
      case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
        terminal = 'unclean';
        break;
      case 'CodingAgentThoughtStreamed': {
        // Mirror of exchangeResponseEvents: accumulate reasoning into the live
        // "Thinking" step, opening one if none is pending.
        const text = (event as { text?: string }).text ?? '';
        let target: Step | undefined;
        for (let i = steps.length - 1; i >= 0; i--) {
          if (steps[i].outcome === 'pending' && steps[i].description === 'Thinking') {
            target = steps[i];
            break;
          }
        }
        if (!target) {
          target = { description: 'Thinking', outcome: 'pending' };
          steps.push(target);
        }
        target.thinkingText = (target.thinkingText ?? '') + text;
        terminal = null; // CC actively reasoning, not finished
        break;
      }
      case 'CodingAgentTextStreamed':
        // Same visible-text gate as the chat arm above, and this is the arm it
        // was written for: a coding agent emits a blank chunk before EVERY tool
        // call.
        if (hasVisibleText((event as { text?: string }).text)) {
          resolveLastPendingStep(steps, isThinking);
        }
        terminal = null; // CC resumed, not finished yet
        break;
    }
  }
  // See `exchangeResponseEvents` for the outcome split and for why a handed-off
  // exchange finalizes only its Thinking markers.
  if (terminal !== null || threadIdle) resolvePendingSteps(steps, pendingOutcomeFor(terminal));
  else if (exchange.continuationMoved) resolvePendingSteps(steps, 'success', isThinking);
  return steps;
}

/** Index of the last pending step matching `pred`, or -1. The lookup half of
 *  `resolveLastPendingResponseStep`, for the one caller that replaces a step
 *  rather than resolving it. */
function lastPendingStepIndex(events: ResponseEvent[], pred: (s: StepLike) => boolean): number {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.type === 'step' && e.outcome === 'pending' && pred(e)) return i;
  }
  return -1;
}

/** Mark the last pending step in a ResponseEvent[] as completed and return it
 *  so callers can attach extra payload (tool result text, images). Optional
 *  `pred` narrows which pending step to resolve. */
function resolveLastPendingResponseStep(
  events: ResponseEvent[],
  pred?: (s: StepLike) => boolean,
): Extract<ResponseEvent, { type: 'step' }> | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.type === 'step' && e.outcome === 'pending' && (!pred || pred(e))) {
      e.outcome = 'success';
      return e;
    }
  }
  return null;
}

/** Does this text chunk merely repeat what the turn's failure card already
 *  says? An agent that loses its upstream connection reports the error twice:
 *  it streams `API Error: …` as ordinary assistant text before exiting, and the
 *  engine records the same string as the turn's `ResponseFailed`. Drawing both
 *  put one identical sentence on screen as a paragraph and again in the red card
 *  right beneath it (reported 2026-08-07).
 *
 *  The card is the copy that stays: it carries the `ResponseFailed`'s own event
 *  id, which is what makes a notification deep-link resolve to the failure (see
 *  `ExchangeError.eventId`), and `ChatExchange` renders it as a SIBLING of the
 *  response panel, so dropping the paragraph can never hide the error.
 *
 *  Matched per chunk, before `mergeAdjacentTextEvents`: an agent emits the error
 *  as its own chunk, and merging would glue it onto whatever prose preceded it,
 *  leaving nothing that compares equal. Exact (trimmed) equality only, so prose
 *  that merely mentions the failure is the agent talking about it and stays.
 *
 *  This is the BACKSTOP, not the primary defense, and it deliberately stays even
 *  though Claude Code's copy no longer arrives: since 2026-08-10 the engine drops
 *  an *agent error banner* at the parser (`claude_code_parse.rs`), so the echo is
 *  never recorded for CC in the first place. What still reaches here is every
 *  other route to the same duplicate: events already persisted by older engines,
 *  the chat channel, Codex, and a CC that stops flagging its banner. Widening it
 *  past exact equality is the wrong move if the duplicate reappears glued to real
 *  prose, because that shape means a NEW ingestion path is treating a harness
 *  message as model output, and the fix belongs there. */
function failureEchoPredicate(exchange: Exchange): (text: string | undefined) => boolean {
  const failure = exchangeError(exchange)?.message.trim();
  if (!failure) return () => false;
  return text => (text ?? '').trim() === failure;
}

/** Build ResponseEvent[] from exchange events (interleaved text + steps for rendering).
 *  @param _isLast — kept for caller compatibility; no longer drives spinner resolution
 *  on its own. See `threadIdle`.
 *  @param threadIdle — true when CC is not producing output (see
 *  `isThreadQuiescent` in store.ts). Combined with the in-exchange completion
 *  flag to finalize pending steps. A non-last exchange can still be the one
 *  the engine is actively processing (chat mid-flight injection), so
 *  resolution must not trigger purely on `!isLast`. */
export function exchangeResponseEvents(exchange: Exchange, _isLast = true, threadIdle = false): ResponseEvent[] {
  const events: ResponseEvent[] = [];
  const hasCCContent = exchangeHasCCContent(exchange);
  const isFailureEcho = failureEchoPredicate(exchange);
  let terminal: TerminalKind = null;
  // Set when the exchange completed via a text-less ResponseGenerated — a
  // benign empty completion (the model ended its turn cleanly with no text).
  // Drives the neutral "empty response" note pushed after the loop.
  let emptyCompletion = false;
  // One ContextAssembled per exchange; attach to every step pushed after it.
  let currentContext: ContextAssembledData | undefined;
  let legacyAcc: LegacyContextEvents = {};
  const attachLegacyToLastThinking = () => {
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i];
      if (e.type === 'step' && e.description === 'Thinking') {
        e.contextCapture = synthesizeContextCapture(legacyAcc);
        return;
      }
    }
  };
  const pushStep = (step: Extract<ResponseEvent, { type: 'step' }>) => {
    if (currentContext) step.context = currentContext;
    events.push(step);
  };

  for (const { event } of exchange.steps) {
    const created = event.created;
    switch (event.type) {
      case 'MemorySearched': {
        const ms = event as { results?: number; queries?: string[] };
        const results = ms.results ?? 0;
        const detail = ms.queries?.length ? ms.queries.join(', ') : undefined;
        pushStep({ type: 'step', description: results > 0 ? 'Memory searched' : 'Memory: no results', outcome: 'success', detail, created });
        break;
      }
      case 'ThoughtStreamed': {
        // Mirror of exchangeSteps: stay pending (spinner) until next visible
        // output supersedes us. See the longer comment there.
        resolveLastPendingResponseStep(events, isThinking);
        const ctx = event as { context_tokens?: number; context_messages?: number; trimmed?: boolean };
        legacyAcc = { thinking: ctx };
        pushStep({
          type: 'step',
          description: 'Thinking',
          outcome: 'pending',
          context_tokens: ctx.context_tokens,
          context_messages: ctx.context_messages,
          trimmed: ctx.trimmed,
          created,
        });
        if (ctx.context_tokens != null || ctx.context_messages != null) {
          attachLegacyToLastThinking();
        }
        break;
      }
      case 'ContextTokensMeasured': {
        const measured = event as { input_tokens: number };
        legacyAcc.tokensMeasured = measured;
        for (let i = events.length - 1; i >= 0; i--) {
          const e = events[i];
          if (e.type === 'step' && e.description === 'Thinking') {
            e.context_tokens = measured.input_tokens;
            break;
          }
        }
        attachLegacyToLastThinking();
        break;
      }
      case 'ContextAssembled': {
        const ctx = event as { sections: ContextSection[]; tools: string[]; model: string; total_chars: number };
        legacyAcc.assembled = ctx;
        currentContext = {
          sections: ctx.sections,
          tools: ctx.tools,
          model: ctx.model,
          total_chars: ctx.total_chars,
        };
        attachLegacyToLastThinking();
        break;
      }
      case 'ContextCaptured': {
        bindSnapshotToStep(
          capturedEventToData(
            event as Extract<ThreadEvent, { type: 'ContextCaptured' }>,
            event._eventId,
          ),
          events,
          (e) => e.type === 'step',
          (e) => e.type === 'step' && e.description === 'Thinking',
          (e, snap) => { if (e.type === 'step') e.contextCapture = snap; },
        );
        break;
      }
      case 'ToolCalled': {
        // Mirror of exchangeSteps: the call names the Thinking row it came out
        // of instead of opening a second row. See `nameThinkingRow`.
        const e = event as { name: string; args: unknown; description?: string };
        const naming = {
          description: e.description || describeEngineTool(e.name, e.args),
          tool_name: e.name,
          full: fullCommandForEngineTool(e.name, e.args),
          created,
        };
        if (!nameThinkingStep(events, naming)) pushStep({ type: 'step', outcome: 'pending', ...naming });
        break;
      }
      case 'ToolResult': {
        const toolResult = event as { name?: string; result?: string; images?: string[]; result_stripped?: boolean };
        // Skip pending Thinking — ToolCalled already resolved it; this should
        // pair with the matching tool step.
        //
        // `await_event` is narrowed to its OWN step: its result fills the
        // rendezvous slot of a park whose step the event-wait row has already
        // replaced, so there is normally nothing left to resolve, and the
        // generic "last pending step" walk would tick off whatever call the
        // woken turn has since started. It still resolves the real thing on the
        // rejected-subscription path, where no row replaced the step.
        const resolved = resolveLastPendingResponseStep(
          events,
          toolResult.name === AWAIT_EVENT_TOOL ? isAwaitEventStep : isNotThinking,
        );
        if (resolved) {
          if (toolResult.result !== undefined) resolved.result = toolResult.result;
          if (toolResult.images?.length) resolved.result_images = toolResult.images;
          // Always stamp the source event id so a future re-fetch path
          // (debug / "copy event id" / partial-strip thresholds) can
          // address this step. Snapshot replays of stripped rows
          // additionally stamp `result_stripped`, which the step-detail
          // modal gates the lazy-fetch on (see `ResultArea` in
          // `StepDetailModal.tsx`). Live SSE leaves `result_stripped`
          // absent; the modal renders the inline `result` instead.
          if (event._eventId) resolved.result_event_id = event._eventId;
          if (toolResult.result_stripped) resolved.result_stripped = true;
        }
        // Render generated images inline. Only `generate_image` ever puts bytes
        // in a ToolResult (the `[GENERATED_IMAGE:]` sentinel), so the resolved
        // step's un-elided primary arg IS the prompt: carry it onto the image so
        // it can describe itself in a tooltip and in its alt text.
        if (toolResult.images?.length) {
          const prompt = resolved?.tool_name === 'generate_image' ? resolved.full : undefined;
          for (const b64 of toolResult.images) {
            events.push({ type: 'image', base64: b64, mime_type: 'image/jpeg', ...(prompt ? { prompt } : {}) });
          }
        }
        break;
      }
      case 'TextStreamed': {
        // Mirror of exchangeSteps: only VISIBLE text ends the thinking pass.
        const text = (event as { text: string }).text;
        if (hasVisibleText(text)) resolveLastPendingResponseStep(events, isThinking);
        if (!isFailureEcho(text)) events.push({ type: 'text', md: text });
        break;
      }
      case 'SessionStarted':
        if (hasCCContent) events.push({ type: 'section_break', channel: 'claude_code' });
        break;
      case 'CodingAgentPromptSent':
        pushStep({ type: 'step', description: 'Thinking', outcome: 'pending', created });
        break;
      case 'CodingAgentToolCalled': {
        const e = event as { name: string; args: unknown; description?: string };
        const naming = {
          description: e.description || describeCCTool(e.name, e.args),
          tool_name: e.name,
          tool_use_id: toolUseIdOf(event),
          full: fullCommandForCCTool(e.name, e.args),
          created,
        };
        if (!nameThinkingStep(events, naming)) pushStep({ type: 'step', outcome: 'pending', ...naming });
        terminal = null; // CC resumed, not finished yet
        break;
      }
      case 'CodingAgentToolResult': {
        // See exchangeSteps for the pairing rationale.
        const id = toolUseIdOf(event);
        const ccResult = (event as { result?: string }).result;
        let resolved = false;
        if (id) {
          for (const e of events) {
            if (e.type === 'step' && e.outcome === 'pending' && e.tool_use_id === id) {
              e.outcome = 'success';
              if (ccResult !== undefined) e.result = ccResult;
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) {
          const fallback = resolveLastPendingResponseStep(events, isNotThinking);
          if (fallback && ccResult !== undefined) fallback.result = ccResult;
        }
        break;
      }
      case 'CodingAgentThoughtStreamed': {
        // Streamed reasoning — accumulate into the live "Thinking" step. If none
        // is pending (a resumed session's initial prompt fires no
        // CodingAgentPromptSent), open one so reasoning is visible from the first
        // token — the fix for a long think reading as a frozen "Working".
        const text = (event as { text?: string }).text ?? '';
        let target: Extract<ResponseEvent, { type: 'step' }> | undefined;
        for (let i = events.length - 1; i >= 0; i--) {
          const e = events[i];
          if (e.type === 'step' && e.outcome === 'pending' && e.description === 'Thinking') {
            target = e;
            break;
          }
        }
        if (!target) {
          target = { type: 'step', description: 'Thinking', outcome: 'pending', created };
          pushStep(target);
        }
        target.thinkingText = (target.thinkingText ?? '') + text;
        terminal = null; // CC actively reasoning, not finished
        break;
      }
      case 'CodingAgentTextStreamed': {
        const text = (event as { text: string }).text;
        if (hasVisibleText(text)) resolveLastPendingResponseStep(events, isThinking);
        if (!isFailureEcho(text)) events.push({ type: 'text', md: text });
        terminal = null; // CC resumed, not finished yet
        break;
      }
      case 'CodingAgentUserMessageSent':
        // Legacy event — now an exchange boundary in groupIntoExchanges, never a step
        break;
      case 'CommandCheckpointed': {
        // Command guard snapshot pair around a ReversibleDanger command (ADR
        // 0002, Phase 4). Renders inline with a one-click Undo and the diff.
        const e = event as {
          checkpoint_id: string;
          command: string;
          summary: string;
          restores?: number;
          removes?: number;
        };
        events.push({
          type: 'checkpoint',
          checkpoint_id: e.checkpoint_id,
          command: e.command,
          summary: e.summary,
          reverted: false,
          // Absent on pre-2026-08-06 events, where Undo was restore-only and
          // no counts were recorded. 0 renders as "unknown", not as "none".
          restores: e.restores ?? 0,
          removes: e.removes ?? 0,
        });
        break;
      }
      case 'EventWaitStarted': {
        // The park, as the transcript's ONE record of it. It is a step-level
        // row, not a divider: the attached wake resumes THIS exchange, and its
        // steps land below this row.
        //
        // It REPLACES the pending step the `await_event` `ToolCalled` pushed a
        // moment earlier rather than queueing under it. That step's engine
        // description is `Waiting: <reason>` and this row names the same
        // reason, so rendering both put two near-identical lines in the
        // transcript for one action; this is the richer of the two (it carries
        // the subscription and the resolution state), so it is the one that
        // survives. A rejected subscription
        // emits no `EventWaitStarted` at all, so a failed `await_event` keeps
        // its ordinary tool step and its error.
        const e = event as {
          wait_id: string;
          on: EventSubscription[];
          reason: string;
          expires_at: string;
        };
        const row: ResponseEvent = {
          type: 'event_wait',
          wait_id: e.wait_id,
          subscriptions: waitSubscriptionLabels(e.on),
          reason: e.reason,
          expires_at: e.expires_at,
          state: 'waiting',
        };
        const parked = lastPendingStepIndex(events, isAwaitEventStep);
        if (parked >= 0) events[parked] = row;
        else events.push(row);
        // Registering the wait is the whole of the turn's last action, and
        // `await_event` is terminal, so the engine emits no terminator here by
        // design. `exchangeStatus` reads the unresolved park directly.
        terminal = null;
        break;
      }
      case 'EventWaitDelivered':
      case 'EventWaitExpired': {
        // Both of these RESOLVE the arming row in place, matched by wait_id:
        // same subject line, now carrying its outcome (and, for a delivery, the
        // event that matched plus the jump to it). They enrich the row rather
        // than relabelling it, which is why they are allowed to touch it at all.
        //
        // Either can arrive in a LATER exchange than the row that armed it,
        // since a subscription outlives its turn, in which case there is nothing
        // here to resolve and nothing more to draw: both WAKE the thread, so the
        // wake already reads as its own turn further down.
        const e = event as {
          wait_id: string;
          event_type?: string;
          event_id?: string;
        };
        const state = event.type === 'EventWaitDelivered' ? 'woke' : 'timed_out';
        for (const prior of events) {
          if (prior.type === 'event_wait' && prior.wait_id === e.wait_id) {
            prior.state = state;
            if (state === 'woke') {
              prior.matched_event_type = e.event_type;
              prior.matched_event_id = e.event_id;
            }
            break;
          }
        }
        break;
      }
      case 'EventWaitCanceled': {
        // **A stop never rewrites the arming row.** "Set up an event wait: X" is
        // a true statement about a moment, and a stop is a different action at a
        // different moment, routinely hours later. Flipping the row in place
        // relabelled it "Stopped waiting: X" and left that struck line sitting
        // above the agent's own "I'm now watching for X" prose, with nothing
        // anywhere saying when the watch actually ended (reported 2026-08-07).
        //
        // Where the stop DOES appear depends on who stopped it. A user stop is a
        // boundary and renders as its own turn (see `isExchangeStartEvent`), so
        // it draws no row here at all. Every other cause is somebody acting
        // inside a turn, most sharply the agent standing its own watch down, and
        // gets a row at the position it happened.
        if (isUserStoppedWait(event)) break;
        const e = event as {
          wait_id: string;
          on?: EventSubscription[];
          reason?: string;
          cause?: EventWaitCancelCause;
        };
        events.push({
          type: 'event_wait',
          wait_id: e.wait_id,
          // Self-contained on the event since 2026-08-07. An older row carries
          // neither, and the row then names no subscription rather than
          // inventing one.
          subscriptions: e.on ? waitSubscriptionLabels(e.on) : [],
          reason: e.reason ?? '',
          // The deadline died with the subscription, so there is nothing to
          // count down to. The row renders its note, not a countdown.
          expires_at: '',
          state: 'canceled',
          cause: e.cause,
        });
        break;
      }
      case 'CommandCheckpointReverted': {
        // The paired revert carries the checkpoint's request_event_id, so it
        // groups into this exchange after its checkpoint — flip the card's state.
        const id = (event as { checkpoint_id: string }).checkpoint_id;
        for (const prior of events) {
          if (prior.type === 'checkpoint' && prior.checkpoint_id === id) {
            prior.reverted = true;
            break;
          }
        }
        break;
      }
      // The terminator's KIND decides what a still-pending step becomes: a
      // turn that finished leaves a step that ran to completion without a
      // recorded result, a turn that DIED leaves one that never finished. See
      // `TerminalKind`.
      case 'ResponseGenerated':
        terminal = 'clean';
        // A text-less ResponseGenerated is a benign empty completion. The
        // [ENGINE-LIMIT] cap path also emits a ResponseGenerated with no
        // preceding TextStreamed, but with non-empty text — so checking the
        // event's own text distinguishes the two.
        emptyCompletion = !(event as { text?: string }).text?.trim();
        break;
      case 'CodingAgentIdled':
        terminal = 'clean';
        break;
      case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
        terminal = 'unclean';
        break;
      // ChangeApplied/Discarded/Reverted/ApplyFailed and UserQuestionAsked/
      // CodingAgentPermissionRequest/CredentialRequested/McpConsentRequested
      // are exchange-STARTERS (see EXCHANGE_START_TYPES) — they render as their
      // own initiator panels and never reach this loop as steps. The matching
      // resolution events (UserQuestionAnswered, CodingAgentPermissionResolved)
      // become steps of the divider exchange and are handled by describeInitiator
      // from the userEvent's exchange — no per-step ResponseEvent synthesis here.
      case 'SessionEnded':
        break;
    }
  }
  // Resolve pending spinners on finished exchanges (missing ToolResult from
  // killed sessions, parallel tool calls with lost results, or non-last
  // exchanges that were genuinely abandoned). Mid-flight chat injection means
  // a non-last exchange can still be the one the agentic loop is actively
  // processing, so we DON'T resolve purely on `!isLast` — wait for the
  // exchange's terminator OR for the thread to go idle.
  //
  // WHAT they resolve TO is the terminator's kind (`pendingOutcomeFor`): a
  // clean end means the step finished and only its result event is missing, a
  // turn that died means it never finished and is marked `'unfinished'`.
  // `threadIdle` on its own keeps resolving to a success deliberately: the
  // quiescent set includes `waiting_for_user_answer`, so a thread merely
  // parked on a question card would otherwise paint "did not finish" over
  // work that is about to resume.
  //
  // The turn ending is not the only way a spinner strands: when the fold hands
  // a RUNNING turn to a later exchange (`Exchange.continuationMoved` — a child
  // completion card / divider took the redirect), the LLM's next output lands
  // there, so a Thinking marker pending here is already dead. Finalize just
  // those — a pending TOOL step is still owed a result that re-routes back by
  // tool id (the `ask_user_question` spinner that must keep running while the
  // card is on screen).
  const finalizeAll = terminal !== null || threadIdle;
  if (finalizeAll || exchange.continuationMoved) {
    const stepEvents = events.filter(e => e.type === 'step') as StepLike[];
    if (finalizeAll) resolvePendingSteps(stepEvents, pendingOutcomeFor(terminal));
    else resolvePendingSteps(stepEvents, 'success', isThinking);
    // Strip trailing Thinking steps — noise from CC processing notifications
    // (e.g., post-ChangeApplied) without producing output. Keep at least one
    // event so canceled/aborted exchanges still show .response-content.
    while (events.length > 1) {
      const last = events[events.length - 1];
      if (last.type === 'step' && isThinking(last)) {
        events.pop();
      } else {
        break;
      }
    }
    // Benign empty completion: the turn finished cleanly with no text and no
    // images (tool steps may still be present — the model acted but didn't
    // summarise). State that plainly instead of leaving a blank body.
    if (
      emptyCompletion
      && !events.some(isMeaningfulText)
      && !events.some(e => e.type === 'image')
    ) {
      events.push({ type: 'empty' });
    }
  }
  return mergeAdjacentTextEvents(events);
}

/** Will rendering these response events actually DRAW anything?
 *
 *  The mirror of `renderResponseEvents` in `ChatExchange.tsx`, which draws a
 *  `text` event only when it is `isMeaningfulText` and every other kind
 *  unconditionally. So the one non-drawing shape is a blank `text`, and it is
 *  not hypothetical: `exchangeResponseEvents` pushes one for EVERY
 *  `CodingAgentTextStreamed`, and a subprocess being torn down signs off with a
 *  bare `"\n\n"`. The "non-empty after trimming" rule is deliberately taken from
 *  `isMeaningfulText` rather than re-spelled, so this and the renderer cannot
 *  drift on what counts as visible text.
 *
 *  Exists because `events.length > 0` is not the same question. An abort
 *  boundary that acquired only that flush answered yes and rendered a response
 *  panel with an empty body, whose sole visible content was a status badge that
 *  read "Working" while the engine was down (reported 2026-08-06). A caller
 *  deciding whether a panel is worth showing wants this; a caller asking
 *  whether the turn produced events wants `length`.
 *
 *  An `event_wait` counts as drawn because it IS drawn: it is a marker, not step
 *  mechanics (`isStepMechanics`), so no toggle can hide it. A `step` counts even
 *  though `renderResponseEvents` gates it on the `showSteps` toggle, because the
 *  response header always carries the steps control that reveals it, so the
 *  panel is neither empty nor a dead end. */
export function hasRenderableResponseContent(events: ResponseEvent[]): boolean {
  return events.some(e => e.type !== 'text' || isMeaningfulText(e));
}

/** User-facing presentation of a `StepOutcome`. Both the inline-step row and
 *  the detail modal consume this: the outcome IS the CSS class (which drives
 *  the icon and the row's treatment), and this adds the label.
 *
 *  'Did not finish' is deliberately not 'Failed': a step killed mid-execution
 *  never reported anything, while 'Failed' asserts it ran and returned an
 *  error. */
export function stepStatus(outcome: StepOutcome): { label: string; icon: string; className: StepOutcome } {
  switch (outcome) {
    // In-progress rows show no leading icon: the shimmering description is the
    // "live" affordance (see `.inline-step.pending .step-icon` in steps.css).
    case 'pending': return { label: 'In progress', icon: '', className: 'pending' };
    case 'success': return { label: 'Completed', icon: '✓', className: 'success' };
    case 'error': return { label: 'Failed', icon: '⚠', className: 'error' };
    case 'unfinished': return { label: 'Did not finish', icon: '⊘', className: 'unfinished' };
  }
}

/** Whether a non-last exchange's response panel should be hidden as visual
 *  noise. The next exchange's user message implies the chronological flow,
 *  so a panel that produced no real output isn't worth a "Done ↳"
 *  placeholder.
 *
 *  An exchange counts as empty if it has no response text and every event is
 *  either a bare 'Thinking' step or a text event that contributes no visible
 *  output. CC follow-ups race the user: the loop emits a Thinking marker
 *  (and sometimes a whitespace-only text header) before producing any tool
 *  call or text, leaving an interrupted exchange with stray steps that say
 *  nothing the status indicator doesn't already. */
export function isEmptyContinuedExchange(
  status: ExchangeStatus,
  hasResponse: boolean,
  events: ResponseEvent[],
  isLast: boolean,
): boolean {
  if (isLast) return false;
  if (status !== 'done' && status !== 'interrupted') return false;
  if (hasResponse) return false;
  return events.every(e =>
    (e.type === 'step' && isThinking(e)) || (e.type === 'text' && !isMeaningfulText(e))
  );
}

/** A `UserQuestionAsked` divider exchange whose question card was
 *  cancel-stamped (user clicked Cancel, or `archive_thread` resolved the
 *  pending question before tearing the thread down). No CC resume is coming
 *  — `ensure_resume_after_answer` short-circuits on `AnswerKind::Canceled` —
 *  so the response panel would otherwise strand as an empty "Working" badge
 *  or a stray "Thinking ✓" placeholder. The QuestionCard's own disabled red
 *  ✓ Cancel button + struck-through options already tells the story. */
export function isCanceledQuestionDivider(exchange: Exchange): boolean {
  if (exchange.userEvent.type !== 'UserQuestionAsked') return false;
  return exchange.steps.some(({ event }) =>
    event.type === 'UserQuestionAnswered' && event.answer.kind === 'Canceled'
  );
}

/** Change-lifecycle banner exchanges whose body may carry a post-boundary CC
 *  continuation. Excludes `ChangeApplyFailed` — its initiator renders the error
 *  and the change stays pending, so it has no "continued work" body. */
const CHANGE_CONTINUATION_PANELS: ReadonlySet<string> = new Set([
  'ChangeApplied',
  'ChangeDiscarded',
  'ChangeReverted',
]);

/** True for a change-lifecycle banner exchange (Applied/Discarded/Reverted)
 *  that ALSO carries coding-agent work as steps — the session kept going after
 *  the apply and produced more text/tool calls (usually a follow-up proposal)
 *  with no new user message to anchor a fresh exchange, so those events folded
 *  into the change exchange. The banner normally suppresses its response body
 *  (`showResponsePanel` in ChatExchange); when this is true the body must
 *  render, or the continued work is invisible between two "Change applied" rows
 *  (real thread 76b4ee76). Idle/snapshot-only steps don't count — only genuine
 *  CC output, matching what `exchangeResponseEvents` would actually render. */
export function changePanelHasContinuation(exchange: Exchange): boolean {
  if (!CHANGE_CONTINUATION_PANELS.has(exchange.userEvent.type)) return false;
  return exchange.steps.some(({ event }) =>
    event.type === 'CodingAgentTextStreamed'
    || event.type === 'CodingAgentToolCalled'
    || event.type === 'TextStreamed'
    || event.type === 'ToolCalled',
  );
}

/** The failure card's content: what a `ResponseFailed` step says, and which
 *  event said it. */
export interface ExchangeError {
  /** `ResponseFailed.error`, the text rendered in the card. */
  message: string;
  /** The `ResponseFailed`'s OWN event id. `ChatExchange` stamps it on the card
   *  as `data-event-id`, which is what makes a notification deep-link to a
   *  failure resolve (`scrollToEventAndPulse`) and pulse the card itself.
   *  `ResponseFailed` is a step, not an exchange starter, so the exchange root's
   *  `data-event-id` carries a different event entirely. Absent on legacy rows
   *  and on the client-synthesized transport-failure event (`actions/chat.ts`),
   *  neither of which any notification points at. */
  eventId?: string;
}

/** The error a failed exchange carries, or null when it didn't fail. */
export function exchangeError(exchange: Exchange): ExchangeError | null {
  for (const { event } of exchange.steps) {
    if (event.type === 'ResponseFailed') return { message: event.error, eventId: event._eventId };
  }
  return null;
}

/** Every event id this exchange puts into the DOM as `data-event-id`, i.e. the
 *  complete set a deep-link can resolve against inside this turn.
 *
 *  There are exactly two today and both live in `ChatExchange`: the root carries
 *  the turn's STARTER, and the failure card carries its own `ResponseFailed`
 *  (see `ExchangeError.eventId` above). Every other step is deliberately
 *  unstamped, inline steps most of all, since the "Show steps" toggle can hide
 *  them and an id there would resolve only sometimes.
 *
 *  Declared here rather than inferred at the deep-link site so the stamping rule
 *  has ONE definition: `deepLinkAnchorForEvent` reads it to decide whether an
 *  event addresses itself, and a source-scan tripwire in
 *  `__tests__/deep-link-anchor.test.ts` asserts the `data-event-id` expressions
 *  in `ChatExchange.tsx` are exactly the two below. (A scan, not a render: there
 *  is no jsdom in the test infra, matching the `skeleton-guard` /
 *  `list-row-prose-guard` precedent.) Add a third stamp to the component and
 *  that tripwire fails until it is declared here too. */
export function stampedEventIds(exchange: Exchange): string[] {
  const ids: string[] = [];
  // Both stamps are conditional in the component (Preact drops an `undefined`
  // attribute), so an id the DOM will not carry must not be listed here either.
  if (exchange.userEvent._eventId) ids.push(exchange.userEvent._eventId);
  const failure = exchangeError(exchange)?.eventId;
  if (failure) ids.push(failure);
  return ids;
}

/** The `data-event-id` a deep-link to `eventId` should actually target within
 *  `exchanges`, or `null` when no exchange holds that event.
 *
 *  An event that stamps its own element (see `stampedEventIds`) is its own
 *  target, so the pulse stays on the thing the user was sent to see. Anything
 *  else is a step that renders no addressable element of its own, so the target
 *  becomes the turn that CONTAINS it: landing on the turn is the honest answer,
 *  and it is the difference between a link that works and one that spends the
 *  4s resolve deadline and recovers to the bottom of the thread.
 *
 *  This is what the *event wait* step's "show it" needs. A wait can match ANY
 *  event type, and the common match by far is a `CodingAgentIdled` from another
 *  thread, which stamps nothing anywhere. Notification deep-links do not need it
 *  (they point at events that are addressable by construction) and deliberately
 *  do not use it. */
export function deepLinkAnchorForEvent(
  exchanges: Exchange[],
  eventId: string,
): string | null {
  // Backward walk, matching `findExchangeByAnchorId`: on the vanishingly rare
  // id collision the most recent owner is the one on screen.
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const exchange = exchanges[i];
    if (stampedEventIds(exchange).includes(eventId)) return eventId;
    if (exchange.steps.some(({ event }) => event._eventId === eventId)) {
      // A turn whose own starter is unstamped (a legacy row with no event id)
      // gives the deep-link nothing to aim at, and saying so beats returning an
      // `undefined` that would read as "not in this thread" further down.
      return exchange.userEvent._eventId ?? null;
    }
  }
  return null;
}

/** True when this event is an abort the engine has PROMISED to resume: the
 *  teardown boundary of a user-initiated *Switch to new version*. The event-shaped
 *  reading of `isSwitchTeardownAbort`, which is where the fingerprint itself is
 *  defined and cross-referenced against the backend.
 *
 *  This is why the Continue button is withheld (see `continuableAbortIndex`), and
 *  it is the same predicate that decides the thread's `paused` status on the
 *  backend, so the dot and the button can never contradict each other. The
 *  promise is kept or withdrawn by the engine, never guessed at here: a boot that
 *  declines to resume the thread emits a fresh `recovery_after_restart` abort,
 *  which does not match this, re-arming the button and turning the dot red. */
export function abortPromisesAutoResume(ev: ThreadEvent): boolean {
  return ev.type === 'ResponseAborted' && isSwitchTeardownAbort(ev.actor, ev.cause);
}

/** Index of the newest ResponseAborted exchange the user may Continue from, or
 *  `null` when the thread offers no Continue button. Used by AbortPanel: only
 *  this exchange renders the button, and older aborts the user already
 *  continued past are inert.
 *
 *  Four ways the scan ends in `null`:
 *
 *  - A later `ContinuationStarted`: the turn was already resumed.
 *  - A stale-settle abort (engine cleanup of a stuck-but-already-gone process,
 *    fired by the user's Stop/Apply/Discard/Archive/Interrupt click). Clicking
 *    Continue would re-run work the user just deliberately stopped.
 *  - A switch-teardown abort (see `abortPromisesAutoResume`): the engine is
 *    auto-resuming this turn, so offering the button races its own recovery.
 *    That race is what the user hit on 2026-08-05: the button sat there for the
 *    whole teardown-plus-restart window and their click landed nine seconds in,
 *    turning an engine-attributed "Resumed after engine restart" into a
 *    human-attributed "Continued the response".
 *  - The abort boundary itself has since RESOLVED: a terminal event landed
 *    among its steps, so a turn already ran under it and finished. Continue
 *    there re-runs completed work, which on 2026-08-06 meant offering to redo a
 *    turn that had applied a change and spawned a sub-thread two minutes
 *    earlier (real thread ebc787a4). The scan does not stop at such a boundary:
 *    an OLDER unresolved abort above it is still legitimately continuable. The
 *    recovery marker is the one terminal that does NOT resolve a boundary, see
 *    `abortBoundaryResolved`. */
export function continuableAbortIndex(exchanges: Exchange[]): number | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ev = exchanges[i].userEvent;
    if (ev.type === 'ContinuationStarted') return null;
    if (ev.type === 'ResponseAborted') {
      if (ev.cause === 'stale_settle') return null;
      if (abortPromisesAutoResume(ev)) return null;
      if (abortBoundaryResolved(exchanges[i])) continue;
      return i;
    }
  }
  return null;
}

/** Did a turn run under this abort boundary and finish? Any terminal among its
 *  steps says yes, whatever ended it, with exactly one exception.
 *
 *  Crash recovery emits its boundary and its own marker as a pair: a
 *  `recovery_after_restart` abort, then a synthetic
 *  `CodingAgentIdled { reason: engine_restart_interrupt }` whose whole purpose
 *  is to say "this session was interrupted, offer Continue"
 *  (`agent_recovery/recovery.rs`). `CodingAgentIdled` does not start an
 *  exchange, so that marker folds into the abort as a step and looked exactly
 *  like a finished turn. Reading the engine's offer as its own refusal withheld
 *  Continue from every coding-agent thread a restart touched, which on
 *  2026-08-07 was all of them at once. A turn that genuinely ran under the
 *  boundary and idled carries no reason, so it still resolves. */
function abortBoundaryResolved(exchange: Exchange): boolean {
  return exchange.steps.some(({ event }) =>
    TERMINAL_EVENT_TYPES.has(event.type) && !isRecoveryInterruptMarker(event));
}

/** The synthetic idle crash recovery stamps under its own abort boundary. */
function isRecoveryInterruptMarker(event: ThreadEvent): boolean {
  return event.type === 'CodingAgentIdled'
    && event.reason === IDLE_ENGINE_RESTART_INTERRUPT_REASON;
}

const TERMINAL_EVENT_TYPES: ReadonlySet<string> = new Set([
  'ResponseGenerated',
  'ResponseFailed',
  'ResponseCanceled',
  'ResponseAborted',
  'CodingAgentIdled',
]);

/** Read the engine note (UserPromptInjected step) from a ContinuationStarted
 *  exchange. Returns the full text and a coarse count of bullet entries for
 *  the subline ("Reminded the model about N prior tool calls"). Returns null
 *  when no engine note is present (e.g., CC resume path). */
export function resumeEngineNote(exchange: Exchange): { text: string; toolCount: number } | null {
  for (const { event } of exchange.steps) {
    if (event.type === 'UserPromptInjected' && (event as { mode?: ActorMode }).mode === 'engine') {
      const text = (event as { text: string }).text || '';
      // Count bullet lines that look like "- name(args) → result" — the engine
      // note format from chat/rerun.rs::build_side_effect_summary.
      let toolCount = 0;
      for (const line of text.split('\n')) {
        const trimmed = line.trim();
        if (trimmed.startsWith('- ') && trimmed.includes(' → ')) toolCount++;
      }
      return { text, toolCount };
    }
  }
  return null;
}

/** SessionEnded reasons that represent deliberate lifecycle events, NOT system
 *  interruptions. Derived from the generated contract — `shutdown` and `panic`
 *  are system interruptions; `closed` is the user closing a thread (deliberate
 *  but terminal). Removed pre-Phase-4 reasons (`completed`, `changes_proposed`,
 *  `changes_applied`, `auto_ended`, `user_ended`, `stale_resume`, `discarded`)
 *  still appear on legacy DB rows and were considered normal lifecycle ends —
 *  preserved here as plain strings so historical exchanges render the same as
 *  before. */
const NORMAL_SESSION_END_REASONS: ReadonlySet<string> = new Set<string>([
  ...SESSION_END_REASONS.filter(r => r !== 'shutdown' && r !== 'panic'),
  'completed',
  'changes_proposed',
  'changes_applied',
  'auto_ended',
  'user_ended',
  'stale_resume',
  'discarded',
]);

/** Identify ResponseAborted events that have been superseded by a later
 *  same-request_event_id terminal (ResponseGenerated / ResponseFailed). This
 *  models the engine-restart-then-recovered turn: recovery emits an abort,
 *  the rerun re-uses the original request_event_id, and the eventual success
 *  or definitive failure should win the exchange's verdict.
 *
 *  Strict matching: only events with the SAME non-null request_event_id are
 *  paired. Two different ids in the same exchange (or one event missing the
 *  field) do NOT merge — preserving the no-recovery case unchanged. */
function supersededAbortIndices(steps: SequencedEvent[]): Set<number> {
  const superseded = new Set<number>();
  for (let i = 0; i < steps.length; i++) {
    const aborted = steps[i].event;
    if (aborted.type !== 'ResponseAborted') continue;
    const abortReqId = aborted.request_event_id;
    if (!abortReqId) continue;
    for (let j = i + 1; j < steps.length; j++) {
      const later = steps[j].event;
      if (later.type !== 'ResponseGenerated' && later.type !== 'ResponseFailed') continue;
      if (later.request_event_id === abortReqId) {
        superseded.add(i);
        break;
      }
    }
  }
  return superseded;
}

/** A pending (optimistic, not-yet-ingested) chat follow-up exchange. These are
 *  synthesized by `computeExchanges` from `thread.pendingUserMessages` with a
 *  `_displayCreated` stamp and NO `created` (only the agent's real persisted
 *  events carry `created`), and sorted to the very end of the timeline. CC
 *  follow-ups instead carry a real `created` (delivered to stdin immediately,
 *  never queued), so this predicate is chat-only by construction. */
export function isPendingFollowup(exchange: Exchange): boolean {
  const ev = exchange.userEvent;
  return ev.type === 'MessageReceived' && !ev.created && !!ev._displayCreated;
}

/** A chat user message that has been accepted by the UI/server but not yet
 *  ingested by the agentic loop. Optimistic messages have `_displayCreated`
 *  and no `created`; persisted queued messages have `created`. Both stay
 *  stepless until `UserPromptInjected` lands and is absorbed into the
 *  exchange. */
function isUningestedChatMessage(exchange: Exchange): boolean {
  return exchange.userEvent.type === 'MessageReceived' && exchange.steps.length === 0;
}

function exchangeHasTerminalStep(exchange: Exchange): boolean {
  return exchange.steps.some(({ event }) =>
    event.type === 'ResponseGenerated'
    || event.type === 'ResponseCanceled'
    || event.type === 'ResponseAborted'
    || event.type === 'ResponseFailed'
    || event.type === 'CodingAgentIdled'
    || event.type === 'SessionEnded'
  );
}

/** Whether a non-queued exchange is a live/parked turn that can have chat
 *  follow-ups queued behind it. Terminal lifecycle panels (change applied,
 *  cancel/abort boundaries, etc.) deliberately return false so a new message
 *  sent after idle becomes the active turn rather than a queued bubble. */
function canQueueBehind(exchange: Exchange): boolean {
  if (exchangeHasTerminalStep(exchange)) return false;
  switch (exchange.userEvent.type) {
    case 'MessageReceived':
    case 'TriggerStarted':
    case 'ContinuationStarted':
    case 'UserQuestionAsked':
    case 'CodingAgentPermissionRequest':
    case 'CommandPermissionRequested':
    case 'McpPermissionRequested':
    case 'CredentialRequested':
    case 'McpConsentRequested':
    case 'ChildThreadCompleted':
    case 'MissingHardeningDetected':
    case 'MergeConflictDetected':
      return true;
    default:
      return false;
  }
}

export interface QueuedFollowupRun {
  activeIndex: number;
  queuedOrder: number[];
  queuedIndices: Set<number>;
}

/** Locate the exchange that owns the active response plus queued follow-ups.
 *  A question/permission divider can arrive after a queued MessageReceived but
 *  before injection, so queued indices are tracked independently instead of
 *  assuming one contiguous trailing run. CC/Codex are excluded because
 *  follow-ups go straight to the subprocess stdin; only regular chat uses the
 *  agentic-loop queue. */
export function queuedFollowupRun(
  exchanges: Exchange[],
  threadBusy: boolean,
  threadIsCC = false,
): QueuedFollowupRun {
  const empty: QueuedFollowupRun = {
    activeIndex: exchanges.length - 1,
    queuedOrder: [],
    queuedIndices: new Set(),
  };
  if (!threadBusy || threadIsCC || exchanges.length === 0) return empty;

  const candidates: number[] = [];
  for (let i = 0; i < exchanges.length; i++) {
    if (isUningestedChatMessage(exchanges[i])) candidates.push(i);
  }
  if (candidates.length === 0) return empty;

  let activeIndex = -1;
  for (let i = exchanges.length - 1; i >= 0; i--) {
    if (isUningestedChatMessage(exchanges[i])) continue;
    // Only the MOST RECENT settled/in-flight turn can own queued follow-ups,
    // so stop at the first non-uningested exchange. If it can be queued behind
    // (still streaming, or parked on a question) the follow-up queues behind
    // it; if it's terminal the thread idled and the just-sent message IS the
    // active turn. Walking PAST a terminal turn into older non-terminal ones —
    // e.g. an answered `UserQuestionAsked` whose continuation flowed into the
    // next question without ever producing a terminal step — wrongly rendered
    // the fresh follow-up as "queued" up in history (real thread aa75ff37: the
    // optimistic bubble blinked away from the bottom and reappeared in a Queued
    // group above).
    if (canQueueBehind(exchanges[i])) activeIndex = i;
    break;
  }
  if (activeIndex === -1) activeIndex = candidates[0];

  const queuedOrder = candidates.filter(i => i !== activeIndex);
  if (queuedOrder.length === 0) {
    return { ...empty, activeIndex };
  }

  const queuedIndices = new Set<number>(queuedOrder);

  return {
    activeIndex,
    queuedOrder,
    queuedIndices,
  };
}

/** A queued (uningested) chat follow-up: its retract id + message text. */
export interface QueuedMessage {
  /** The client `event_id` (== events-table PK) — the `message_id` the
   *  `/chat/queued-message/remove` endpoint retracts by. */
  id: string;
  text: string;
}

/** The thread's queued (un-injected) chat follow-ups, in FIFO order — the set
 *  a user Stop should retract and return to compose (see
 *  `store/actions/chat.ts::cancelCurrentExchange`). Derived from the same
 *  `queuedFollowupRun` the UI renders "Queued" bubbles from, so what Stop
 *  clears is exactly what the user saw queued. An exchange with no `_eventId`
 *  (legacy row / synthetic boundary) can't be retracted by id and is skipped. */
export function queuedMessagesFromExchanges(
  exchanges: Exchange[],
  threadBusy: boolean,
  threadIsCC = false,
): QueuedMessage[] {
  const { queuedOrder } = queuedFollowupRun(exchanges, threadBusy, threadIsCC);
  const out: QueuedMessage[] = [];
  for (const idx of queuedOrder) {
    const id = exchanges[idx].userEvent._eventId;
    if (!id) continue;
    out.push({ id, text: exchangeUserMessage(exchanges[idx]) });
  }
  return out;
}

/** Index of the exchange the agent is actively working on — i.e. the one that
 *  owns the live stream and should read 'streaming'/'working', not the literal
 *  last exchange.
 *
 *  When the thread is busy, follow-ups typed while it worked are queued. The
 *  active exchange is the live/parked non-queued turn when one exists;
 *  otherwise it is the first stepless user message. When the thread is idle,
 *  the literal last exchange is active (a freshly-sent message is about to be
 *  picked up — it must read 'Requesting', not 'Queued'). */
export function activeExchangeIndex(exchanges: Exchange[], threadBusy: boolean): number {
  return queuedFollowupRun(exchanges, threadBusy).activeIndex;
}

/** Derive ExchangeStatus for an exchange.
 *  @param isLast — true if this is the last (newest) exchange in the thread
 *  @param hasPriorActive — true if a prior exchange is still active (pending/streaming/coding-agent-working),
 *         meaning this exchange is queued behind it
 *  @param threadIdle — true when CC is not producing output (see
 *         `isThreadQuiescent` in store.ts). When true and the exchange has no
 *         terminal event, the exchange was interrupted by an engine crash/lid
 *         close and should show as 'aborted', not 'streaming'.
 *  @param threadAwaitingAnswer — true when the backend status is
 *         `waiting_for_user_answer` (the thread is parked on, or resuming from,
 *         a question / permission card). Such a thread is NEVER crashed, so the
 *         stale-`'aborted'` detector below must not fire for it: a just-answered
 *         question-divider whose resume `running` aggregate hasn't reached the
 *         client yet would otherwise flash "Aborted" during the answer→resume
 *         gap (the running status is set by `UserQuestionAnswered` /
 *         `CodingAgentPermissionResolved` / `CommandPermissionResolved`, but the
 *         client `meta.status` can briefly lag at `waiting_for_user_answer`).
 *         A genuine crash settles to `idle`/`failed` — never
 *         `waiting_for_user_answer` — so this never masks a real abort. */
export function exchangeStatus(exchange: Exchange, streamingBuffer: string, isLast: boolean, hasPriorActive?: boolean, threadIsCC?: boolean, threadIdle = false, threadAwaitingAnswer = false): ExchangeStatus {
  let isComplete = false;
  let isCanceled = false;
  let isAborted = false;
  let isFailed = false;
  let isCC = false;
  let isCCWaiting = false;
  let isSessionEnded = false;
  // SessionEnded with a normal lifecycle reason (changes_proposed, completed, etc.)
  // — terminal for CC exchanges even when CodingAgentIdled was skipped (e.g. the
  // engine's auto-harden `continue` path bailed out before emitting it).
  let isSessionEndedNormally = false;
  let isShutdown = false;
  // CC paused on AskUserQuestion. The QuestionCard owns the action surface;
  // the exchange itself reads as "done" so it doesn't show a "Working" spinner
  // while the user thinks. Resume (UserQuestionAnswered followed by CC text)
  // clears this flag and the exchange falls back to coding-agent-working.
  let isWaitingForAnswer = false;
  // The turn registered an *event wait* and has not been woken out of it
  // (ADR 0047). `await_event` is terminal and the engine deliberately emits no
  // terminator for the park, because the dangling `ToolCalled{await_event}` IS
  // the slot the delivered event lands in. So the generic "steps but nothing
  // ended it" fallthrough read a parked turn as 'streaming' and the panel said
  // "Working" for however long the thread slept. It is not working: it did its
  // work and parked, and the live state belongs to the subscription indicator.
  let isParkedOnEventWait = false;
  // Track whether the exchange reached a "completed" state BEFORE any
  // abort/shutdown event. When true, the abort is from a system-injected
  // prompt crash (e.g., auto-harden) and the user's work was already done.
  // This distinguishes "CC completed → auto-harden crashed → ResponseAborted"
  // (should be 'done') from "CC crashed mid-work → ResponseAborted" (should
  // be 'aborted').
  let wasCompleted = false;
  let completedBeforeAbort = false;

  const supersededAborts = supersededAbortIndices(exchange.steps);

  // Divider exchanges (UserQuestionAsked / CodingAgentPermissionRequest as
  // userEvent) start in awaiting-answer until a matching resolution lands as
  // a step. Without seeding here, the steps loop sees only the resolution and
  // never the request, so isWaitingForAnswer stays false for pending dividers.
  const userEventType = exchange.userEvent.type;
  if (
    userEventType === 'UserQuestionAsked'
    || userEventType === 'CodingAgentPermissionRequest'
    || userEventType === 'CommandPermissionRequested'
    || userEventType === 'McpPermissionRequested'
  ) {
    isWaitingForAnswer = true;
  }

  for (let i = 0; i < exchange.steps.length; i++) {
    const event = exchange.steps[i].event;
    switch (event.type) {
      case 'ResponseGenerated': isComplete = true; wasCompleted = true; break;
      case 'ResponseCanceled':
        // A Codex mid-turn follow-up redirect is a cancel mechanically but the
        // user steered, they didn't Stop — render it neutrally (terminal "Done",
        // no red ✕), exactly like the chat/CC follow-up. Only a real Stop /
        // user-action cancel sets isCanceled (→ "Canceled ✕").
        if (event.cause === 'superseded_by_followup') { isComplete = true; break; }
        isCanceled = true; isComplete = true; break;
      case 'ResponseAborted':
        if (supersededAborts.has(i)) break; // superseded by a later same-id terminal
        if (wasCompleted) completedBeforeAbort = true;
        isAborted = true; isComplete = true; break;
      case 'ResponseFailed': isFailed = true; isComplete = true; break;
      case 'SessionStarted':
        isCC = true; isSessionEnded = false; isSessionEndedNormally = false; isShutdown = false;
        break;
      // SessionEnded: deliberate lifecycle endings must NOT flash the
      // "engine restarted" aborted banner, even if isCCWaiting was
      // transiently cleared by a CodingAgentPromptSent (e.g., hardening
      // follow-ups during apply_now). Only `shutdown`/`panic` are system
      // interruptions; everything else (including missing reason from
      // legacy DB rows, coalesced to `completed`) is a normal lifecycle end.
      case 'SessionEnded': {
        const reason = event.reason ?? 'completed';
        if (reason === 'shutdown') {
          if (wasCompleted) completedBeforeAbort = true;
          isShutdown = true;
        }
        if (!NORMAL_SESSION_END_REASONS.has(reason)) {
          isSessionEnded = true;
        } else if (reason !== 'stale_resume') {
          // stale_resume is mid-flight (a fresh SessionStarted follows) — not terminal.
          isSessionEndedNormally = true;
        }
        break;
      }
      case 'CodingAgentIdled': isCCWaiting = true; wasCompleted = true; break;
      // CC work events after waiting → CC resumed, no longer waiting/complete.
      // CodingAgentUserMessageSent resets wasCompleted — a user follow-up in the
      // same exchange (legacy data) means new work was requested.
      case 'CodingAgentUserMessageSent':
        isCCWaiting = false; isComplete = false; wasCompleted = false; break;
      case 'CodingAgentToolCalled':
      case 'CodingAgentTextStreamed':
      case 'CodingAgentPromptSent':
        isCCWaiting = false; isComplete = false; isWaitingForAnswer = false; break;
      case 'UserQuestionAsked':
      case 'CodingAgentPermissionRequest':
      case 'CommandPermissionRequested':
      case 'McpPermissionRequested':
        isWaitingForAnswer = true; break;
      case 'UserQuestionAnswered':
      case 'CodingAgentPermissionResolved':
      case 'CommandPermissionResolved':
      case 'McpPermissionResolved':
        isWaitingForAnswer = false; break;
      case 'EventWaitStarted': isParkedOnEventWait = true; break;
      // A delivery and an expiry both hand the parked model a `ToolResult` and
      // re-enter the turn, so the exchange is live again and falls back to the
      // ordinary machinery. A CANCEL does not: it closes the dangling call so
      // the next turn is sendable at all, and the thread settles (the engine's
      // `status_transitions` maps `EventWaitCanceled` to Idle). Leaving the
      // flag set there is what keeps a stopped wait reading "Done" instead of
      // falling through to the stale detector's "Aborted".
      case 'EventWaitDelivered':
      case 'EventWaitExpired':
        isParkedOnEventWait = false; break;
    }
  }

  // Follow-up exchanges in a CC thread inherit CC context even without
  // their own SessionStarted event (the session is shared across exchanges).
  if (threadIsCC) isCC = true;

  const hasSteps = exchange.steps.length > 0;
  // Absorbed-UPI placeholder: the engine emitted a UPI carrying this
  // exchange's MR via injected_message_id, so the response actually lives in
  // the prior exchange (req_id-routed there). The placeholder reads as 'done'
  // and is excluded from the 'interrupted' carve-out below (the "↳" continues-
  // below arrow is wrong — the answer is above, not below).
  //
  // "Lives in the prior exchange" is the whole claim, and there is one shape
  // where it is false: the UPI's own `request_event_id` naming THIS exchange's
  // MessageReceived. Then the turn is anchored right here and its steps are on
  // their way in. That is the orphan re-entry (`announce_orphan_batch` in the
  // engine's `api/chat.rs`), where a follow-up queued behind a cancelled turn
  // is re-submitted as a turn of its own; read as a placeholder it rendered
  // "Done ✓" for the whole gap before the first step arrived, on the one panel
  // that WAS being worked on.
  //
  // Both ids must be PRESENT to draw that conclusion. A legacy row carries
  // neither, and two `undefined`s comparing equal would revoke the placeholder
  // from exactly the case it was written for, dropping it through to the stale
  // detector and a misleading "Aborted ⚠".
  const onlyStep = exchange.steps.length === 1 ? exchange.steps[0].event : undefined;
  const upiRequestId = (onlyStep as { request_event_id?: string } | undefined)?.request_event_id;
  const anchorId = exchange.userEvent._eventId;
  const announcesThisExchange = !!upiRequestId && !!anchorId && upiRequestId === anchorId;
  const isAbsorbedUpiPlaceholder = onlyStep?.type === 'UserPromptInjected'
    && !!onlyStep.injected_message_id
    && !announcesThisExchange;

  // A switch-teardown boundary is CLOSED BY CONSTRUCTION, whatever the thread
  // projection currently says. The engine went down with it, and its resume
  // opens an exchange of its own (`ContinuationStarted` is an exchange starter),
  // so nothing that lands here is new work. What does land is the dying
  // subprocess's drain: the teardown Esc produces a `CodingAgentToolResult`
  // rejection and a last `"\n\n"` flush, ~40ms after the abort, and CC events
  // fold chronologically rather than by request id, so they land in this
  // boundary as steps.
  //
  // Without this the boundary had steps and no terminal, and the stale detector
  // below could not reach it: that detector is gated on `threadIdle`, and a
  // switch teardown settles the thread at `paused` (or, when a change was
  // already proposed, at `waiting`, which the drain then revives to `running`).
  // Neither is quiescent, so the last branch of this function won and the panel
  // shimmered "Working" for the whole teardown-plus-restart window, on a thread
  // whose engine was not running. Reported 2026-08-06; see
  // `docs/plans/2026-08-06-no-working-label-while-nothing-is-running.md`.
  //
  // Keyed on the switch fingerprint rather than on "is an abort boundary",
  // because a boundary CAN acquire a live turn: a `safety_net` abort fires on a
  // turn the watchdog thought was stuck, the loop keeps going, and two minutes
  // of real work lands under it (real thread ebc787a4, the case commit
  // 3da5620eb exists for). Only the switch teardown takes the engine with it.
  const isSwitchTeardownBoundary = abortPromisesAutoResume(exchange.userEvent);

  // Stale exchange: the thread's projection says quiescent, but this exchange
  // has steps and no terminal event. The agentic loop (or the coding-agent
  // subprocess) died without emitting ResponseGenerated / ResponseAborted /
  // CodingAgentIdled: an engine crash, a lid close, or a teardown that skipped
  // its terminal. Nothing is running, so the panel must read "Aborted" rather
  // than spin forever. `hasSteps` covers tool calls AND streamed text (both are
  // in `exchange.steps`).
  //
  // EXCEPT when the thread is `waiting_for_user_answer` (threadAwaitingAnswer):
  // a thread parked on / resuming from a question or permission card is never
  // crashed. A just-answered question-divider (UserQuestionAnswered step, no
  // terminal yet) whose resume `running` aggregate hasn't reached the client
  // would otherwise flash "Aborted" during the answer→resume gap. The agent's
  // continuation (and its terminal) is in flight; render it as working, not
  // crashed. A genuine crash settles to `idle`/`failed`, never
  // `waiting_for_user_answer`, so this can't mask a real abort.
  //
  // The absorbed-UPI placeholder is excluded because its lone UPI step means
  // the real response lives in the PRIOR exchange: it is 'done', not crashed.
  //
  // A switch-teardown boundary supplies the quiescence itself (see above): the
  // engine is down, so it does not have to wait for the projection to say so.
  // It is deliberately NOT exempt from `threadAwaitingAnswer`, which stands for
  // a live agent mid-answer, a state a torn-down engine cannot be in.
  const isStale =
    (threadIdle || isSwitchTeardownBoundary) && !threadAwaitingAnswer
    && isLast && !isComplete && hasSteps
    && !isAbsorbedUpiPlaceholder;

  if (isFailed) return 'error';
  // Abort/shutdown AFTER the exchange was already completed (e.g., auto-harden
  // crash after CodingAgentIdled/ResponseGenerated) — the user's work was done.
  // System-level crashes after completion don't undo that.
  if ((isAborted || isShutdown) && completedBeforeAbort) return 'done';
  // ResponseAborted event — system-initiated interruption (crash, shutdown, etc.)
  if (isAborted) return 'aborted';
  // Engine shutdown — system-initiated interruption, not user cancel.
  if (isShutdown) return 'aborted';
  if (isCanceled) return 'canceled';
  // Session ended without a proper response = aborted.
  // Chat: no ResponseGenerated. CC: no CodingAgentIdled (was mid-work when killed).
  if (isSessionEnded && !isComplete && !isCCWaiting) return 'aborted';
  // If a prior exchange is still active and this exchange has no events yet,
  // it's queued (waiting for the prior to finish). Must check BEFORE the
  // !isLast→done fallthrough to avoid showing "No response generated".
  // CC threads don't queue — messages go to CC's stdin, not engine queue.
  // Only the LAST queued exchange shows "Queued" — earlier ones were superseded
  // by a newer message and are handled by the empty-non-last rule below
  // (→ 'done', renders as "Done ✓").
  // A user stop is a boundary that states its own outcome, and nothing continues
  // out of it: the wait is gone and the engine settles the thread rather than
  // resuming anything. Terminal by construction, so it never spins "Requesting"
  // while the thread is still `running` for an unrelated turn, and never falls
  // through to the stale detector's "Aborted" once it settles.
  //
  // Placed AFTER the terminal-verdict arms above rather than first, defensively.
  // Grouping deliberately does not make this boundary `current` (see
  // `isExchangeStartEvent`), so it should hold no steps at all; if a future
  // routing path ever lands a real `ResponseFailed` or abort in it, that
  // terminal reports itself rather than being swallowed by this line.
  if (isUserStoppedWait(exchange.userEvent)) return 'done';
  if (hasPriorActive && !hasSteps && !isCC && isLast) return 'queued';
  // CC idle → done. WaitingBanner handles the "can interact" state separately.
  if (isCCWaiting) return 'done';
  // Parked on an event wait: the turn ran to its end and registered a
  // subscription, which is a completed piece of work, not one in flight. Read
  // as 'done' for the same reason `isWaitingForAnswer` reads as its own state
  // rather than "Working": nothing is running, and the surface that owns the
  // live half (the subscription indicator, with its countdown and Stop) is
  // elsewhere. Placed before the stale detector so a settled thread whose wait
  // the user stopped doesn't read "Aborted", and before the `!isLast`
  // 'interrupted' arm so a detached wake landing in a later exchange doesn't
  // retroactively mark the park as abandoned.
  if (isParkedOnEventWait) return 'done';
  // Claude Code session ended with a normal reason (changes_proposed, completed, etc.) —
  // terminal even when CodingAgentIdled was missing.
  if (isCC && isSessionEndedNormally) return 'done';
  // CC paused on a user question or permission prompt — render as
  // 'awaiting-answer' so the surrounding spinner stops AND the header reads
  // "Needs your answer" (not the misleading "Done ✓"). The QuestionCard /
  // PermissionCard inside the exchange shows the action surface.
  //
  // `!exchange.questionOvertaken` is what keeps this honest. The switch above
  // clears `isWaitingForAnswer` on only three progression types, while
  // `QUESTION_OVERTAKEN_STEP_TYPES` (which decides whether the card renders
  // struck through and disabled) covers twelve. A shape in the gap read
  // "Needs your answer" over a card whose buttons were already dead: exactly
  // the lone `CodingAgentToolResult` a teardown-Esc'd `AskUserQuestion`
  // produces. Deferring to the overtaken flag means one list decides both, the
  // client mirror of the engine's single park-ending set. An overtaken divider
  // falls through to the stale detector below ('aborted' on a settled thread)
  // or to 'coding-agent-working' while the agent really is still going.
  if (isWaitingForAnswer && !exchange.questionOvertaken) return 'awaiting-answer';
  // Non-last with steps but no terminator: the user moved past this exchange
  // (chat fast-path injects the follow-up via UPI under the parent's
  // request_event_id and redirects later events to the new exchange; CC
  // shares one session across exchanges). Render as 'interrupted' so only
  // the last panel reads "Working".
  if (!isLast && !isComplete && hasSteps && !isAbsorbedUpiPlaceholder) return 'interrupted';
  if (isComplete) return 'done';
  // Non-last CC exchange without a terminator was skipped by CC's msg_tx queue
  // — safely 'done'.
  if (!isLast && (isCC || threadIdle)) return 'done';
  // Empty chat exchange when the engine has gone idle — extends the !isLast
  // empty-→-done rule to the isLast case, so an MR whose response landed in
  // a sibling exchange (off-by-one in the orphan re-process chain — see
  // thread 9b5a05aa) doesn't spin "Requesting" forever. Without `threadIdle`
  // the isLast branch must keep falling through so a freshly-sent MR before
  // the loop has emitted anything still reads as 'pending'/'Requesting'.
  // Relies on chat's request_event_id serialization invariant: by the time an
  // exchange is non-last, the loop has already moved past it (mid-flight
  // injection routes new events back to the parent's request_event_id, so a
  // non-last exchange the loop is still actively processing has steps).
  if (!hasSteps && !isCC && (!isLast || threadIdle)) return 'done';
  // A child-completion row is itself terminal: it renders the spawned
  // sub-thread's result (success/failure/canceled). When the parent is
  // QUIESCENT (idle, or parked on a question — both `threadIdle`) and never
  // resumed to react (its in-memory wake was lost to a restart, or — real
  // thread 276f5580 — its worktree was already gone), OR the card was
  // superseded by a newer boundary (`!isLast`), the stepless card is terminal:
  // return 'done' so it doesn't fall through to a phantom
  // 'pending'/'coding-agent-working' spinner forever.
  //
  // But while the parent is still RUNNING and this is the last exchange, it is
  // about to react to the completion — the WakeFromChild summary was injected
  // into the live loop and its continuation now request-id-routes INTO this
  // card (see the redirect-advance for ChildThreadCompleted in
  // exchange-grouping.ts). Fall through to the normal pending/working machinery
  // so the card shows a live "working" spinner during the gap before the first
  // post-completion step, instead of a misleading "Done ✓" (real thread
  // 4d193da8). Once steps arrive the spinner is driven by them as usual.
  if (userEventType === 'ChildThreadCompleted' && !hasSteps && (threadIdle || !isLast)) return 'done';

  // CC exchanges are 'coding-agent-working' once they have steps, 'pending' before.
  //
  // `!isStale` is what stops a DEAD coding-agent turn reading "Working"
  // forever. A coding agent used to escape the stale detector below entirely:
  // this branch returned first, unconditionally. So a CC turn whose terminator
  // never landed spun a live-looking spinner on a subprocess that no longer
  // exists. That is exactly what a teardown-Esc'd `AskUserQuestion` produced on
  // 2026-08-01 (see
  // `docs/plans/2026-08-01-preserve-question-parked-session-through-teardown.md`):
  // progression events after the question, no terminal, thread settled to
  // 'idle' by the boot reset, panel stuck on "Working". A live CC turn is
  // `running`, so `isStale` is false and this branch still wins.
  if (isCC && !isStale) return hasSteps ? 'coding-agent-working' : 'pending';
  // A live streaming buffer beats staleness for either agent: tokens are
  // arriving right now, whatever the projection last said.
  if (streamingBuffer) return 'streaming';

  // Absorbed-UPI placeholder: handled here for the isLast case (the
  // !isLast branch above bypasses 'interrupted' for it). It must not read as a
  // crash, which is why `isStale` excludes it outright rather than relying on
  // this line's position.
  if (isAbsorbedUpiPlaceholder) return 'done';

  if (isStale) return 'aborted';

  // Persisted response text (TextStreamed events) without a completion event
  // means the response is still in progress — the streaming buffer was just
  // cleared by a persisted event arrival. Show 'streaming', not 'done'.
  const responseText = exchangeResponseText(exchange);
  if (responseText) return 'streaming';

  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, isLast, threadIdle);
  if (steps.length > 0 || events.length > 0) return 'streaming';

  return 'pending';
}
