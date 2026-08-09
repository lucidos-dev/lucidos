import { signal, useSignal } from '@preact/signals';
import { useEffect, useMemo, useRef } from 'preact/hooks';
import { showToast } from '../../store/store';
import { answerThreadQuestion } from '../../store/actions/chat-claude-code';
import { createTapGate } from '../../utils/tapGesture';
import { renderMarkdownInline, renderMarkdownInlineWithLinks } from '../../utils/renderMarkdown';
import { CHOICE_CARD_ROLE, handleChoiceCardKeyDown, seedChoiceCardFocus } from './choiceCardNav';
import { followAnsweredQuestion } from './scrollState';

export type ResolvedAnswer =
  | { kind: 'Selected'; option_id: string }
  | { kind: 'FreeText'; text: string }
  | { kind: 'MultiSelected'; option_ids: string[]; text?: string }
  | { kind: 'Canceled' };

export interface QuestionBodyProps {
  threadId: string;
  toolUseId: string;
  question: string;
  options: Array<{ id: string; label: string; description?: string }>;
  multiSelect?: boolean;
  resolved?: ResolvedAnswer;
  /** Surrounding response was canceled / aborted / failed / superseded
   *  without an answer landing — render every option disabled. */
  terminated?: boolean;
}

// Multi-select state lives at module level so PromptInput can read selections
// + write the optimistic answer. The QuestionBody useEffect below drains both
// maps when the persisted UserQuestionAnswered lands.
export const multiSelectedByToolUse = signal<Map<string, string[]>>(new Map());
export const pendingAnswerByToolUse = signal<Map<string, ResolvedAnswer>>(new Map());

export function getMultiSelectedIds(toolUseId: string): string[] {
  return multiSelectedByToolUse.value.get(toolUseId) ?? [];
}

export function setMultiSelectedIds(toolUseId: string, ids: string[]): void {
  const map = multiSelectedByToolUse.value;
  if (ids.length === 0 && !map.has(toolUseId)) return;
  const next = new Map(map);
  if (ids.length === 0) next.delete(toolUseId);
  else next.set(toolUseId, ids);
  multiSelectedByToolUse.value = next;
}

function toggleMultiSelectedId(toolUseId: string, optionId: string): void {
  const current = getMultiSelectedIds(toolUseId);
  setMultiSelectedIds(
    toolUseId,
    current.includes(optionId) ? current.filter(x => x !== optionId) : [...current, optionId],
  );
}

function clearMultiSelected(toolUseId: string): void {
  setMultiSelectedIds(toolUseId, []);
}

export function setPendingAnswer(toolUseId: string, answer: ResolvedAnswer): void {
  const next = new Map(pendingAnswerByToolUse.value);
  next.set(toolUseId, answer);
  pendingAnswerByToolUse.value = next;
}

export function clearPendingAnswer(toolUseId: string): void {
  if (!pendingAnswerByToolUse.value.has(toolUseId)) return;
  const next = new Map(pendingAnswerByToolUse.value);
  next.delete(toolUseId);
  pendingAnswerByToolUse.value = next;
}

/** Radio (single) vs checkbox (multi) shape on each option, distinct from the
 *  start so the user can tell the mode apart before any click. The selected
 *  state is also rendered visually so the same component works for the live
 *  options and the answered-state recap. */
export function OptionIndicator({
  multiSelect,
  selected,
}: {
  multiSelect: boolean;
  selected: boolean;
}) {
  const shape = multiSelect ? 'question-option-indicator-checkbox' : 'question-option-indicator-radio';
  const sel = selected ? ' question-option-indicator-selected' : '';
  return <span class={`question-option-indicator ${shape}${sel}`} aria-hidden="true" />;
}

/** Shared indicator + label/desc layout used by both the live OptionButton
 *  and the AnsweredBody static row. Keeps the two surfaces visually identical
 *  so resolving a question doesn't reflow. The flat fragment relies on the
 *  parent grid (`.question-option` / `.question-option-static`) to place
 *  desc on its own row spanning both columns. */
function OptionContent({
  option,
  multiSelect,
  selected,
}: {
  option: { id: string; label: string; description?: string };
  multiSelect: boolean;
  selected: boolean;
}) {
  return (
    <>
      <OptionIndicator multiSelect={multiSelect} selected={selected} />
      <span class="question-option-label">{option.label}</span>
      {option.description && (
        <span
          class="question-option-desc"
          dangerouslySetInnerHTML={{ __html: renderMarkdownInline(option.description) }}
        />
      )}
    </>
  );
}

/** The question prompt itself. Rendered as inline markdown with live links so
 *  a URL the LLM pastes into the question (e.g. a PR link) is clickable rather
 *  than dead text. It sits in a plain `<div>`, not a `<button>`, so real `<a>`
 *  elements are valid here — unlike the option label/desc inside OptionButton.
 *  Shared across the live, answered, and terminated bodies so all three render
 *  the question identically. */
function QuestionText({ question }: { question: string }) {
  return (
    <div
      class="question-text"
      dangerouslySetInnerHTML={{ __html: renderMarkdownInlineWithLinks(question) }}
    />
  );
}

/** Body of an `AskUserQuestion` divider exchange — rendered inside the
 *  initiator panel which provides the chrome (border, header, timestamp).
 *  Multi-select Submit lives in the prompt action row (PromptInput.tsx); the
 *  card just renders toggleable options and reads its optimistic / resolved
 *  state from module-level signals.
 *
 *  The card is the question and its options, nothing else. The two escapes that
 *  need no option slot are named by the prompt row instead: typing (which routes
 *  to this question as a `FreeText` answer) by the prompt textarea's placeholder,
 *  `PLACEHOLDER_ANSWERING` in `prompt-input-helpers.ts`, and Cancel by the
 *  prompt row's Cancel tooltip, `ANSWER_CANCEL_TOOLTIP` in `PromptInput.tsx`. A
 *  guide line under the options said the same thing a few pixels above the field
 *  it pointed at, so it is gone; do not reintroduce one (pinned by
 *  `question-card.test.tsx`). Naming the escapes at all is load-bearing: there
 *  is no text-entry option kind, every option resolves to its LABEL when picked,
 *  and an agent with nothing telling it otherwise invents an "Other, I'll type
 *  it" row that hands that phrase back as the user's answer. The agent-side half
 *  lives in the `ask_user_question` tool description and the question rules in
 *  the engine prompts. */
export function QuestionBody({ threadId, toolUseId, question, options, multiSelect, resolved, terminated }: QuestionBodyProps) {
  // Single-select keeps a local pending — nothing outside the card needs it.
  const localPending = useSignal<ResolvedAnswer | null>(null);

  // Drain the module-level maps once the persisted answer lands. Without this,
  // selections + optimistic pending leak across the session.
  useEffect(() => {
    if (!resolved) return;
    clearPendingAnswer(toolUseId);
    clearMultiSelected(toolUseId);
  }, [resolved, toolUseId]);

  const liftedPending = pendingAnswerByToolUse.value.get(toolUseId);
  const effective = resolved ?? liftedPending ?? localPending.value ?? undefined;
  if (effective) {
    return <AnsweredBody toolUseId={toolUseId} question={question} options={options} multiSelect={multiSelect} resolved={effective} />;
  }
  if (terminated) {
    return <TerminatedQuestionBody question={question} options={options} multiSelect={multiSelect} />;
  }

  if (multiSelect) {
    const selected = multiSelectedByToolUse.value.get(toolUseId) ?? [];
    return (
      <div class="question-body" data-tool-use-id={toolUseId}>
        <QuestionText question={question} />
        {options.length > 0 && (
          <LiveOptions
            toolUseId={toolUseId}
            options={options}
            selectedIds={selected}
            onActivate={(id) => toggleMultiSelectedId(toolUseId, id)}
          />
        )}
      </div>
    );
  }

  const onPick = async (optionId: string) => {
    localPending.value = { kind: 'Selected', option_id: optionId };
    // Answering is a send: take the reader to what they just answered and keep
    // them at the live edge while the agent resumes underneath it. Before the
    // await, because the scroll is the composer's-tap half of the action and
    // must not wait on the round trip. See `followAnsweredQuestion`.
    followAnsweredQuestion(toolUseId);
    const ok = await answerThreadQuestion(threadId, toolUseId, { kind: 'Selected', option_id: optionId });
    if (!ok) {
      localPending.value = null;
      showToast('Could not send answer — please try again.', 'error');
    }
  };

  return (
    <div class="question-body" data-tool-use-id={toolUseId}>
      <QuestionText question={question} />
      {options.length > 0 && (
        <LiveOptions toolUseId={toolUseId} options={options} onActivate={onPick} />
      )}
    </div>
  );
}

/** The option list of a LIVE question card, shared by the single- and
 *  multi-select bodies. It is the card's *choice card* surface (see
 *  `choiceCardNav.ts`): arrow keys step between options and the first option
 *  takes DOM focus on arrival, so Enter answers without reaching for the mouse.
 *  Only rendered while the question is unresolved and not terminated, which is
 *  what keeps the marker (and therefore keyboard focus) off historical cards. */
function LiveOptions({
  toolUseId,
  options,
  selectedIds,
  onActivate,
}: {
  toolUseId: string;
  options: QuestionBodyProps['options'];
  /** Toggled ids for a multi-select question; omitted for single-select, which
   *  is what switches `OptionButton` between toggle and one-shot pick. */
  selectedIds?: string[];
  onActivate: (id: string) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  // Keyed on `toolUseId`, and latched inside `seedChoiceCardFocus` so it fires
  // on the card's ARRIVAL only. A failed answer rolls the optimistic pending
  // back, which remounts this component; without the latch that would re-seed
  // option 0 over whichever option the user actually picked.
  useEffect(() => { seedChoiceCardFocus(ref.current, toolUseId); }, [toolUseId]);
  return (
    <div
      class="question-options"
      data-role={CHOICE_CARD_ROLE}
      ref={ref}
      onKeyDown={(e) => handleChoiceCardKeyDown(e, ref.current)}
    >
      {options.map(opt => (
        <OptionButton
          key={opt.id}
          option={opt}
          pressed={selectedIds ? selectedIds.includes(opt.id) : undefined}
          onActivate={onActivate}
        />
      ))}
    </div>
  );
}

/** Each option owns its own tap gate. Without scroll-vs-tap detection, an
 *  iOS Safari user dragging to scroll the chat could land a `click` on the
 *  button if the touch happened to stay under iOS's native cancel threshold —
 *  which silently dispatches the answer to CC and resumes the session.
 *
 *  When `pressed` is provided, the button renders as a multi-select toggle
 *  (aria-pressed reflects state, click toggles); when undefined, it's a
 *  single-pick button (click dispatches the answer once). */
function OptionButton({
  option,
  pressed,
  onActivate,
}: {
  option: { id: string; label: string; description?: string };
  pressed?: boolean;
  onActivate: (id: string) => void;
}) {
  const gate = useMemo(() => createTapGate(), []);
  const isToggle = pressed !== undefined;
  return (
    <button
      type="button"
      class="question-option"
      aria-pressed={isToggle ? pressed : undefined}
      onPointerDown={e => gate.down(e.clientX, e.clientY)}
      onPointerMove={e => gate.move(e.clientX, e.clientY)}
      onPointerCancel={() => gate.cancel()}
      onClick={() => {
        if (!gate.isTap()) return;
        onActivate(option.id);
      }}
      aria-label={`${isToggle ? 'Toggle' : 'Answer'}: ${option.label}`}
    >
      <OptionContent option={option} multiSelect={isToggle} selected={!!pressed} />
    </button>
  );
}

/** Resolved-state rendering: options dim, the picked one is highlighted; the
 *  Custom-answer block surfaces freetext (FreeText answers, or the freetext
 *  typed alongside a MultiSelected); Canceled renders a disabled Cancel button
 *  styled like the picked permission affordance. Exported for unit tests.
 *
 *  Carries `data-tool-use-id` like the live body it replaces, so the answer's
 *  landing glide (`questionCardTurn` in scrollState) finds the card whether or
 *  not this swap has already happened. The two submit sites resolve it
 *  synchronously, before the render, so today they see the live body; depending
 *  on that ordering to make the id unnecessary here would be one Preact
 *  scheduling change away from silently losing the glide. The TERMINATED body
 *  carries no id on purpose: a dead question is one nobody can answer, so nothing
 *  ever looks it up. */
export function AnsweredBody({
  toolUseId,
  question,
  options,
  multiSelect,
  resolved,
}: {
  toolUseId: string;
  question: string;
  options: QuestionBodyProps['options'];
  multiSelect: boolean | undefined;
  resolved: ResolvedAnswer;
}) {
  const isSelected = (id: string) =>
    (resolved.kind === 'Selected' && resolved.option_id === id) ||
    (resolved.kind === 'MultiSelected' && resolved.option_ids.includes(id));
  const customText =
    resolved.kind === 'FreeText' ? resolved.text
    : resolved.kind === 'MultiSelected' ? resolved.text
    : undefined;
  return (
    <div class="question-body question-body-answered" data-tool-use-id={toolUseId}>
      <QuestionText question={question} />
      {options.length > 0 && (
        <div class="question-options">
          {options.map(opt => (
            <div
              key={opt.id}
              class={`question-option-static${isSelected(opt.id) ? ' question-option-selected' : ' question-option-dimmed'}`}
            >
              <OptionContent option={opt} multiSelect={!!multiSelect} selected={isSelected(opt.id)} />
            </div>
          ))}
        </div>
      )}
      {customText && customText.length > 0 && (
        <div class="question-freetext">
          <span class="question-freetext-label">Custom answer</span>
          <span class="question-freetext-text">{customText}</span>
        </div>
      )}
      {resolved.kind === 'Canceled' && (
        <button
          type="button"
          class="action-btn action-btn-danger permission-btn-picked question-cancel-picked"
          disabled
        >
          <span class="permission-btn-check" aria-hidden="true">✓ </span>
          Cancel
        </button>
      )}
    </div>
  );
}

/** Dead-question rendering: same layout as the live card so the user can
 *  still read the question, but every option is disabled. Pure render so
 *  tests can walk the vnode tree directly. */
export function TerminatedQuestionBody({
  question,
  options,
  multiSelect,
}: {
  question: string;
  options: QuestionBodyProps['options'];
  multiSelect: boolean | undefined;
}) {
  return (
    <div class="question-body question-body-terminated">
      <QuestionText question={question} />
      {options.length > 0 && (
        <div class="question-options">
          {options.map(opt => (
            <button key={opt.id} type="button" class="question-option" disabled>
              <OptionContent option={opt} multiSelect={!!multiSelect} selected={false} />
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
