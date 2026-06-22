import { SESSION_END_REASONS } from '../../generated/thread-lifecycle';
import { isMeaningfulText, mergeAdjacentTextEvents } from '../event-rendering';
import { describeCCTool, describeEngineTool, exchangeHasCCContent, exchangeResponseText, exchangeUserImageHashes, fullCommandForCCTool, fullCommandForEngineTool } from './exchange';
import { toolUseIdOf } from './exchange-grouping';
import type { ExchangeStatus } from '../exchange-status';
import type { ContextAssembledData, ContextCapture, ContextSection, ResponseEvent, Step } from '../types';
import type { Exchange } from './exchange';
import type { ActorMode, SequencedEvent, ThreadEvent } from './thread-event-types';

/** Mark the last pending step (success === null) as completed.
 *  Walks backwards so parallel tool calls resolve in LIFO order as results arrive.
 *  Optional `pred` narrows which pending step to resolve (e.g. only "Thinking" steps). */
function resolveLastPendingStep(
  steps: { success: boolean | null; description?: string }[],
  pred?: (s: { description?: string }) => boolean,
): void {
  for (let i = steps.length - 1; i >= 0; i--) {
    if (steps[i].success === null && (!pred || pred(steps[i]))) {
      steps[i].success = true;
      return;
    }
  }
}

/** Force ALL pending steps to completed.
 *  Called after a completion event so spinners don't persist on finished exchanges. */
function resolvePendingSteps(steps: { success: boolean | null }[]): void {
  for (const step of steps) {
    if (step.success === null) step.success = true;
  }
}

const isThinking = (s: { description?: string }) => s.description === 'Thinking';
const isNotThinking = (s: { description?: string }) => !isThinking(s);

/** Bag of legacy events. All optional — `synthesizeContextCapture`
 *  produces something useful from any subset (Thinking-only is the
 *  oldest case). */
export interface LegacyContextEvents {
  thinking?: { text?: string; context_tokens?: number; context_messages?: number; trimmed?: boolean };
  tokensMeasured?: { input_tokens?: number };
  assembled?: { sections?: ContextSection[]; tools?: string[]; model?: string; total_chars?: number };
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
    estimated_total_tokens: legacy.thinking?.context_tokens ?? legacy.assembled?.total_chars ?? 0,
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

/** Build Step[] from exchange events (tool calls with success tracking).
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
  let isComplete = false;
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
        steps.push({ description: results > 0 ? 'Memory searched' : 'Memory: no results', success: true });
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
          success: null,
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
        // LLM produced visible text — the prior Thinking step is done.
        resolveLastPendingStep(steps, isThinking);
        break;
      case 'ToolCalled': {
        // LLM produced a tool call — the prior Thinking step is done.
        resolveLastPendingStep(steps, isThinking);
        const e = event as { name: string; args: unknown; description?: string };
        steps.push({ description: e.description || describeEngineTool(e.name, e.args), success: null });
        break;
      }
      case 'ToolResult':
        resolveLastPendingStep(steps, isNotThinking);
        break;
      case 'CodingAgentPromptSent':
        steps.push({ description: 'Thinking', success: null });
        break;
      case 'CodingAgentToolCalled': {
        resolveLastPendingStep(steps, isThinking);
        const e = event as { name: string; args: unknown; description?: string };
        steps.push({ description: e.description || describeCCTool(e.name, e.args), success: null, tool_use_id: toolUseIdOf(event) });
        isComplete = false; // CC resumed — not finished yet
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
            if (step.success === null && step.tool_use_id === id) {
              step.success = true;
              resolved = true;
              break;
            }
          }
        }
        if (!resolved) resolveLastPendingStep(steps, isNotThinking);
        break;
      }
      case 'ResponseGenerated': case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
      case 'CodingAgentIdled':
        isComplete = true;
        break;
      case 'CodingAgentTextStreamed':
        resolveLastPendingStep(steps, isThinking);
        isComplete = false; // CC resumed — not finished yet
        break;
    }
  }
  if (isComplete || threadIdle) resolvePendingSteps(steps);
  return steps;
}

/** Count images in an exchange (user-pasted + generated) for thread:N offset computation. */
export function exchangeImageCount(exchange: Exchange): number {
  let count = exchangeUserImageHashes(exchange).length;
  for (const { event } of exchange.steps) {
    if (event.type === 'ToolResult') {
      const imgs = (event as { images?: string[] }).images;
      if (imgs?.length) count += imgs.length;
    }
  }
  return count;
}

/** Mark the last pending step in a ResponseEvent[] as completed and return it
 *  so callers can attach extra payload (tool result text, images). Optional
 *  `pred` narrows which pending step to resolve. */
function resolveLastPendingResponseStep(
  events: ResponseEvent[],
  pred?: (s: { description?: string }) => boolean,
): Extract<ResponseEvent, { type: 'step' }> | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.type === 'step' && e.success === null && (!pred || pred(e))) {
      e.success = true;
      return e;
    }
  }
  return null;
}

/** Build ResponseEvent[] from exchange events (interleaved text + steps for rendering).
 *  `imageOffset` is the number of images in all previous exchanges (for thread:N numbering).
 *  @param _isLast — kept for caller compatibility; no longer drives spinner resolution
 *  on its own. See `threadIdle`.
 *  @param threadIdle — true when CC is not producing output (see
 *  `isThreadQuiescent` in store.ts). Combined with the in-exchange completion
 *  flag to finalize pending steps. A non-last exchange can still be the one
 *  the engine is actively processing (chat mid-flight injection), so
 *  resolution must not trigger purely on `!isLast`. */
export function exchangeResponseEvents(exchange: Exchange, imageOffset = 0, _isLast = true, threadIdle = false): ResponseEvent[] {
  const events: ResponseEvent[] = [];
  const hasCCContent = exchangeHasCCContent(exchange);
  // Count images across the thread for thread:N numbering — starts after user images in this exchange
  let imageCounter = imageOffset + exchangeUserImageHashes(exchange).length;
  let isComplete = false;
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
        pushStep({ type: 'step', description: results > 0 ? 'Memory searched' : 'Memory: no results', success: true, detail, created });
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
          success: null,
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
        // LLM produced a tool call — the prior Thinking step is done.
        resolveLastPendingResponseStep(events, isThinking);
        const e = event as { name: string; args: unknown; description?: string };
        const description = e.description || describeEngineTool(e.name, e.args);
        const full = fullCommandForEngineTool(e.name, e.args);
        pushStep({ type: 'step', description, tool_name: e.name, success: null, full, created });
        break;
      }
      case 'ToolResult': {
        const toolResult = event as { result?: string; images?: string[]; result_stripped?: boolean };
        // Skip pending Thinking — ToolCalled already resolved it; this should
        // pair with the matching tool step.
        const resolved = resolveLastPendingResponseStep(events, isNotThinking);
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
        // Render generated images inline
        if (toolResult.images?.length) {
          for (const b64 of toolResult.images) {
            imageCounter++;
            events.push({ type: 'image', base64: b64, mime_type: 'image/jpeg', index: imageCounter });
          }
        }
        break;
      }
      case 'TextStreamed':
        // LLM produced visible text — the prior Thinking step is done.
        resolveLastPendingResponseStep(events, isThinking);
        events.push({ type: 'text', md: (event as { text: string }).text });
        break;
      case 'SessionStarted':
        if (hasCCContent) events.push({ type: 'section_break', channel: 'claude_code' });
        break;
      case 'CodingAgentPromptSent':
        pushStep({ type: 'step', description: 'Thinking', success: null, created });
        break;
      case 'CodingAgentToolCalled': {
        resolveLastPendingResponseStep(events, isThinking);
        const e = event as { name: string; args: unknown; description?: string };
        const description = e.description || describeCCTool(e.name, e.args);
        const full = fullCommandForCCTool(e.name, e.args);
        pushStep({ type: 'step', description, tool_name: e.name, success: null, tool_use_id: toolUseIdOf(event), full, created });
        isComplete = false; // CC resumed — not finished yet
        break;
      }
      case 'CodingAgentToolResult': {
        // See exchangeSteps for the pairing rationale.
        const id = toolUseIdOf(event);
        const ccResult = (event as { result?: string }).result;
        let resolved = false;
        if (id) {
          for (const e of events) {
            if (e.type === 'step' && e.success === null && e.tool_use_id === id) {
              e.success = true;
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
      case 'CodingAgentTextStreamed':
        resolveLastPendingResponseStep(events, isThinking);
        events.push({ type: 'text', md: (event as { text: string }).text });
        isComplete = false; // CC resumed — not finished yet
        break;
      case 'CodingAgentUserMessageSent':
        // Legacy event — now an exchange boundary in groupIntoExchanges, never a step
        break;
      case 'CommandCheckpointed': {
        // Command guard snapshot before a ReversibleDanger command (ADR 0002,
        // Phase 4) — renders inline with a one-click Undo.
        const e = event as { checkpoint_id: string; command: string; summary: string };
        events.push({
          type: 'checkpoint',
          checkpoint_id: e.checkpoint_id,
          command: e.command,
          summary: e.summary,
          reverted: false,
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
      case 'ResponseGenerated':
        isComplete = true;
        // A text-less ResponseGenerated is a benign empty completion. The
        // [ENGINE-LIMIT] cap path also emits a ResponseGenerated with no
        // preceding TextStreamed, but with non-empty text — so checking the
        // event's own text distinguishes the two.
        emptyCompletion = !(event as { text?: string }).text?.trim();
        break;
      case 'ResponseCanceled': case 'ResponseAborted': case 'ResponseFailed':
      case 'CodingAgentIdled':
        isComplete = true;
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
  if (isComplete || threadIdle) {
    const stepEvents = events.filter(e => e.type === 'step') as { success: boolean | null }[];
    resolvePendingSteps(stepEvents);
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

/** Tri-state mapping for a step's `success` field: null = pending, true =
 *  succeeded, false = failed. Both the inline-step row and the detail modal
 *  consume this — the class drives the icon color, the label is user-facing. */
export function stepStatus(success: boolean | null): { label: string; className: 'pending' | 'success' | 'error' } {
  if (success === null) return { label: 'In progress', className: 'pending' };
  if (success) return { label: 'Completed', className: 'success' };
  return { label: 'Failed', className: 'error' };
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

/** Get the error message from a failed exchange. */
export function exchangeError(exchange: Exchange): string {
  for (const { event } of exchange.steps) {
    if (event.type === 'ResponseFailed') return event.error;
  }
  return '';
}

/** Index of the last (newest) ResponseAborted exchange that has NO later
 *  ContinuationStarted exchange anywhere in the thread. Used by AbortPanel to
 *  decide whether to render the Continue button — only the unresumed abort
 *  shows it; older aborts that the user already continued past are inert.
 *
 *  Stale-settle aborts (engine cleanup of a stuck-but-already-gone process,
 *  fired by the user's Stop/Apply/Discard/Archive/Interrupt click) are
 *  treated like ContinuationStarted: they terminate the scan with `null`.
 *  Clicking Continue would re-run work the user just deliberately stopped. */
export function unresumedAbortIndex(exchanges: Exchange[]): number | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const ev = exchanges[i].userEvent;
    if (ev.type === 'ContinuationStarted') return null;
    if (ev.type === 'ResponseAborted') {
      if (ev.cause === 'stale_settle') return null;
      return i;
    }
  }
  return null;
}

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
  const onlyStep = exchange.steps.length === 1 ? exchange.steps[0].event : undefined;
  const isAbsorbedUpiPlaceholder = onlyStep?.type === 'UserPromptInjected' && !!onlyStep.injected_message_id;

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
  if (hasPriorActive && !hasSteps && !isCC && isLast) return 'queued';
  // CC idle → done. WaitingBanner handles the "can interact" state separately.
  if (isCCWaiting) return 'done';
  // Claude Code session ended with a normal reason (changes_proposed, completed, etc.) —
  // terminal even when CodingAgentIdled was missing.
  if (isCC && isSessionEndedNormally) return 'done';
  // CC paused on a user question or permission prompt — render as
  // 'awaiting-answer' so the surrounding spinner stops AND the header reads
  // "Needs your answer" (not the misleading "Done ✓"). The QuestionCard /
  // PermissionCard inside the exchange shows the action surface.
  if (isWaitingForAnswer) return 'awaiting-answer';
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
  // A child-completion card is itself terminal — it renders the spawned
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
  if (isCC) return hasSteps ? 'coding-agent-working' : 'pending';
  if (streamingBuffer) return 'streaming';

  // Absorbed-UPI placeholder: handled here for the isLast case (the
  // !isLast branch above bypasses 'interrupted' for it). Must run before
  // the threadIdle stale-detector to avoid a false 'aborted'.
  if (isAbsorbedUpiPlaceholder) return 'done';

  // Stale exchange: thread DB says idle but exchange has no terminal event and
  // no live streaming buffer. This happens when the engine crashed or lid closed
  // mid-response — the agentic loop died without emitting ResponseGenerated or
  // ResponseAborted. Detect this BEFORE the streaming fallbacks so we show
  // "Aborted" instead of an eternal "Working" spinner.
  // hasSteps covers both tool calls AND TextStreamed events (both are in exchange.steps).
  //
  // EXCEPT when the thread is `waiting_for_user_answer` (threadAwaitingAnswer):
  // a thread parked on / resuming from a question or permission card is never
  // crashed. A just-answered question-divider (UserQuestionAnswered step, no
  // terminal yet) whose resume `running` aggregate hasn't reached the client
  // would otherwise flash "Aborted" during the answer→resume gap. The agent's
  // continuation (and its terminal) is in flight; render it as working, not
  // crashed. A genuine crash settles to `idle`/`failed`, never
  // `waiting_for_user_answer`, so this can't mask a real abort.
  if (threadIdle && !threadAwaitingAnswer && isLast && !isComplete && hasSteps) return 'aborted';

  // Persisted response text (TextStreamed events) without a completion event
  // means the response is still in progress — the streaming buffer was just
  // cleared by a persisted event arrival. Show 'streaming', not 'done'.
  const responseText = exchangeResponseText(exchange);
  if (responseText) return 'streaming';

  const steps = exchangeSteps(exchange, isLast, threadIdle);
  const events = exchangeResponseEvents(exchange, 0, isLast, threadIdle);
  if (steps.length > 0 || events.length > 0) return 'streaming';

  return 'pending';
}
