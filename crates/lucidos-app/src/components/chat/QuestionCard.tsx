import { useSignal } from '@preact/signals';
import { useMemo } from 'preact/hooks';
import { showToast } from '../../store/store';
import { answerCCQuestion } from '../../store/actions/chat-claude-code';
import { createTapGate } from '../../utils/tapGesture';

export interface QuestionBodyProps {
  threadId: string;
  toolUseId: string;
  question: string;
  options: Array<{ id: string; label: string; description?: string }>;
  resolved?:
    | { kind: 'Selected'; option_id: string }
    | { kind: 'FreeText'; text: string }
    | { kind: 'Canceled' };
}

/** Body of an `AskUserQuestion` divider exchange — rendered inside the
 *  initiator panel which provides the chrome (border, header, timestamp).
 *  The `pending` signal is an optimistic override — replaced by `resolved`
 *  once the SSE roundtrip lands. */
export function QuestionBody({ threadId, toolUseId, question, options, resolved }: QuestionBodyProps) {
  const pending = useSignal<string | null>(null);

  const effective: QuestionBodyProps['resolved'] = resolved
    ?? (pending.value !== null ? { kind: 'Selected', option_id: pending.value } : undefined);

  if (effective) {
    return <AnsweredBody question={question} options={options} resolved={effective} />;
  }

  const onPick = async (optionId: string) => {
    pending.value = optionId;
    const ok = await answerCCQuestion(threadId, toolUseId, { kind: 'Selected', option_id: optionId });
    if (!ok) {
      pending.value = null;
      showToast('Could not send answer — please try again.', 'error');
    }
  };

  return (
    <div class="cc-question-body" data-tool-use-id={toolUseId}>
      <div class="cc-question-text">{question}</div>
      {options.length > 0 && (
        <div class="cc-question-options">
          {options.map(opt => (
            <OptionButton key={opt.id} option={opt} onPick={onPick} />
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
 *  which silently dispatches the answer to CC and resumes the session. */
function OptionButton({
  option,
  onPick,
}: {
  option: { id: string; label: string; description?: string };
  onPick: (id: string) => void;
}) {
  const gate = useMemo(() => createTapGate(), []);
  return (
    <button
      type="button"
      class="cc-question-option"
      onPointerDown={e => gate.down(e.clientX, e.clientY)}
      onPointerMove={e => gate.move(e.clientX, e.clientY)}
      onPointerCancel={() => gate.cancel()}
      onClick={() => {
        if (gate.isTap()) onPick(option.id);
      }}
      aria-label={`Answer: ${option.label}`}
    >
      <span class="cc-question-option-label">{option.label}</span>
      {option.description && <span class="cc-question-option-desc">{option.description}</span>}
    </button>
  );
}

/** Resolved-state rendering: options dim, the picked one (Selected) is
 *  highlighted; FreeText answers show all options dimmed and a highlighted
 *  "Custom answer: …" block; Canceled shows dimmed options + a "Canceled" badge. */
function AnsweredBody({
  question,
  options,
  resolved,
}: {
  question: string;
  options: QuestionBodyProps['options'];
  resolved: NonNullable<QuestionBodyProps['resolved']>;
}) {
  const isSelected = (id: string) => resolved.kind === 'Selected' && resolved.option_id === id;
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
      {resolved.kind === 'FreeText' && (
        <div class="cc-question-freetext">
          <span class="cc-question-freetext-label">Custom answer</span>
          <span class="cc-question-freetext-text">{resolved.text}</span>
        </div>
      )}
      {resolved.kind === 'Canceled' && (
        <div class="cc-question-canceled-badge">Canceled</div>
      )}
    </div>
  );
}
