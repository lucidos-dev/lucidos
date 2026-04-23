import { useSignal } from '@preact/signals';
import { showToast } from '../../store/store';
import { answerCCQuestion } from '../../store/actions/chat-claude-code';

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
            <button
              key={opt.id}
              type="button"
              class="cc-question-option"
              onClick={() => onPick(opt.id)}
              aria-label={`Answer: ${opt.label}`}
            >
              <span class="cc-question-option-label">{opt.label}</span>
              {opt.description && <span class="cc-question-option-desc">{opt.description}</span>}
            </button>
          ))}
        </div>
      )}
      {event.options.length === 0 && (
        <div class="cc-question-hint">Type your answer in the prompt below.</div>
      )}
    </div>
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
