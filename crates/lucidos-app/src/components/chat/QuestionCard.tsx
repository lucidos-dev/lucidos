import { signal, useSignal } from '@preact/signals';
import { useEffect, useMemo } from 'preact/hooks';
import { showToast } from '../../store/store';
import { answerThreadQuestion } from '../../store/actions/chat-claude-code';
import { createTapGate } from '../../utils/tapGesture';
import { renderMarkdownInline, renderMarkdownInlineWithLinks } from '../../utils/renderMarkdown';
import { preserveAtBottom } from './scrollState';

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
 *  state from module-level signals. */
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
    return <AnsweredBody question={question} options={options} multiSelect={multiSelect} resolved={effective} />;
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
          <div class="question-options">
            {options.map(opt => (
              <OptionButton
                key={opt.id}
                option={opt}
                pressed={selected.includes(opt.id)}
                onActivate={(id) => toggleMultiSelectedId(toolUseId, id)}
              />
            ))}
          </div>
        )}
      </div>
    );
  }

  const onPick = async (optionId: string) => {
    localPending.value = { kind: 'Selected', option_id: optionId };
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
        <div class="question-options">
          {options.map(opt => (
            <OptionButton key={opt.id} option={opt} onActivate={onPick} />
          ))}
        </div>
      )}
      {options.length === 0 && (
        <div class="question-hint">Type your answer in the prompt below.</div>
      )}
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
        preserveAtBottom();
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
 *  styled like the picked permission affordance. Exported for unit tests. */
export function AnsweredBody({
  question,
  options,
  multiSelect,
  resolved,
}: {
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
    <div class="question-body question-body-answered">
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
