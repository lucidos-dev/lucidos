import { useSignal } from '@preact/signals';
import { useMemo } from 'preact/hooks';
import { showToast } from '../../store/store';
import { answerCCQuestion } from '../../store/actions/chat-claude-code';
import { createTapGate } from '../../utils/tapGesture';

export interface QuestionEvent {
  tool_use_id: string;
  question: string;
  options: Array<{ id: string; label: string; description?: string }>;
  resolved?:
    | { kind: 'Selected'; option_id: string }
    | { kind: 'FreeText'; text: string }
    | { kind: 'Canceled' };
}

interface Props {
  threadId: string;
  event: QuestionEvent;
}

/** Interactive `AskUserQuestion` card. The `pending` signal is an optimistic
 *  override — replaced by `event.resolved` once the SSE roundtrip lands. */
export function QuestionCard({ threadId, event }: Props) {
  const pending = useSignal<string | null>(null);

  const resolved = event.resolved;

  if (resolved) {
    return <AnsweredCard event={event} resolved={resolved} />;
  }

  if (pending.value !== null) {
    return <AnsweredCard event={event} resolved={{ kind: 'Selected', option_id: pending.value }} optimistic />;
  }

  const onPick = async (optionId: string) => {
    pending.value = optionId;
    const ok = await answerCCQuestion(threadId, event.tool_use_id, { kind: 'Selected', option_id: optionId });
    if (!ok) {
      pending.value = null;
      showToast('Could not send answer — please try again.', 'error');
    }
  };

  return (
    <div class="cc-question-card" data-tool-use-id={event.tool_use_id}>
      <div class="cc-question-text">{event.question}</div>
      {event.options.length > 0 && (
        <div class="cc-question-options">
          {event.options.map(opt => (
            <OptionButton key={opt.id} option={opt} onPick={onPick} />
          ))}
        </div>
      )}
      {event.options.length === 0 && (
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

function AnsweredCard({
  event,
  resolved,
  optimistic = false,
}: {
  event: QuestionEvent;
  resolved: NonNullable<QuestionEvent['resolved']>;
  optimistic?: boolean;
}) {
  const summary = resolvedSummary(event, resolved);
  return (
    <div class={`cc-question-card cc-question-card-answered${optimistic ? ' cc-question-card-pending' : ''}`}>
      <div class="cc-question-text">{event.question}</div>
      <div class="cc-question-answer">{summary}</div>
    </div>
  );
}

function resolvedSummary(event: QuestionEvent, resolved: NonNullable<QuestionEvent['resolved']>): string {
  switch (resolved.kind) {
    case 'Selected': {
      const opt = event.options.find(o => o.id === resolved.option_id);
      return `Answered: ${opt?.label ?? resolved.option_id}`;
    }
    case 'FreeText':
      return `Answered: ${resolved.text}`;
    case 'Canceled':
      return 'Canceled';
  }
}
