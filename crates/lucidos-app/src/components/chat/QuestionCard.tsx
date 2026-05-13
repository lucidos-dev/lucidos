import { signal, useSignal } from '@preact/signals';
import { useEffect, useMemo } from 'preact/hooks';
import { showToast } from '../../store/store';
import { answerCCQuestion } from '../../store/actions/chat-claude-code';
import { createTapGate } from '../../utils/tapGesture';
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

export function toggleMultiSelectedId(toolUseId: string, optionId: string): void {
  const current = getMultiSelectedIds(toolUseId);
  setMultiSelectedIds(
    toolUseId,
    current.includes(optionId) ? current.filter(x => x !== optionId) : [...current, optionId],
  );
}

export function clearMultiSelected(toolUseId: string): void {
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

/** Body of an `AskUserQuestion` divider exchange — rendered inside the
 *  initiator panel which provides the chrome (border, header, timestamp).
 *  Multi-select Submit lives in the prompt action row (PromptInput.tsx); the
 *  card just renders toggleable options and reads its optimistic / resolved
 *  state from module-level signals. */
export function QuestionBody({ threadId, toolUseId, question, options, multiSelect, resolved }: QuestionBodyProps) {
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
    return <AnsweredBody question={question} options={options} resolved={effective} />;
  }

  if (multiSelect) {
    const selected = multiSelectedByToolUse.value.get(toolUseId) ?? [];
    return (
      <div class="cc-question-body" data-tool-use-id={toolUseId}>
        <div class="cc-question-text">{question}</div>
        {options.length > 0 && (
          <div class="cc-question-options">
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
    const ok = await answerCCQuestion(threadId, toolUseId, { kind: 'Selected', option_id: optionId });
    if (!ok) {
      localPending.value = null;
      showToast('Could not send answer — please try again.', 'error');
    }
  };

  return (
    <div class="cc-question-body" data-tool-use-id={toolUseId}>
      <div class="cc-question-text">{question}</div>
      {options.length > 0 && (
        <div class="cc-question-options">
          {options.map(opt => (
            <OptionButton key={opt.id} option={opt} onActivate={onPick} />
          ))}
        </div>
      )}
      {options.length === 0 && (
        <div class="cc-question-hint">Type your answer in the prompt below.</div>
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
      class="cc-question-option"
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
      <span class="cc-question-option-label">{option.label}</span>
      {option.description && <span class="cc-question-option-desc">{option.description}</span>}
    </button>
  );
}

/** Resolved-state rendering: options dim, the picked one is highlighted; the
 *  Custom-answer block surfaces freetext (FreeText answers, or the freetext
 *  typed alongside a MultiSelected); Canceled shows dimmed options + a badge. */
function AnsweredBody({
  question,
  options,
  resolved,
}: {
  question: string;
  options: QuestionBodyProps['options'];
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
    <div class="cc-question-body cc-question-body-answered">
      <div class="cc-question-text">{question}</div>
      {options.length > 0 && (
        <div class="cc-question-options">
          {options.map(opt => (
            <div
              key={opt.id}
              class={`cc-question-option-static${isSelected(opt.id) ? ' cc-question-option-selected' : ' cc-question-option-dimmed'}`}
            >
              <span class="cc-question-option-label">{opt.label}</span>
              {opt.description && <span class="cc-question-option-desc">{opt.description}</span>}
            </div>
          ))}
        </div>
      )}
      {customText && customText.length > 0 && (
        <div class="cc-question-freetext">
          <span class="cc-question-freetext-label">Custom answer</span>
          <span class="cc-question-freetext-text">{customText}</span>
        </div>
      )}
      {resolved.kind === 'Canceled' && (
        <div class="cc-question-canceled-badge">Canceled</div>
      )}
    </div>
  );
}
