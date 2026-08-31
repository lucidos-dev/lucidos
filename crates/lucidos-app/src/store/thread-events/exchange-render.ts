import { SESSION_END_REASONS } from '../../generated/thread-lifecycle';
import { hasVisibleText, isMeaningfulText, mergeAdjacentTextEvents } from '../event-rendering';
import { AWAIT_EVENT_TOOL } from './event-waits';
import { describeCCTool, describeEngineTool, exchangeHasCCContent, exchangeResponseText, exchangeUserMessage, fullCommandForCCTool, fullCommandForEngineTool } from './exchange';
import { isSpokenTurn, toolUseIdOf } from './exchange-grouping';
import { IDLE_ENGINE_RESTART_INTERRUPT_REASON, isEngineDownAbort, isSwitchTeardownAbort, isUserStoppedWait } from './thread-event-types';
import type { ExchangeStatus } from '../exchange-status';
import type { ContextAssembledData, ContextCapture, ContextSection, ResponseEvent, Step, StepOutcome } from '../types';
import type { Exchange } from './exchange';
import type { ActorMode, EventSubscription, EventWaitCancelCause, SequencedEvent, ThreadEvent } from './thread-event-types';

/** The two projections' step shapes, as far as the resolvers care. */
type StepLike = { outcome: StepOutcome; description?: string; tool_name?: string };

/** How an exchange ended, as far as its pending steps are concerned.
 *  `null` = no terminator yet, or the agent resumed past one.
 *  `'clean'` = the turn finished, so a step still pending did run to
 *  completion and merely lacks a recorded result.
 *  `'unclean'` = the turn died, so a step still pending never finished and
 *  must not be resolved to a success.
 *  The LAST terminator in the exchange wins, which is what makes a superseded
 *  abort (an abort followed by a same-request `ResponseGenerated`) come out
 *  right. */
type TerminalKind = null | 'clean' | 'unclean';

function pendingOutcomeFor(terminal: TerminalKind): StepOutcome {
  return terminal === 'unclean' ? 'unfinished' : 'success';
}

/** Resolve the last pending step. Backwards, so parallel tool calls resolve in
 *  LIFO order as their results arrive. */
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
 *  exchanges. A clean end resolves them to a success. A turn that died marks
 *  them `'unfinished'`, because a green check on a tool killed mid-execution
 *  is a worse lie than the spinner.
 *  `pred` narrows the set, to finalize ONLY the dead 'Thinking' markers of a
 *  handed-off exchange while its tool steps keep spinning. */
function resolvePendingSteps(
  steps: StepLike[],
  outcome: StepOutcome,
  pred?: (s: StepLike) => boolean,
): void {
  for (const step of steps) {
    if (step.outcome === 'pending' && (!pred || pred(step))) step.outcome = outcome;
  }
}

/** A row that has not reached a verdict: running, or held on a permission card.
 *  Both projections ask through this, the flat one over its own array and the
 *  other through `lastStepIndex`.
 *
 *  Deliberately NOT used by the sweeps, which key on `'pending'` alone, so a
 *  held row is exempt from them by construction. */
const isLiveStep = (s: StepLike) => s.outcome === 'pending' || s.outcome === 'blocked';

/** Settle the step a `tool_use_id`-matched result belongs to, which is most of
 *  them: this runs for every coding-agent call, gated or not.
 *
 *  It ticks off a running call, and a held one whose resolution never folded.
 *  It never touches a DENIED one. The result there is the refusal handed back
 *  to the agent, and a green check over it is the lie this state exists to
 *  end. */
function settleMatchedCallStep(step: StepLike): void {
  if (step.outcome === 'pending' || step.outcome === 'blocked') step.outcome = 'success';
}

/** Does a LIVE coding-agent turn owe the reader a `Thinking` row right now?
 *
 *  A tool result landed, no terminator has arrived, and nothing is running, so
 *  the model holds control. That IS a `Thinking` row. The next
 *  `CodingAgentToolCalled` consumes it by taking the index it occupied, since
 *  step rows are keyed by index in `ChatExchange`. The row is DERIVED at the
 *  end of each projection, never pushed from an event arm: ADR 0066.
 *
 *  Three conditions are the branch this is called from, in both projections,
 *  so they are not re-tested here. Of the four tested here, `anyLive` is
 *  self-evident and the other three are:
 *
 *  - A coding-agent turn, because only one of those has the gap. Wider than
 *    `hasCCContent` by `SessionStarted`, which covers the resumed-session
 *    start window and is the same event `exchangeStatus` reads as "Working".
 *  - `isLast`, the ACTIVE exchange. Coding-agent events fold chronologically
 *    rather than by request id, so the live turn IS the last one.
 *  - Not an *engine-down* boundary (`abortTookEngineDown`), which is closed by
 *    construction. A row there RESURRECTS the response panel
 *    `hasRenderableResponseContent` suppressed, with a "Working" badge over
 *    the dying subprocess's drain. */
function needsLiveThinkingRow(opts: {
  exchange: Exchange;
  isLast: boolean;
  hasCCContent: boolean;
  /** A row already speaks for the turn. `'blocked'` counts, and it is the one
   *  that is not running: a call held on a permission card IS the turn's
   *  current row. A Thinking row beside it would shimmer over a turn that is
   *  waiting for the reader. */
  anyLive: boolean;
}): boolean {
  const { exchange, isLast, hasCCContent, anyLive } = opts;
  if (!isLast || anyLive) return false;
  if (abortTookEngineDown(exchange.userEvent)) return false;
  // The `SessionStarted` scan runs only in the start window, where
  // `hasCCContent` is false and everything cheaper has already passed.
  return hasCCContent || exchange.steps.some(({ event }) => event.type === 'SessionStarted');
}

/** A step row that has not named itself yet: the model is thinking and has not
 *  said what it is about to do. Exported because `InlineStep` shows the
 *  reasoning ticker only while the row is unnamed. Two definitions of "is this
 *  row still a Thinking marker" would drift. */
export const isThinking = (s: StepLike) => s.description === 'Thinking';
const isNotThinking = (s: StepLike) => !isThinking(s);

/** Step label for a `MemoryRecalled` (or a legacy `MemorySearched` row).
 *
 *  Deliberately verb-first and free of the word "search". The `memory` tool's
 *  own step reads "Searching memory for ...". Sharing that word makes the
 *  engine's automatic pre-turn recall and the agent's deliberate mid-turn
 *  lookup read as the same thing happening twice.
 *
 *  Mirrors `memory_recalled_label` in `crates/lucidos-engine/src/core/store/
 *  mod.rs`, which builds the same strings for session replay. Change one,
 *  change both. */
export function memoryRecalledLabel(results: number): string {
  if (results <= 0) return 'No memories recalled';
  if (results === 1) return 'Recalled 1 memory';
  return `Recalled ${results} memories`;
}
/** The park's own step, i.e. the one the event-wait row replaces. See the
 *  `EventWaitStarted` arm of `exchangeResponseEvents`. */
const isAwaitEventStep = (s: StepLike) => s.tool_name === AWAIT_EVENT_TOOL;

/** Name the `Thinking` row after the action the model just produced. One LLM
 *  call is then ONE row, not a resolved "Thinking ✓" beside the thing that
 *  call decided to do. Same replace-in-place shape as the `EventWaitStarted`
 *  arm below: two rows for one action reads as two actions.
 *
 *  The row keeps what it earned before it could name itself: the context
 *  snapshot, the streamed reasoning, the legacy counters. It ends `pending`,
 *  because the tool it named is now what is running. The snapshot survives by
 *  ordering: the engine emits `ThoughtStreamed`, `ContextCaptured`, then
 *  `ToolCalled` within one iteration of the agentic loop.
 *
 *  **A pass that says something first still owns its row.** Its first visible
 *  text resolves the marker, leaving the arriving tool call nothing PENDING to
 *  name, so the fallback claims that RESOLVED marker instead.
 *
 *  Only the FIRST action of a pass takes the row: naming it stops it matching
 *  `isThinking`. Parallel calls behind it must push rows of their own, since a
 *  result pairs back by `tool_use_id`. False means no claimable row, so the
 *  caller pushes a fresh one. An `unfinished` marker is not claimable: that
 *  turn died, so a later call is not the pass's own. */
function nameThinkingRow<T extends StepLike>(rows: T[], naming: Partial<T>): boolean {
  for (let i = rows.length - 1; i >= 0; i--) {
    if (rows[i].outcome === 'pending' && isThinking(rows[i])) {
      Object.assign(rows[i], naming);
      return true;
    }
  }
  const last = rows[rows.length - 1];
  if (last && last.outcome === 'success' && isThinking(last)) {
    Object.assign(last, naming);
    // Reopened into the arriving call's OWN state, which the caller supplies:
    // `'pending'` normally, `'blocked'` or `'denied'` under a permission card.
    last.outcome = naming.outcome ?? 'pending';
    return true;
  }
  return false;
}

/** What a tool call's row starts in. `'pending'` is the ordinary answer. The
 *  two others come from a permission card gating this exact call step, which
 *  the grouping fold records (see `Exchange.blockedStepSeqs`).
 *
 *  Both projections read it, so a held call cannot say one thing in the summary
 *  row and another inline. */
function callOutcome(exchange: Exchange, seq: number): StepOutcome {
  if (exchange.deniedStepSeqs?.has(seq)) return 'denied';
  if (exchange.blockedStepSeqs?.has(seq)) return 'blocked';
  return 'pending';
}

/** `nameThinkingRow` for the ResponseEvent projection, whose array also carries
 *  text / image / event-wait rows.
 *
 *  One thing is its own, and it is why the two are not a single generic walk:
 *  claiming a RESOLVED marker also has to MOVE it. The prose that resolved it
 *  was pushed after it, so renaming in place would put "Running: cd …" above
 *  the sentence introducing that command. The row goes to the end, where the
 *  call happened, and the text events it vacated are then free to merge into
 *  one document (`mergeAdjacentTextEvents`). The flat `Step[]` projection
 *  carries no text, so its marker is already last. */
function nameThinkingStep(
  events: ResponseEvent[],
  naming: Partial<Extract<ResponseEvent, { type: 'step' }>>,
): boolean {
  const pending = lastPendingStepIndex(events, isThinking);
  if (pending >= 0) {
    Object.assign(events[pending], naming);
    return true;
  }
  const idx = lastStepIndex(events);
  const marker = idx >= 0 ? events[idx] : undefined;
  if (!marker || marker.type !== 'step' || marker.outcome !== 'success' || !isThinking(marker)) {
    return false;
  }
  Object.assign(marker, naming);
  // Same as `nameThinkingRow`: the caller owns the reopened state.
  marker.outcome = naming.outcome ?? 'pending';
  events.splice(idx, 1);
  events.push(marker);
  return true;
}

/** Bag of legacy events. All optional: `synthesizeContextCapture` produces
 *  something useful from any subset. */
export interface LegacyContextEvents {
  thinking?: { text?: string; context_tokens?: number; context_messages?: number; trimmed?: boolean };
  tokensMeasured?: { input_tokens?: number };
  /** `total_chars` is deliberately absent: it is a CHARACTER count, and its
   *  only use was standing in for a token total, a different unit. The
   *  per-section budget deltas carry the same information honestly. */
  assembled?: { sections?: ContextSection[]; tools?: string[]; model?: string };
}

/** Default context_window for legacy rows. Pre-`ContextCaptured` events never
 *  persisted the budget, and under-reporting on the 1M-context Opus fork is
 *  preferable to faking headroom. */
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
    // NOT `?? legacy.assembled?.total_chars`, which puts a CHARACTER count in
    // a token field at roughly 2.5x the truth. `frontend.md`'s "No Silent
    // Defaults": fall back to unknown, not to a plausible value.
    // `ContextCapturePanel` renders a zero headline as no token figure while
    // still showing every section's real char count.
    estimated_total_tokens: legacy.thinking?.context_tokens ?? 0,
    usage,
    trimmed: legacy.thinking?.trimmed ?? false,
    legacy: true,
  };
}

/** Convert a `ContextCaptured` ThreadEvent into the store-side
 *  `ContextCapture` shape.
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

/** Pick which step a ContextCaptured snapshot binds to. A main-LLM emit fires
 *  after a `Thinking` step and binds there, so the inline
 *  `tokens / window (pct%)` chip renders next to the request. A coding agent
 *  manages its own loop and has no per-API-call Thinking step. Its snapshot
 *  binds to whichever step is on top of the stack.
 *
 *  Shared by both projections so the inline chip and the summary agree on
 *  which step owns each snapshot. The caller supplies `assign` because Step
 *  and the ResponseEvent step variant share the `contextCapture` field but
 *  live in different unions. */
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
 *  @param isLast the ACTIVE exchange, the one at the bottom of the transcript
 *  (`queuedFollowupRun.activeIndex`). It does NOT resolve spinners, because a
 *  non-last exchange can still be the one the agentic loop is processing
 *  (chat mid-flight injection). It gates only the derived live row: see
 *  `needsLiveThinkingRow`.
 *  @param threadIdle true when the coding agent is not producing output (see
 *  `isThreadQuiescent` in store.ts). Combined with the in-exchange completion
 *  flag to finalize pending steps. */
export function exchangeSteps(exchange: Exchange, isLast = true, threadIdle = false): Step[] {
  const steps: Step[] = [];
  let terminal: TerminalKind = null;
  let legacyAcc: LegacyContextEvents = {};
  let lastThinkingIdx = -1;
  const refreshLegacySnapshot = () => {
    if (lastThinkingIdx < 0) return;
    steps[lastThinkingIdx].contextCapture = synthesizeContextCapture(legacyAcc);
  };
  for (const { seq, event } of exchange.steps) {
    switch (event.type) {
      // 'MemorySearched' is the retired name. Historical rows still carry it,
      // because the snapshot endpoint serves the raw `event_type` column.
      case 'MemoryRecalled':
      case 'MemorySearched': {
        const results = (event as { results?: number }).results ?? 0;
        steps.push({ description: memoryRecalledLabel(results), outcome: 'success' });
        break;
      }
      // The agent's own document under the self-curated context mode. It never
      // reaches the chat as prose, so a row of its own is the only sign the
      // agent wrote one. The body rides on the `responseEvents` step, which is
      // the path with a `detail` to fold it into.
      case 'WorkingUnderstandingWritten': {
        steps.push({
          description: 'Updated its working understanding',
          outcome: 'success',
        });
        break;
      }
      case 'ThoughtStreamed': {
        // ThoughtStreamed marks "about to invoke the LLM with this context".
        // Stay pending (spinner) until the LLM produces output, the next
        // ThoughtStreamed supersedes us, or the thread goes idle. Back-to-back
        // ThoughtStreamed means the previous call finished without visible
        // output, so resolve it.
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
        // VISIBLE text is output, so the marker stops shimmering. A blank
        // chunk puts nothing on screen, so a checkmark there would report a
        // pass that finished nothing. Either way the pass keeps its row, which
        // a tool call arriving next still claims (`nameThinkingRow`).
        if (hasVisibleText((event as { text?: string }).text)) {
          resolveLastPendingStep(steps, isThinking);
        }
        break;
      case 'ToolCalled': {
        // The call NAMES the row it came out of rather than checking that row
        // off and queueing beneath it. See `nameThinkingRow`.
        const e = event as { name: string; args: unknown; description?: string };
        const naming = {
          description: e.description || describeEngineTool(e.name, e.args),
          outcome: callOutcome(exchange, seq),
        };
        if (!nameThinkingRow(steps, naming)) steps.push({ ...naming });
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
          outcome: callOutcome(exchange, seq),
        };
        if (!nameThinkingRow(steps, naming)) steps.push({ ...naming });
        terminal = null; // CC resumed, not finished yet
        break;
      }
      case 'CodingAgentToolResult': {
        // Pair by tool_use_id, which is unique per call. A description is
        // ambiguous for parallel calls (two `Read SKILL.md` of different files
        // share a row label). The fallback handles AskUserQuestion: its
        // CodingAgentToolCalled is suppressed (run_session.rs) so no step
        // carries its id, and the result must not resolve the resume-marker
        // Thinking spinner agent_question.rs queued. Hence isNotThinking.
        const id = toolUseIdOf(event);
        let resolved = false;
        if (id) {
          for (const step of steps) {
            if (step.tool_use_id === id) {
              settleMatchedCallStep(step);
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) resolveLastPendingStep(steps, isNotThinking);
        break;
      }
      // The terminator's KIND decides what a still-pending step becomes. See
      // `TerminalKind`.
      case 'ResponseGenerated': case 'CodingAgentIdled':
        terminal = 'clean';
        break;
      case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
        terminal = 'unclean';
        break;
      case 'CodingAgentThoughtStreamed': {
        // Accumulate reasoning into the live "Thinking" step, opening one if
        // none is pending.
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
        // was written for: a coding agent emits a blank chunk before EVERY
        // tool call.
        if (hasVisibleText((event as { text?: string }).text)) {
          resolveLastPendingStep(steps, isThinking);
        }
        terminal = null; // CC resumed, not finished yet
        break;
    }
  }
  // See `exchangeResponseEvents` for the outcome split and for why a handed-off
  // exchange finalizes only its Thinking markers. The last branch is the live
  // turn, the only one that can owe a row: see `needsLiveThinkingRow`.
  if (terminal !== null || threadIdle) resolvePendingSteps(steps, pendingOutcomeFor(terminal));
  else if (exchange.continuationMoved) resolvePendingSteps(steps, 'success', isThinking);
  else if (needsLiveThinkingRow({
    exchange,
    isLast,
    hasCCContent: exchangeHasCCContent(exchange),
    anyLive: steps.some(isLiveStep),
  })) {
    steps.push({ description: 'Thinking', outcome: 'pending' });
  }
  return steps;
}

/** Index of the last step matching `pred`, or -1. Whatever its outcome, so a
 *  caller asking about a FINISHED row and one asking about a running row share
 *  a single walk. */
function lastStepIndex(events: ResponseEvent[], pred?: (s: StepLike) => boolean): number {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.type === 'step' && (!pred || pred(e))) return i;
  }
  return -1;
}

/** Index of the last pending step matching `pred`, or -1. The lookup half of
 *  `resolveLastPendingResponseStep`, for callers that replace a step rather
 *  than resolving it. With `pred` omitted it answers "is anything still
 *  RUNNING", which is narrower than `isLiveStep`: a call held on a permission
 *  card is live without running. */
function lastPendingStepIndex(events: ResponseEvent[], pred?: (s: StepLike) => boolean): number {
  return lastStepIndex(events, s => s.outcome === 'pending' && (!pred || pred(s)));
}

/** The denied step an arriving chat `ToolResult` belongs to, or null.
 *
 *  The chat lanes pair a result to the last PENDING step, and a denied one is
 *  no longer pending. So the refusal the guard hands back would land nowhere,
 *  and the step detail would show a refused command with no explanation. Only
 *  a step still missing its result can claim one, and the chat agentic loop is
 *  sequential, so at most one is ever waiting. */
function lastDeniedStepAwaitingResult(
  events: ResponseEvent[],
): Extract<ResponseEvent, { type: 'step' }> | null {
  const idx = lastStepIndex(events, s => s.outcome === 'denied');
  const step = idx >= 0 ? events[idx] : undefined;
  if (!step || step.type !== 'step' || step.result !== undefined) return null;
  return step;
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
 *  says? An agent that loses its upstream connection reports the error twice.
 *  It streams `API Error: …` as ordinary assistant text before exiting, and
 *  the engine records the same string as the turn's `ResponseFailed`.
 *
 *  The card is the copy that stays. It carries the `ResponseFailed`'s own
 *  event id, which is what resolves a notification deep-link to the failure
 *  (`ExchangeError.eventId`). `ChatExchange` renders it as a SIBLING of the
 *  response panel, so dropping the paragraph cannot hide it.
 *
 *  Matched per chunk, before `mergeAdjacentTextEvents`: an agent emits the
 *  error as its own chunk, and merging would glue it onto neighbouring prose.
 *  Exact trimmed equality only, so prose that mentions the failure stays.
 *
 *  A BACKSTOP rather than the primary defense: the engine drops an *agent
 *  error banner* at the parser (`claude_code_parse.rs`), so a Claude Code echo
 *  is never recorded. Older persisted events, the chat channel and Codex still
 *  route here. Do NOT widen past exact equality if a duplicate reappears glued
 *  to real prose: that shape means a new ingestion path is treating a harness
 *  message as model output, and the fix belongs there. */
function failureEchoPredicate(exchange: Exchange): (text: string | undefined) => boolean {
  const failure = exchangeError(exchange)?.message.trim();
  if (!failure) return () => false;
  return text => (text ?? '').trim() === failure;
}

/** Build ResponseEvent[] from exchange events (interleaved text + steps for rendering).
 *  @param isLast the ACTIVE exchange, as in `exchangeSteps`. It does not drive
 *  spinner resolution (see `threadIdle`), only the derived live row.
 *  @param threadIdle true when the coding agent is not producing output (see
 *  `isThreadQuiescent` in store.ts). Combined with the in-exchange completion
 *  flag to finalize pending steps. A non-last exchange can still be the one
 *  the engine is processing (chat mid-flight injection), so resolution must
 *  not trigger purely on `!isLast`. */
export function exchangeResponseEvents(exchange: Exchange, isLast = true, threadIdle = false): ResponseEvent[] {
  const events: ResponseEvent[] = [];
  const hasCCContent = exchangeHasCCContent(exchange);
  const isFailureEcho = failureEchoPredicate(exchange);
  let terminal: TerminalKind = null;
  // A text-less ResponseGenerated: the model ended its turn cleanly with no
  // text. Drives the neutral "empty response" note pushed after the loop.
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

  for (const { seq, event } of exchange.steps) {
    const created = event.created;
    switch (event.type) {
      // Legacy name kept alongside the current one, as in `exchangeSteps`.
      case 'MemoryRecalled':
      case 'MemorySearched': {
        const ms = event as { results?: number; queries?: string[] };
        const results = ms.results ?? 0;
        const detail = ms.queries?.length ? ms.queries.join(', ') : undefined;
        pushStep({ type: 'step', description: memoryRecalledLabel(results), outcome: 'success', detail, created });
        break;
      }
      // See the same case in `exchangeSteps`.
      case 'WorkingUnderstandingWritten': {
        const wu = event as { document?: string };
        pushStep({
          type: 'step',
          description: 'Updated its working understanding',
          outcome: 'success',
          detail: wu.document || undefined,
          created,
        });
        break;
      }
      case 'ThoughtStreamed': {
        // Stay pending until the next visible output supersedes us. See the
        // longer comment in `exchangeSteps`.
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
        // The call names the Thinking row it came out of instead of opening a
        // second row. See `nameThinkingRow`.
        const e = event as { name: string; args: unknown; description?: string };
        const naming = {
          description: e.description || describeEngineTool(e.name, e.args),
          tool_name: e.name,
          full: fullCommandForEngineTool(e.name, e.args),
          outcome: callOutcome(exchange, seq),
          created,
        };
        if (!nameThinkingStep(events, naming)) pushStep({ type: 'step', ...naming });
        break;
      }
      case 'ToolResult': {
        const toolResult = event as { name?: string; result?: string; images?: string[]; result_stripped?: boolean };
        // Skip pending Thinking, which ToolCalled already resolved: this pairs
        // with the matching tool step.
        //
        // `await_event` is narrowed to its OWN step. Its result fills the
        // rendezvous slot of a park whose step the event-wait row has already
        // replaced, so there is normally nothing left to resolve, and the
        // generic walk would tick off whatever call the re-entered turn has
        // since started. It still resolves the real thing on the
        // rejected-subscription path, where no row replaced the step.
        const resolved = resolveLastPendingResponseStep(
          events,
          toolResult.name === AWAIT_EVENT_TOOL ? isAwaitEventStep : isNotThinking,
        ) ?? lastDeniedStepAwaitingResult(events);
        if (resolved) {
          if (toolResult.result !== undefined) resolved.result = toolResult.result;
          // Always stamp the source event id so a re-fetch path can address
          // this step. Snapshot replays of stripped rows additionally stamp
          // `result_stripped`, which the step-detail modal gates the
          // lazy-fetch on (`ResultArea` in `StepDetailModal.tsx`). Live SSE
          // leaves it absent, and the modal renders the inline `result`.
          if (event._eventId) resolved.result_event_id = event._eventId;
          if (toolResult.result_stripped) resolved.result_stripped = true;
        }
        // Only `generate_image` puts bytes in a ToolResult (the
        // `[GENERATED_IMAGE:]` sentinel), so the resolved step's un-elided
        // primary arg IS the prompt. Carry it onto the image so it can
        // describe itself in a tooltip and in its alt text.
        if (toolResult.images?.length) {
          const prompt = resolved?.tool_name === 'generate_image' ? resolved.full : undefined;
          for (const b64 of toolResult.images) {
            events.push({ type: 'image', base64: b64, mime_type: 'image/jpeg', ...(prompt ? { prompt } : {}) });
          }
        }
        break;
      }
      case 'TextStreamed': {
        // Only VISIBLE text ends the thinking pass.
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
        const e = event as { name: string; args?: unknown; description?: string; args_stripped?: boolean };
        // Always stamp the source event id so the modal can address this call,
        // and stamp `args_stripped` only when the snapshot dropped the args.
        // Live SSE leaves the marker absent and `full` is computed inline, the
        // same split the `ToolResult` arm below draws.
        const naming = {
          description: e.description || describeCCTool(e.name, e.args),
          tool_name: e.name,
          tool_use_id: toolUseIdOf(event),
          full: fullCommandForCCTool(e.name, e.args),
          outcome: callOutcome(exchange, seq),
          created,
          ...(event._eventId ? { call_event_id: event._eventId } : {}),
          ...(e.args_stripped ? { args_stripped: true } : {}),
        };
        if (!nameThinkingStep(events, naming)) pushStep({ type: 'step', ...naming });
        terminal = null; // CC resumed, not finished yet
        break;
      }
      case 'CodingAgentToolResult': {
        // See exchangeSteps for the pairing rationale.
        const id = toolUseIdOf(event);
        const cc = event as { result?: string; result_stripped?: boolean };
        // Carry the result onto whichever step this settled, and the address of
        // the row it came from. A stripped snapshot row has no text, so the
        // marker is what tells the modal to fetch rather than render nothing.
        const land = (step: Extract<ResponseEvent, { type: 'step' }>) => {
          if (cc.result !== undefined) step.result = cc.result;
          if (event._eventId) step.result_event_id = event._eventId;
          if (cc.result_stripped) step.result_stripped = true;
        };
        let resolved = false;
        if (id) {
          for (const e of events) {
            if (e.type === 'step' && e.tool_use_id === id) {
              settleMatchedCallStep(e);
              land(e);
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) {
          const fallback = resolveLastPendingResponseStep(events, isNotThinking);
          if (fallback) land(fallback);
        }
        break;
      }
      case 'CodingAgentThoughtStreamed': {
        // Accumulate streamed reasoning into the live "Thinking" step. If none
        // is pending (a resumed session's initial prompt fires no
        // CodingAgentPromptSent), open one, so a long think does not read as a
        // frozen "Working".
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
        // An exchange boundary in groupIntoExchanges, never a step.
        break;
      case 'SpokenReplyGenerated': {
        // What the caller actually heard. It lands inside the doer's turn
        // because that is when it was said: the talker stalls truthfully while
        // the turn runs, then says what the answer means once it lands.
        //
        // No audio is kept, so this row is the whole record of a spoken turn.
        // It is a marker rather than a step, and renders ungated.
        const e = event as { text: string; interrupted?: boolean };
        if (!hasVisibleText(e.text)) break;
        events.push({
          type: 'spoken_reply',
          text: e.text,
          interrupted: e.interrupted === true,
        });
        break;
      }
      case 'SpokenMessageReceived': {
        // What the caller said when the talker answered it alone. It started
        // no turn, so unlike a delegated utterance it becomes no
        // `MessageReceived` and has no bubble of its own anywhere.
        //
        // Without this row half of a call is missing from the thread: every
        // question the caller asked and the talker fielded itself.
        const e = event as { text: string };
        if (!hasVisibleText(e.text)) break;
        events.push({ type: 'spoken_message', text: e.text });
        break;
      }
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
          // Absent on legacy events, where Undo was restore-only and no counts
          // were recorded. 0 renders as "unknown", not as "none".
          restores: e.restores ?? 0,
          removes: e.removes ?? 0,
        });
        break;
      }
      case 'EventWaitStarted': {
        // The park, as the transcript's ONE record of it. A step-level row,
        // not a divider: the attached delivery resumes THIS exchange, and its
        // steps land below this row.
        //
        // It REPLACES the pending step the `await_event` `ToolCalled` pushed a
        // moment earlier rather than queueing under it. That step's engine
        // description is `Waiting: <reason>` and this row names the same
        // reason, so rendering both puts two near-identical lines in the
        // transcript for one action. This row is the richer of the two, since
        // it carries the subscription and the resolution state. A rejected
        // subscription emits no `EventWaitStarted` at all, so a failed
        // `await_event` keeps its ordinary tool step and its error.
        const e = event as {
          wait_id: string;
          on: EventSubscription[];
          reason: string;
          expires_at: string;
        };
        const row: ResponseEvent = {
          type: 'event_wait',
          wait_id: e.wait_id,
          subscriptions: e.on,
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
        // Both RESOLVE the arming row in place, matched by wait_id: the same
        // subject line, now carrying its outcome, and for a delivery the event
        // that matched plus the jump to it. They enrich the row rather than
        // relabelling it, which is why they may touch it at all.
        //
        // Either can arrive in a LATER exchange than the row that armed it,
        // since a subscription outlives its turn. There is then nothing here
        // to resolve and nothing more to draw: both RE-ENTER the thread, so
        // the delivery already reads as its own turn further down.
        const e = event as {
          wait_id: string;
          event_type?: string;
          event_id?: string;
        };
        const state = event.type === 'EventWaitDelivered' ? 'matched' : 'timed_out';
        for (const prior of events) {
          if (prior.type === 'event_wait' && prior.wait_id === e.wait_id) {
            prior.state = state;
            if (state === 'matched') {
              prior.matched_event_type = e.event_type;
              prior.matched_event_id = e.event_id;
            }
            break;
          }
        }
        break;
      }
      case 'EventWaitCanceled': {
        // **A stop never rewrites the arming row.** "Set up an event wait: X"
        // is a true statement about a moment, and a stop is a different action
        // at a different moment, routinely hours later. Rewriting the row in
        // place leaves nothing anywhere saying when the watch ended.
        //
        // Where the stop DOES appear depends on who stopped it. A user stop is
        // a boundary and renders as its own turn (`isExchangeStartEvent`), so
        // it draws no row here. Every other cause is somebody acting inside a
        // turn, most sharply the agent standing its own watch down. Those get
        // a row at the position it happened.
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
          // A legacy row carries neither, and then names no subscription
          // rather than inventing one.
          subscriptions: e.on ?? [],
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
        // groups into this exchange after its checkpoint.
        const id = (event as { checkpoint_id: string }).checkpoint_id;
        for (const prior of events) {
          if (prior.type === 'checkpoint' && prior.checkpoint_id === id) {
            prior.reverted = true;
            break;
          }
        }
        break;
      }
      // The terminator's KIND decides what a still-pending step becomes. See
      // `TerminalKind`.
      case 'ResponseGenerated':
        terminal = 'clean';
        // A text-less ResponseGenerated is a benign empty completion. The
        // [ENGINE-LIMIT] cap path also emits one with no preceding
        // TextStreamed, but with non-empty text, so checking the event's own
        // text distinguishes the two.
        emptyCompletion = !(event as { text?: string }).text?.trim();
        break;
      case 'CodingAgentIdled':
        terminal = 'clean';
        break;
      case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
        terminal = 'unclean';
        break;
      // The change-lifecycle and question/permission events are
      // exchange-STARTERS (see EXCHANGE_START_TYPES). They render as their own
      // initiator panels and never reach this loop as steps. Their resolution
      // events become steps of the divider exchange and are handled by
      // `describeInitiator`, so no ResponseEvent is synthesized here.
      case 'SessionEnded':
        break;
    }
  }
  // Resolve pending spinners on finished exchanges: a missing ToolResult from
  // a killed session, parallel calls with lost results, or a non-last exchange
  // genuinely abandoned. Mid-flight chat injection means a non-last exchange
  // can still be the one the agentic loop is processing, so do NOT resolve
  // purely on `!isLast`. Wait for a terminator OR for the thread to go idle.
  //
  // WHAT they resolve TO is the terminator's kind (`pendingOutcomeFor`).
  // `threadIdle` on its own keeps resolving to a success deliberately: the
  // quiescent set includes `waiting_for_user_answer`, so a thread parked on a
  // question card would otherwise paint "did not finish" over work that is
  // about to resume.
  //
  // The turn ending is not the only way a spinner strands. The fold can hand
  // a RUNNING turn to a later exchange (`Exchange.continuationMoved`). The
  // LLM's next output lands there, so a Thinking marker pending here is dead.
  // Finalize just those. A pending TOOL step is still owed a result that
  // re-routes back by tool id: the `ask_user_question` spinner must keep
  // running while the card is on screen.
  const finalizeAll = terminal !== null || threadIdle;
  if (finalizeAll || exchange.continuationMoved) {
    const stepEvents = events.filter(e => e.type === 'step') as StepLike[];
    if (finalizeAll) resolvePendingSteps(stepEvents, pendingOutcomeFor(terminal));
    else resolvePendingSteps(stepEvents, 'success', isThinking);
    // Strip trailing Thinking steps: noise from a coding agent processing
    // notifications without producing output. Keep at least one event so
    // canceled and aborted exchanges still show .response-content.
    while (events.length > 1) {
      const last = events[events.length - 1];
      if (last.type === 'step' && isThinking(last)) {
        events.pop();
      } else {
        break;
      }
    }
    // Benign empty completion: the turn finished cleanly with no text and no
    // images. Tool steps may still be present, meaning the model acted but did
    // not summarise. State that plainly instead of leaving a blank body.
    if (
      emptyCompletion
      && !events.some(isMeaningfulText)
      && !events.some(e => e.type === 'image')
    ) {
      events.push({ type: 'empty' });
    }
  } else if (needsLiveThinkingRow({
    exchange,
    isLast,
    hasCCContent,
    anyLive: lastStepIndex(events, isLiveStep) >= 0,
  })) {
    // The turn is live and nothing is running, so the model holds control.
    // See `needsLiveThinkingRow` and ADR 0066.
    pushStep({ type: 'step', description: 'Thinking', outcome: 'pending' });
  }
  return mergeAdjacentTextEvents(events);
}

/** Will rendering these response events actually DRAW anything?
 *
 *  The mirror of `renderResponseEvents` in `ChatExchange.tsx`, which draws a
 *  `text` event only when it is `isMeaningfulText` and every other kind
 *  unconditionally. So the one non-drawing shape is a blank `text`, and it is
 *  not hypothetical: `exchangeResponseEvents` pushes one for EVERY
 *  `CodingAgentTextStreamed`, and a subprocess being torn down signs off with
 *  a bare `"\n\n"`. The "non-empty after trimming" rule is taken from
 *  `isMeaningfulText` rather than re-spelled, so the two cannot drift.
 *
 *  `events.length > 0` is NOT the same question. An abort boundary that
 *  acquired only that flush answers yes there. It then renders a response
 *  panel whose sole visible content is a badge reading "Working" while the
 *  engine is down. Ask this one when deciding whether a panel is worth showing.
 *
 *  An `event_wait` counts as drawn because it IS drawn: it is a marker rather
 *  than step mechanics (`isStepMechanics`), so no toggle can hide it. A `step`
 *  counts even under a collapsed `showSteps` toggle, because the response
 *  header always carries the control that reveals it. */
export function hasRenderableResponseContent(events: ResponseEvent[]): boolean {
  return events.some(e => e.type !== 'text' || isMeaningfulText(e));
}

/** True when this body carries something said out loud, in either direction. */
function hasSpokenRow(events: ResponseEvent[]): boolean {
  return events.some(e => e.type === 'spoken_reply' || e.type === 'spoken_message');
}

/** True when a question divider draws its header and nothing else.
 *
 *  A question resolved without an answer normally has no body: the card says
 *  what happened, and `ensure_resume_after_answer` short-circuits on both
 *  kinds, so nothing follows it in THIS exchange. A superseded question's next
 *  turn belongs to the follow-up's own exchange.
 *
 *  On a VOICE thread something does follow. The divider is an exchange
 *  boundary, so a spoken turn said while the card sat open folds in here as a
 *  step. No other surface carries a spoken row, so suppressing the body took a
 *  whole call with it.
 *
 *  The test is a spoken row rather than `hasRenderableResponseContent`. That
 *  one is true of a step or a cancel too, so it would un-suppress every typed
 *  thread's canceled question as well. */
export function dividerBodyIsSuppressed(exchange: Exchange, events: ResponseEvent[]): boolean {
  return questionDividerResolution(exchange) !== null && !hasSpokenRow(events);
}

/** User-facing presentation of a `StepOutcome`. The outcome IS the CSS class,
 *  which drives the icon and the row's treatment, and this adds the label.
 *
 *  'Did not finish' is deliberately not 'Failed': a step killed mid-execution
 *  never reported anything, while 'Failed' asserts it ran and returned an
 *  error. */
export function stepStatus(outcome: StepOutcome): { label: string; icon: string; className: StepOutcome } {
  switch (outcome) {
    // In-progress rows show no leading mark: the shimmering description is the
    // "live" affordance. The empty icon still gets its fixed-width slot
    // (`.inline-step .step-icon`, steps.css). The running row's text then sits
    // on the same column as the finished rows above it.
    case 'pending': return { label: 'In progress', icon: '', className: 'pending' };
    case 'success': return { label: 'Completed', icon: '✓', className: 'success' };
    case 'error': return { label: 'Failed', icon: '⚠', className: 'error' };
    case 'unfinished': return { label: 'Did not finish', icon: '⊘', className: 'unfinished' };
    // A pause bar, because held-not-running is exactly what was being misread.
    // Text rather than an emoji, for the reason `EVENT_ROW_MARK` gives: these
    // are marks in a column of prose, coloured by the type around them.
    case 'blocked': return { label: 'Needs approval', icon: '‖', className: 'blocked' };
    // The pair of the success check, so the two read as one answered question.
    case 'denied': return { label: 'Denied', icon: '✗', className: 'denied' };
  }
}

/** Whether a non-last exchange's response panel should be hidden as visual
 *  noise. The next exchange's user message implies the chronological flow, so
 *  a panel that produced no real output is not worth a "Done ↳" placeholder.
 *
 *  Empty means no response text, and every event is either a bare 'Thinking'
 *  step or a text event with no visible output. Coding-agent follow-ups race
 *  the user: the loop emits a Thinking marker, sometimes with a
 *  whitespace-only text header, before producing any tool call or text. That
 *  leaves an interrupted exchange with stray steps saying nothing the status
 *  indicator does not. */
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

/** How a `UserQuestionAsked` divider was resolved WITHOUT the user answering
 *  it, or `null` when it was answered or is still open.
 *
 *  `canceled` is the cancel stamp, from the user clicking Cancel or from
 *  `archive_thread` clearing the pending question. `superseded` is a follow-up
 *  that could not be the answer and replaced the question instead.
 *
 *  Either way no response body follows in THIS exchange, so the panel stays
 *  hidden rather than stranding as an empty "Working" badge.
 *  `ensure_resume_after_answer` short-circuits on both kinds, and a superseded
 *  question's next turn belongs to the follow-up's own exchange. The card's own
 *  resolved state already tells the story. */
export function questionDividerResolution(
  exchange: Exchange,
): 'canceled' | 'superseded' | null {
  if (exchange.userEvent.type !== 'UserQuestionAsked') return null;
  for (const { event } of exchange.steps) {
    if (event.type !== 'UserQuestionAnswered') continue;
    if (event.answer.kind === 'Canceled') return 'canceled';
    if (event.answer.kind === 'Superseded') return 'superseded';
  }
  return null;
}

/** Change-lifecycle banner exchanges whose body may carry a post-boundary CC
 *  continuation. Excludes `ChangeApplyFailed` — its initiator renders the error
 *  and the change stays pending, so it has no "continued work" body. */
const CHANGE_CONTINUATION_PANELS: ReadonlySet<string> = new Set([
  'ChangeApplied',
  'ChangeDiscarded',
  'ChangeReverted',
]);

/** True for a change-lifecycle banner exchange that ALSO carries coding-agent
 *  work as steps. The session kept going after the apply and produced more
 *  text or tool calls. No new user message anchored a fresh exchange, so those
 *  events folded into the change exchange.
 *
 *  The banner normally suppresses its response body (`showResponsePanel` in
 *  ChatExchange). When this is true the body must render, or the continued
 *  work is invisible between two "Change applied" rows. Idle and snapshot-only
 *  steps do not count: only output `exchangeResponseEvents` would render. */
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
   *  as `data-event-id`, which is what resolves a notification deep-link to a
   *  failure (`scrollToEventAndPulse`) and pulses the card itself.
   *  `ResponseFailed` is a step, not an exchange starter, so the exchange
   *  root's `data-event-id` carries a different event. Absent on legacy rows
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

/** Every event id this exchange puts into the DOM as `data-event-id`: the
 *  complete set a deep-link can resolve against inside this turn.
 *
 *  Exactly two, both in `ChatExchange`. The root carries the turn's STARTER,
 *  and the failure card carries its own `ResponseFailed` (see
 *  `ExchangeError.eventId` above). Every other step is deliberately unstamped,
 *  inline steps most of all: the "Show steps" toggle can hide them, and an id
 *  there would resolve only sometimes.
 *
 *  Declared here rather than inferred at the deep-link site, so the stamping
 *  rule has ONE definition. `deepLinkAnchorForEvent` reads it to decide
 *  whether an event addresses itself. A source-scan tripwire in
 *  `__tests__/deep-link-anchor.test.ts` asserts the `data-event-id`
 *  expressions in `ChatExchange.tsx` are exactly the two below. Add a third
 *  stamp to the component and that tripwire fails until it is declared here. */
export function stampedEventIds(exchange: Exchange): string[] {
  const ids: string[] = [];
  // Both stamps are conditional in the component, since Preact drops an
  // `undefined` attribute. An id the DOM will not carry is not listed here.
  if (exchange.userEvent._eventId) ids.push(exchange.userEvent._eventId);
  const failure = exchangeError(exchange)?.eventId;
  if (failure) ids.push(failure);
  return ids;
}

/** Events that merely LANDED in a turn rather than being produced by it, and
 *  that render nothing of their own. They have no anchor at all.
 *
 *  The containing-turn fallback below infers "this step has no element of its
 *  own, so show the turn that produced it". Sound for a step the turn caused,
 *  false for an event that arrived asynchronously and was grouped into
 *  whichever exchange happened to be open. A background bash task finishing
 *  under an open question would pulse that question, which the two are
 *  causally unrelated to.
 *
 *  Deliberately an explicit list rather than a clever predicate. Membership is
 *  a claim about CAUSATION, which nothing in the event's shape reveals, so it
 *  is stated per type with the reasoning attached and grows on evidence.
 *
 *  `BackgroundBashStarted` is deliberately NOT here. The turn's own
 *  `run_bash_background` call emits it, so landing there is honest. Only the
 *  COMPLETION floats free, firing whenever the process happens to exit. */
const UNANCHORABLE_ASYNC_EVENTS: ReadonlySet<string> = new Set([
  'BackgroundBashCompleted',
]);

/** The `data-event-id` a deep-link to `eventId` should target within
 *  `exchanges`, or `null` when there is nowhere honest to land.
 *
 *  Three answers, in order:
 *
 *   - An event that stamps its own element (`stampedEventIds`) is its own
 *     target, so the pulse stays on the thing the user was sent to see.
 *   - An event the turn PRODUCED but that renders no addressable element
 *     targets the turn containing it. That is the difference between a link
 *     that works and one that spends the 4s resolve deadline.
 *   - An event that merely arrived inside an open turn
 *     (`UNANCHORABLE_ASYNC_EVENTS`) has no anchor, and says so.
 *
 *  `null` is a real answer rather than a failure. Callers read it as "nowhere
 *  to go". The delivery and trigger rows then render no jump affordance rather
 *  than a tap that pulses something unrelated.
 *
 *  This is what the *event wait*'s "show it" needs, since a wait can match ANY
 *  event type and commonly matches a `CodingAgentIdled` from another thread.
 *  Notification deep-links point at addressable events by construction, so
 *  they deliberately do not use this. */
export function deepLinkAnchorForEvent(
  exchanges: Exchange[],
  eventId: string,
): string | null {
  // Backward walk, matching `findExchangeByAnchorId`: on the vanishingly rare
  // id collision the most recent owner is the one on screen.
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const exchange = exchanges[i];
    if (stampedEventIds(exchange).includes(eventId)) return eventId;
    const step = exchange.steps.find(({ event }) => event._eventId === eventId);
    if (step) {
      if (UNANCHORABLE_ASYNC_EVENTS.has(step.event.type)) return null;
      // A turn whose own starter is unstamped (a legacy row) gives the
      // deep-link nothing to aim at. Saying so beats an `undefined` that would
      // read as "not in this thread" further down.
      return exchange.userEvent._eventId ?? null;
    }
  }
  return null;
}

/** True when this event is an abort that took the ENGINE down with it: an
 *  `engine_shutdown` abort, whoever asked for the shutdown. The event-shaped
 *  reading of `isEngineDownAbort`, where the split from the switch fingerprint
 *  is argued.
 *
 *  It licenses exactly one conclusion, and both callers want that one. The
 *  boundary is CLOSED BY CONSTRUCTION, whatever the thread's projection says.
 *  Nothing new can land in it: the engine that would produce the work is gone,
 *  and its successor's resume opens an exchange of its own. What DOES land is
 *  the dying subprocess's drain, which is not work by any reading.
 *
 *  Distinct from `abortPromisesAutoResume`, which is this plus a device actor
 *  and answers "will the engine bring the turn back?". Not interchangeable in
 *  either direction: a `stale_settle` abort carries a device actor without the
 *  engine going anywhere, and an unattributed shutdown takes the engine down
 *  without promising anything. */
export function abortTookEngineDown(ev: ThreadEvent): boolean {
  return ev.type === 'ResponseAborted' && isEngineDownAbort(ev.cause);
}

/** Everything a call can leave in an exchange without a turn behind it.
 *
 *  The two spoken rows are what a reader sees. The session pair and the
 *  delegation marker draw nothing, yet they still fold in as steps. A set
 *  naming only the visible two would answer `false` for most real calls.
 *
 *  A delegation is in the set deliberately. It sits beside a `MessageReceived`,
 *  which is a boundary, so that turn opens an exchange of its own. The marker
 *  left here reports no work landing HERE. */
const VOICE_ONLY_STEP_TYPES: ReadonlySet<string> = new Set([
  'SpokenMessageReceived',
  'SpokenReplyGenerated',
  'VoiceSessionStarted',
  'VoiceSessionEnded',
  'WorkDelegated',
]);

/** True when this exchange is a stretch of a call and holds no turn at all.
 *
 *  A call is not a turn (ADR 0148), so such an exchange never gets a
 *  terminator: nothing was running to end. Read as an unterminated turn it
 *  looks exactly like a crash, which is what stamped "Aborted" on every
 *  finished call.
 *
 *  Both halves are required. The boundary must be a spoken turn, so an ordinary
 *  turn that merely OVERLAPPED a call is untouched. And every step must be a
 *  voice row, so a delegation landing here later takes the exchange back to the
 *  ordinary machinery. */
function isCallOnly(exchange: Exchange): boolean {
  if (!isSpokenTurn(exchange.userEvent)) return false;
  return exchange.steps.every(s => VOICE_ONLY_STEP_TYPES.has(s.event.type));
}

/** True when the call this exchange holds has rung off.
 *
 *  Keyed on the LAST session event, not on whether an end exists anywhere. A
 *  second call on the same thread then reads as live until its own end lands.
 *
 *  No session event at all means live, not ended. A call's first
 *  `VoiceSessionStarted` precedes every exchange, so the fold has nowhere to
 *  put it and drops it (`foldEvent`). Its end always lands, including the one
 *  the boot sweep writes for a call whose engine died. */
function callHasEnded(exchange: Exchange): boolean {
  for (let i = exchange.steps.length - 1; i >= 0; i--) {
    const type = exchange.steps[i].event.type;
    if (type === 'VoiceSessionEnded') return true;
    if (type === 'VoiceSessionStarted') return false;
  }
  return false;
}

/** True when this event is an abort the engine has PROMISED to resume: the
 *  teardown boundary of a user-initiated *Switch to new version*. The
 *  event-shaped reading of `isSwitchTeardownAbort`, where the fingerprint is
 *  defined and cross-referenced against the backend.
 *
 *  This is why the Continue button is withheld (`continuableAbortIndex`), and
 *  it is the same predicate that decides the thread's `paused` status on the
 *  backend, so the dot and the button cannot contradict each other. The engine
 *  keeps or withdraws the promise, and nothing guesses at it here. A boot that
 *  declines to resume emits a fresh `recovery_after_restart` abort, which does
 *  not match this, re-arming the button and turning the dot red.
 *
 *  Narrower than `abortTookEngineDown` above by the actor, and the two are not
 *  substitutes. Use this one for anything about the PROMISE (the button, the
 *  wording, the status badge), and that one for whether work is still
 *  running. */
export function abortPromisesAutoResume(ev: ThreadEvent): boolean {
  return ev.type === 'ResponseAborted' && isSwitchTeardownAbort(ev.actor, ev.cause);
}

/** Index of the newest ResponseAborted exchange the user may Continue from, or
 *  `null` when the thread offers no Continue button. Only this exchange
 *  renders the button in AbortPanel, so older aborts are inert.
 *
 *  Four ways the scan ends in `null`:
 *
 *  - A later `ContinuationStarted`: the turn was already resumed.
 *  - A stale-settle abort, i.e. engine cleanup of a stuck-but-already-gone
 *    process fired by a user click. Continue would re-run work the user just
 *    deliberately stopped.
 *  - A switch-teardown abort (`abortPromisesAutoResume`): the engine is
 *    auto-resuming this turn, so offering the button races its own recovery.
 *    A click landing mid-window turns an engine-attributed "Resumed after
 *    engine restart" into a human-attributed "Continued the response".
 *  - The abort boundary itself has since RESOLVED: a terminal event landed
 *    among its steps, so a turn already ran under it and finished. Continue
 *    there re-runs completed work. The scan does not stop at such a boundary,
 *    because an OLDER unresolved abort above it is still continuable. The
 *    recovery marker is the one terminal that does NOT resolve a boundary,
 *    see `abortBoundaryResolved`. */
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
 *  `CodingAgentIdled { reason: engine_restart_interrupt }` that says "this
 *  session was interrupted, offer Continue" (`agent_recovery/recovery.rs`).
 *  `CodingAgentIdled` does not start an exchange, so that marker folds into
 *  the abort as a step and looks exactly like a finished turn. Reading the
 *  engine's offer as its own refusal withholds Continue from every
 *  coding-agent thread a restart touched. A turn that genuinely ran under the
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
 *  the subline ("Reminded the model about N prior tool calls"), or null when
 *  there is no engine note. */
export function resumeEngineNote(exchange: Exchange): { text: string; toolCount: number } | null {
  for (const { event } of exchange.steps) {
    if (event.type === 'UserPromptInjected' && (event as { mode?: ActorMode }).mode === 'engine') {
      const text = (event as { text: string }).text || '';
      // The engine note format from
      // chat/rerun.rs::build_side_effect_summary: "- name(args) → result".
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
 *  interruptions. Derived from the generated contract, minus `shutdown` and
 *  `panic`. The plain strings below are retired reasons that still appear on
 *  legacy DB rows, listed so historical exchanges render as normal lifecycle
 *  ends. `stale_resume` is the exception: it is still current, so the spread
 *  above already covers it. */
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

/** ResponseAborted events superseded by a later same-request_event_id terminal
 *  (ResponseGenerated / ResponseFailed). This models the
 *  engine-restart-then-recovered turn: recovery emits an abort, the rerun
 *  re-uses the original request_event_id, and the eventual success or
 *  definitive failure wins the exchange's verdict.
 *
 *  Strict matching: only events with the SAME non-null request_event_id pair
 *  up. Two different ids in one exchange, or one event missing the field, do
 *  NOT merge, which leaves the no-recovery case unchanged. */
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

/** A pending (optimistic, not-yet-ingested) chat follow-up exchange.
 *  `computeExchanges` synthesizes these from `thread.pendingUserMessages` with
 *  a `_displayCreated` stamp and NO `created`, then sorts them to the end of
 *  the timeline. A coding-agent follow-up goes to stdin immediately and so
 *  carries a real `created`, which makes this predicate chat-only. */
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

/** Whether a non-queued exchange is a live or parked turn that can have chat
 *  follow-ups queued behind it. Terminal lifecycle panels deliberately return
 *  false, so a message sent after idle becomes the active turn rather than a
 *  queued bubble. */
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
 *  A question or permission divider can arrive after a queued MessageReceived
 *  but before injection. Queued indices are therefore tracked independently
 *  rather than as one contiguous trailing run. Coding agents are excluded:
 *  their follow-ups go straight to subprocess stdin, and only chat uses the
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
    // Only the MOST RECENT settled or in-flight turn can own queued
    // follow-ups, so stop at the first non-uningested exchange. If it can be
    // queued behind, the follow-up queues behind it. If it is terminal, the
    // thread idled and the just-sent message IS the active turn.
    //
    // Never walk PAST a terminal turn into older non-terminal ones. An
    // answered `UserQuestionAsked` whose continuation flowed into the next
    // question produces no terminal step. The fresh follow-up would then
    // render as "queued" up in history.
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
  /** The client `event_id` (the events-table PK), which is the `message_id`
   *  the `/chat/queued-message/remove` endpoint retracts by. */
  id: string;
  text: string;
}

/** The thread's queued (un-injected) chat follow-ups, in FIFO order: the set a
 *  user Stop retracts and returns to compose (see
 *  `store/actions/chat.ts::cancelCurrentExchange`). Derived from the same
 *  `queuedFollowupRun` the UI renders "Queued" bubbles from, so what Stop
 *  clears is exactly what the user saw queued. An exchange with no `_eventId`
 *  cannot be retracted by id and is skipped. */
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

/** Index of the exchange the agent is working on: the one that owns the live
 *  stream and reads 'streaming' or 'working', not the literal last exchange.
 *
 *  When the thread is busy, follow-ups typed while it worked are queued. The
 *  active exchange is then the live or parked non-queued turn when one exists,
 *  and otherwise the first stepless user message. When the thread is idle, the
 *  literal last exchange is active. A freshly-sent message is about to be
 *  picked up and must read 'Requesting' rather than 'Queued'. */
export function activeExchangeIndex(exchanges: Exchange[], threadBusy: boolean): number {
  return queuedFollowupRun(exchanges, threadBusy).activeIndex;
}

/** Derive ExchangeStatus for an exchange.
 *
 *  @param isLast the last (newest) exchange in the thread.
 *
 *  @param hasPriorActive a prior exchange is still active, so this one is
 *         queued behind it.
 *
 *  @param threadIdle the coding agent is not producing output (see
 *         `isThreadQuiescent` in store.ts). With no terminal event, the
 *         exchange was interrupted by a crash or a lid close and shows as
 *         'aborted' rather than 'streaming'.
 *
 *  @param threadAwaitingAnswer the backend status is
 *         `waiting_for_user_answer`, so the thread is parked on or resuming
 *         from a question or permission card. Such a thread is NEVER crashed,
 *         so the stale-`'aborted'` detector below must not fire for it. The
 *         client `meta.status` can briefly lag behind the resolution event,
 *         and a just-answered divider would otherwise flash "Aborted" in the
 *         gap. A genuine crash settles to `idle` or `failed`, never to
 *         `waiting_for_user_answer`, so this cannot mask a real abort. */
export function exchangeStatus(exchange: Exchange, streamingBuffer: string, isLast: boolean, hasPriorActive?: boolean, threadIsCC?: boolean, threadIdle = false, threadAwaitingAnswer = false): ExchangeStatus {
  let isComplete = false;
  let isCanceled = false;
  let isAborted = false;
  let isFailed = false;
  let isCC = false;
  let isCCWaiting = false;
  let isSessionEnded = false;
  // SessionEnded with a normal lifecycle reason. Terminal for a coding-agent
  // exchange even when CodingAgentIdled was skipped, as the engine's
  // auto-harden `continue` path can bail out before emitting it.
  let isSessionEndedNormally = false;
  let isShutdown = false;
  // The agent is paused on AskUserQuestion. The QuestionCard owns the action
  // surface, and the exchange reads as "done" so it shows no "Working" spinner
  // while the user thinks. Resume clears this flag and the exchange falls back
  // to coding-agent-working.
  let isWaitingForAnswer = false;
  // The turn registered an *event wait* and has not been re-entered out of it
  // (ADR 0047). `await_event` is terminal and the engine deliberately emits no
  // terminator for the park, because the dangling `ToolCalled{await_event}` IS
  // the slot the delivered event lands in. Without this flag the generic
  // "steps but nothing ended it" fallthrough reads a parked turn as
  // 'streaming'. It is not working: it did its work and parked, and the live
  // state belongs to the waiting indicator.
  let isParkedOnEventWait = false;
  // Did the exchange reach a "completed" state BEFORE any abort or shutdown?
  // Then the abort is a system-injected prompt crash and the user's work was
  // already done. This is what separates "idled, then auto-harden crashed"
  // (which is 'done') from "crashed mid-work" (which is 'aborted').
  let wasCompleted = false;
  let completedBeforeAbort = false;

  const supersededAborts = supersededAbortIndices(exchange.steps);

  // A divider exchange starts in awaiting-answer until a matching resolution
  // lands as a step. Without seeding here, the steps loop sees only the
  // resolution and never the request, so `isWaitingForAnswer` would stay false
  // for a pending divider.
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
        // A Codex mid-turn follow-up redirect is mechanically a cancel, but
        // the user steered rather than stopping. Render it neutrally, like
        // every other follow-up. Only a real Stop sets isCanceled.
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
      // A deliberate lifecycle ending must NOT flash the "engine restarted"
      // aborted banner, even when a CodingAgentPromptSent transiently cleared
      // isCCWaiting. Only `shutdown` and `panic` are system interruptions.
      // Everything else is a normal lifecycle end, including a missing reason
      // on a legacy row, coalesced to `completed`.
      case 'SessionEnded': {
        const reason = event.reason ?? 'completed';
        if (reason === 'shutdown') {
          if (wasCompleted) completedBeforeAbort = true;
          isShutdown = true;
        }
        if (!NORMAL_SESSION_END_REASONS.has(reason)) {
          isSessionEnded = true;
        } else if (reason !== 'stale_resume') {
          // stale_resume is mid-flight: a fresh SessionStarted follows.
          isSessionEndedNormally = true;
        }
        break;
      }
      case 'CodingAgentIdled': isCCWaiting = true; wasCompleted = true; break;
      // A work event after waiting means the agent resumed. On legacy data a
      // user follow-up in the same exchange means new work was requested, so
      // CodingAgentUserMessageSent also resets wasCompleted.
      case 'CodingAgentUserMessageSent':
        isCCWaiting = false; isComplete = false; wasCompleted = false; break;
      case 'CodingAgentToolCalled':
      case 'CodingAgentTextStreamed':
      case 'CodingAgentPromptSent':
        isCCWaiting = false; isComplete = false; isWaitingForAnswer = false;
        // Work after the park says the turn never parked. A coding agent arms
        // its watch through a CLI call inside the turn. It carries on when the
        // call answers that the event already happened, and when the work
        // itself stands the watch down (ADR 0059). The chain below reads its
        // real endings first (`CodingAgentIdled`, a normal `SessionEnded`), so
        // a turn that did park keeps its verdict.
        isParkedOnEventWait = false;
        break;
      // The awaiting CLI call ANSWERING is the agent running again, so it
      // clears the park too, and nothing else. It comes back the moment the
      // event has already happened, seconds before the agent's next call, and
      // the turn must not read done in between. A lone arm rather than a fourth
      // label above: a result can land after `CodingAgentIdled`, where clearing
      // `isCCWaiting` would revive a finished turn. It matches no id, because
      // the CLI route mints its own `cli-<uuid>` for the wait and no agent
      // result carries it (`docs/code-review-priors.md`).
      case 'CodingAgentToolResult': isParkedOnEventWait = false; break;
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
      // the next turn is sendable, and the thread settles (`status_transitions`
      // maps `EventWaitCanceled` to Idle). Leaving the flag set there is what
      // keeps a stopped wait reading "Done" instead of "Aborted". A turn that
      // stood its own watch down and kept working clears the flag above,
      // through the work it went on to do.
      case 'EventWaitDelivered':
      case 'EventWaitExpired':
        isParkedOnEventWait = false; break;
    }
  }

  // A follow-up in a coding-agent thread inherits that context without its own
  // SessionStarted, since the session is shared across exchanges.
  if (threadIsCC) isCC = true;

  const hasSteps = exchange.steps.length > 0;
  // Absorbed-UPI placeholder: the engine emitted a UPI carrying this
  // exchange's MR via injected_message_id, so the response lives in the PRIOR
  // exchange, routed there by request id. The placeholder reads as 'done' and
  // is excluded from the 'interrupted' carve-out below, whose "↳" arrow points
  // the wrong way.
  //
  // "Lives in the prior exchange" is the whole claim, and one shape makes it
  // false: the UPI's own `request_event_id` naming THIS exchange's
  // MessageReceived. The turn is then anchored right here and its steps are on
  // their way in. That is the orphan re-entry (`announce_orphan_batch` in
  // `api/chat.rs`), where a follow-up queued behind a cancelled turn is
  // re-submitted as a turn of its own.
  //
  // Both ids must be PRESENT to draw that conclusion. A legacy row carries
  // neither, and two `undefined`s comparing equal would revoke the placeholder
  // from exactly the case it was written for.
  const onlyStep = exchange.steps.length === 1 ? exchange.steps[0].event : undefined;
  const upiRequestId = (onlyStep as { request_event_id?: string } | undefined)?.request_event_id;
  const anchorId = exchange.userEvent._eventId;
  const announcesThisExchange = !!upiRequestId && !!anchorId && upiRequestId === anchorId;
  const isAbsorbedUpiPlaceholder = onlyStep?.type === 'UserPromptInjected'
    && !!onlyStep.injected_message_id
    && !announcesThisExchange;

  // An ENGINE-DOWN boundary is CLOSED BY CONSTRUCTION, whatever the thread
  // projection says. The engine went down with it, and its resume opens an
  // exchange of its own, so nothing landing here is new work. What does land
  // is the dying subprocess's drain: a rejected `CodingAgentToolResult` and a
  // last `"\n\n"` flush. Coding-agent events fold chronologically rather than
  // by request id, so they land in this boundary as steps.
  //
  // The stale detector below cannot reach that state, because it is gated on
  // `threadIdle` and a teardown leaves no quiescent status behind. A switch
  // settles at `paused`, or at `waiting` when a change was already proposed;
  // an unattributed shutdown settles at `failed`. The drain revives either to
  // `running`. None of those is quiescent, so without this flag the panel
  // shimmers "Working" for the whole teardown-plus-restart window. See
  // `docs/plans/2026-08-06-no-working-label-while-nothing-is-running.md`.
  //
  // Keyed on the abort's CAUSE, not on "is an abort boundary", because a
  // boundary CAN acquire a live turn. A `safety_net` abort fires on a turn the
  // watchdog thought was stuck, the loop keeps going, and real work lands
  // under it. Only a shutdown takes the engine with it.
  //
  // Not keyed on the ACTOR either. Who asked for the shutdown decides whether
  // the engine PROMISED to resume the turn. That drives the `paused` verdict,
  // the wording, the withheld Continue button and the status badge, and says
  // nothing about whether anything is still running. A terminal `stop.sh`
  // tears the engine down just as thoroughly as the button.
  const isEngineDownBoundary = abortTookEngineDown(exchange.userEvent);

  // Stale exchange: the thread's projection says quiescent, but this exchange
  // has steps and no terminal event. The loop or the subprocess died without
  // emitting a terminator, through a crash, a lid close, or a teardown that
  // skipped it. Nothing is running, so the panel must read "Aborted" rather
  // than spin forever. `hasSteps` covers tool calls AND streamed text.
  //
  // EXCEPT under `threadAwaitingAnswer`: a thread parked on or resuming from a
  // question or permission card is never crashed. Its continuation and
  // terminal are in flight, so render it as working. A genuine crash settles
  // to `idle` or `failed`, so this cannot mask a real abort.
  //
  // The absorbed-UPI placeholder is excluded because its lone UPI step means
  // the real response lives in the PRIOR exchange: it is 'done', not crashed.
  //
  // An engine-down boundary supplies the quiescence itself, so it need not
  // wait for the projection to say so. It is deliberately NOT exempt from
  // `threadAwaitingAnswer`, which stands for a live agent mid-answer, a state
  // a torn-down engine cannot be in.
  const isStale =
    (threadIdle || isEngineDownBoundary) && !threadAwaitingAnswer
    && isLast && !isComplete && hasSteps
    && !isAbsorbedUpiPlaceholder;

  if (isFailed) return 'error';
  // An abort or shutdown AFTER the exchange completed does not undo the user's
  // work. The auto-harden crash following a clean idle is the shape.
  if ((isAborted || isShutdown) && completedBeforeAbort) return 'done';
  // Both are system-initiated interruptions, not a user cancel.
  if (isAborted) return 'aborted';
  if (isShutdown) return 'aborted';
  if (isCanceled) return 'canceled';
  // Session ended without a proper response: no ResponseGenerated for chat, no
  // CodingAgentIdled for an agent killed mid-work.
  if (isSessionEnded && !isComplete && !isCCWaiting) return 'aborted';
  // A prior exchange is still active and this one has no events yet, so it is
  // queued. Checked BEFORE the `!isLast` fallthrough, which would otherwise
  // show "No response generated". Coding-agent threads do not queue, since
  // their messages go to stdin. Only the LAST queued exchange shows "Queued":
  // earlier ones were superseded and the empty-non-last rule below takes them.
  //
  // A user stop is a boundary that states its own outcome, and nothing
  // continues out of it. Terminal by construction, so it never spins
  // "Requesting" while an unrelated turn keeps the thread `running`, and never
  // falls through to the stale detector's "Aborted".
  //
  // Placed AFTER the terminal-verdict arms above, defensively. Grouping
  // deliberately does not make this boundary `current`
  // (`isExchangeStartEvent`), so it should hold no steps. If a future routing
  // path lands a real terminal in it, that terminal reports itself.
  if (isUserStoppedWait(exchange.userEvent)) return 'done';
  if (hasPriorActive && !hasSteps && !isCC && isLast) return 'queued';
  // Agent idle. WaitingBanner handles the "can interact" state separately.
  if (isCCWaiting) return 'done';
  // Parked on an event wait: the turn ran to its end and registered a
  // subscription, which is completed work rather than work in flight. Nothing
  // is running, and the surface owning the live half is the subscription
  // indicator, with its countdown and Stop.
  //
  // Placed before the stale detector, so a settled thread whose wait the user
  // stopped does not read "Aborted". Placed before the `!isLast` 'interrupted'
  // arm, so a delivery landing in a later exchange does not retroactively mark
  // the park as abandoned.
  if (isParkedOnEventWait) return 'done';
  // A normal session-end reason is terminal even when CodingAgentIdled was
  // missing.
  if (isCC && isSessionEndedNormally) return 'done';
  // Paused on a question or permission prompt. 'awaiting-answer' stops the
  // surrounding spinner AND makes the header read "Needs your answer" rather
  // than a misleading "Done ✓". The card inside the exchange carries the
  // action surface.
  //
  // `!exchange.questionOvertaken` is what keeps this honest. The switch above
  // clears `isWaitingForAnswer` on only three progression types, while
  // `QUESTION_OVERTAKEN_STEP_TYPES` covers twelve. A shape in the gap reads
  // "Needs your answer" over a card whose buttons are already dead. Deferring
  // to the overtaken flag means one list decides both, mirroring the engine's
  // single park-ending set. An overtaken divider falls through to the stale
  // detector, or to 'coding-agent-working' while the agent is still going.
  if (isWaitingForAnswer && !exchange.questionOvertaken) return 'awaiting-answer';
  // Non-last with steps but no terminator: the user moved past this exchange.
  // The chat fast path injects the follow-up via UPI under the parent's
  // request_event_id and redirects later events to the new exchange. A coding
  // agent shares one session across exchanges. 'interrupted' keeps "Working"
  // on the last panel only.
  if (!isLast && !isComplete && hasSteps && !isAbsorbedUpiPlaceholder) return 'interrupted';
  if (isComplete) return 'done';
  // A non-last coding-agent exchange without a terminator was skipped by the
  // msg_tx queue, so it is safely 'done'.
  if (!isLast && (isCC || threadIdle)) return 'done';
  // Empty chat exchange once the engine has gone idle. This extends the
  // `!isLast` empty-to-done rule to the isLast case, so an MR whose response
  // landed in a sibling exchange does not spin "Requesting" forever. Without
  // `threadIdle` the isLast branch must keep falling through, so a freshly
  // sent MR still reads as 'pending'.
  //
  // Relies on chat's request_event_id serialization invariant: by the time an
  // exchange is non-last, the loop has moved past it. Mid-flight injection
  // routes new events back to the parent's request_event_id, so a non-last
  // exchange the loop is still processing has steps.
  if (!hasSteps && !isCC && (!isLast || threadIdle)) return 'done';
  // A child-completion row is itself terminal: it renders the spawned
  // sub-thread's result. The stepless card is terminal when the parent is
  // QUIESCENT and never resumed to react, or when a newer boundary superseded
  // the card (`!isLast`). 'done' keeps it out of a phantom spinner.
  //
  // While the parent is still RUNNING and this is the last exchange, it is
  // about to react to the completion. The ReentryFromEngine summary went into
  // the live loop and its continuation request-id-routes INTO this card (see
  // the redirect-advance for ChildThreadCompleted in exchange-grouping.ts).
  // Fall through to the normal machinery, so the card shows a live spinner in
  // the gap before the first post-completion step rather than "Done ✓".
  if (userEventType === 'ChildThreadCompleted' && !hasSteps && (threadIdle || !isLast)) return 'done';

  // A coding-agent exchange is 'coding-agent-working' once it has steps.
  //
  // `!isStale` is what stops a DEAD turn reading "Working" forever. Without
  // it, this branch returns unconditionally and a turn whose terminator never
  // landed spins a live-looking spinner on a subprocess that no longer exists.
  // See
  // `docs/plans/2026-08-01-preserve-question-parked-session-through-teardown.md`.
  // A live turn is `running`, so `isStale` is false and this branch wins.
  if (isCC && !isStale) return hasSteps ? 'coding-agent-working' : 'pending';
  // A live streaming buffer beats staleness for either agent: tokens are
  // arriving right now, whatever the projection last said.
  if (streamingBuffer) return 'streaming';

  // The absorbed-UPI placeholder, for the isLast case that the `!isLast`
  // branch above bypasses. It must not read as a crash, which is why `isStale`
  // excludes it outright rather than relying on this line's position.
  if (isAbsorbedUpiPlaceholder) return 'done';

  // A call the talker answered alone, which holds no turn at all. So the
  // stale detector below has nothing to have caught: it reads a transcript
  // with no terminator as a crash, and every finished call rendered "Aborted"
  // until this arm.
  //
  // A call still up is NOT terminal, and the thread cannot say so: voice never
  // moves `status`, so `threadIdle` is true for the whole of a talker-only
  // call. The session's own end is the signal, and one always lands, so
  // nothing can spin here forever.
  //
  // `threadIdle` still guards the arm, to keep it off a doer turn running
  // elsewhere on the thread.
  if (threadIdle && isCallOnly(exchange)) {
    return callHasEnded(exchange) ? 'done' : 'streaming';
  }

  if (isStale) return 'aborted';

  // Persisted response text without a completion event means the response is
  // still in progress: a persisted event arrival just cleared the streaming
  // buffer.
  const responseText = exchangeResponseText(exchange);
  if (responseText) return 'streaming';

  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, isLast, threadIdle);
  if (steps.length > 0 || events.length > 0) return 'streaming';

  return 'pending';
}
