import { signal } from '@preact/signals';
import { useEffect, useMemo, useRef } from 'preact/hooks';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { answerThreadQuestion } from '../../store/actions/chat-claude-code';
import { createTapGate } from '../../utils/tapGesture';
import { pendingDecisions } from './pendingDecisions';
import { renderMarkdown, renderMarkdownInline } from '../../utils/renderMarkdown';
import { CHOICE_CARD_ROLE, handAnsweredCardFocusToPrompt, handleChoiceCardKeyDown, seedChoiceCardFocus } from './choiceCardNav';
import { UserImages } from './chat-exchange-parts';
import { followAnsweredQuestion } from './scrollState';
import type { AnswerKind, QuestionOption } from '../../store/thread-events';

/** Local name for the wire `AnswerKind`. Aliased rather than restated, because
 *  this card renders every kind the engine can persist. A second copy of the
 *  union is a variant waiting to be added in one place and missed here. */
export type ResolvedAnswer = AnswerKind;

export interface QuestionBodyProps {
  threadId: string;
  toolUseId: string;
  question: string;
  options: QuestionOption[];
  multiSelect?: boolean;
  resolved?: ResolvedAnswer;
  /** Surrounding response was canceled / aborted / failed / superseded
   *  without an answer landing — render every option disabled. */
  terminated?: boolean;
}

// Selections and optimistic answers live at module level. PromptInput reads
// the selections and writes a multi-select answer, and the divider header reads
// the answers to say "Sending". The QuestionBody useEffect below drains both
// when the persisted UserQuestionAnswered lands.
export const multiSelectedByToolUse = signal<Map<string, string[]>>(new Map());
export const pendingAnswers = pendingDecisions<ResolvedAnswer>();

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
  sending = false,
}: {
  option: QuestionOption;
  multiSelect: boolean;
  selected: boolean;
  /** The pick is on its way to the engine: the indicator spins in its place. */
  sending?: boolean;
}) {
  return (
    <>
      {selected && sending
        ? <span class="mini-spinner question-option-sending" aria-hidden="true" />
        : <OptionIndicator multiSelect={multiSelect} selected={selected} />}
      <span class="question-option-label">{option.label}</span>
      {option.description && (
        <span
          class="question-option-desc"
          dangerouslySetInnerHTML={{ __html: renderMarkdownInline(option.description) }}
        />
      )}
      {/* Inline markdown only: the option is a <button>, so no link may
          appear inside it. */}
      {option.preview && (
        <span
          class="question-option-preview"
          dangerouslySetInnerHTML={{ __html: renderMarkdownInline(option.preview) }}
        />
      )}
    </>
  );
}

/** The question prompt itself, rendered as full markdown like a reply. A card
 *  often carries findings before the decision, and only block markdown keeps
 *  their paragraphs and lists apart. It sits in a plain `<div>`, not a
 *  `<button>`, so links are valid here, unlike inside OptionButton. They route
 *  like a reply's: workspace links through the initiator panel's body click,
 *  web links through the global click handler. Shared across the live, answered
 *  and terminated bodies so all three render the question identically. */
function QuestionText({ question }: { question: string }) {
  return (
    <div
      class="question-text markdown-content"
      dangerouslySetInnerHTML={{ __html: renderMarkdown(question) }}
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
  // Drain the module-level maps once the persisted answer lands. Without this,
  // selections + optimistic pending leak across the session.
  useEffect(() => {
    if (!resolved) return;
    pendingAnswers.clear(toolUseId);
    clearMultiSelected(toolUseId);
  }, [resolved, toolUseId]);

  // A dead card draws no unconfirmed pick, matching its "Unanswered" header.
  // The pick stays stored, so an answer that still lands shows it again.
  const pending = terminated ? undefined : pendingAnswers.map.value.get(toolUseId);
  const effective = resolved ?? pending;
  const sending = useDelayedFlag(!resolved && !!pending);
  if (effective) {
    return <AnsweredBody toolUseId={toolUseId} question={question} options={options} multiSelect={multiSelect} resolved={effective} sending={sending} />;
  }
  if (terminated) {
    return <TerminatedQuestionBody question={question} options={options} multiSelect={multiSelect} />;
  }

  if (multiSelect) {
    const selected = multiSelectedByToolUse.value.get(toolUseId) ?? [];
    return (
      <div class="question-body protected-surface" data-tool-use-id={toolUseId}>
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
    handAnsweredCardFocusToPrompt();
    pendingAnswers.set(toolUseId, { kind: 'Selected', option_id: optionId });
    // Answering is a send: keep the reader at the live edge while the agent
    // resumes, landing on what they just answered when they were not already
    // riding it. Before the await, because the scroll is the composer's-tap half
    // of the action and must not wait on the round trip. See
    // `followAnsweredQuestion`.
    followAnsweredQuestion(toolUseId);
    const ok = await answerThreadQuestion(threadId, toolUseId, { kind: 'Selected', option_id: optionId });
    // Roll the optimistic pick back so the card goes live again. The action
    // owns the message: a second toast here made one failed tap say two things,
    // neither of them the cause. See `answerFailureMessage`.
    if (!ok) pendingAnswers.clear(toolUseId);
  };

  return (
    <div class="question-body protected-surface" data-tool-use-id={toolUseId}>
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
  option: QuestionOption;
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
      onPointerDown={e => gate.down(e)}
      onPointerMove={e => gate.move(e)}
      onPointerCancel={() => gate.cancel()}
      onClick={(e) => {
        if (!gate.isTap()) return;
        // A tap on a picture opens the image viewer (the global click
        // handler), and dragging its scrollbar ends in a click on the
        // wrapper. Neither is a choice.
        if (e.target instanceof Element && e.target.closest('.image-scroll-wrapper')) return;
        onActivate(option.id);
      }}
      aria-label={`${isToggle ? 'Toggle' : 'Answer'}: ${option.label}`}
    >
      <OptionContent option={option} multiSelect={isToggle} selected={!!pressed} />
    </button>
  );
}

/** Resolved-state rendering: options dim, the picked one is highlighted; the
 *  typed-answer block surfaces freetext (FreeText answers, or the freetext
 *  typed alongside a MultiSelected); Canceled renders a disabled Cancel button
 *  styled like the picked permission affordance. Exported for unit tests.
 *
 *  Carries `data-tool-use-id` like the live body it replaces, so the answer's
 *  landing glide finds the card whether or not this swap has already happened.
 *  That lookup is `landsOnCard` in scrollState, over the `cardTurn` matcher it
 *  shares with the permission cards. The two submit sites resolve it
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
  sending = false,
}: {
  toolUseId: string;
  question: string;
  options: QuestionBodyProps['options'];
  multiSelect: boolean | undefined;
  resolved: ResolvedAnswer;
  sending?: boolean;
}) {
  const isSelected = (id: string) =>
    (resolved.kind === 'Selected' && resolved.option_id === id) ||
    (resolved.kind === 'MultiSelected' && resolved.option_ids.includes(id));
  const customText =
    resolved.kind === 'FreeText' ? resolved.text
    : resolved.kind === 'MultiSelected' ? resolved.text
    : undefined;
  const imageHashes =
    resolved.kind === 'FreeText' || resolved.kind === 'MultiSelected' ? resolved.image_hashes ?? [] : [];
  return (
    <div class="question-body question-body-answered protected-surface" data-tool-use-id={toolUseId}>
      <QuestionText question={question} />
      {options.length > 0 && (
        <div class="question-options">
          {options.map(opt => (
            <div
              key={opt.id}
              class={`question-option-static${isSelected(opt.id) ? ' question-option-selected' : ' question-option-dimmed'}`}
            >
              <OptionContent option={opt} multiSelect={!!multiSelect} selected={isSelected(opt.id)} sending={sending} />
            </div>
          ))}
        </div>
      )}
      {((customText && customText.length > 0) || imageHashes.length > 0) && (
        <div class="question-freetext">
          <span class="question-freetext-label">Your answer</span>
          <div class="user-bubble question-freetext-text">
            {customText}
            <UserImages imageHashes={imageHashes} />
          </div>
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
      {/* Not a button, because nobody chose it. The user replied with
          something else and the engine closed the question for them, so the
          card says what happened rather than offering an affordance. */}
      {resolved.kind === 'Superseded' && (
        <span class="question-superseded-note">
          Replaced by your next message
        </span>
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
    <div class="question-body question-body-terminated protected-surface">
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
