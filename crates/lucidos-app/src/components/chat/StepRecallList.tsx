import { stepDetailModal } from '../../store/store';
import { showEventWhereItLives } from '../../store/actions/event-navigation';
import { openFilePreview } from '../../store/actions/artifacts';
import { formatShortDateWithYear } from '../../utils/formatTime';
import type { MemorySource, StepRecall } from '../../store/types';

/** Close the step detail and go where a recalled memory came from. Both paths
 *  land or say why: `showEventWhereItLives` toasts an event it cannot reach. */
export function openRecalledSource(source: MemorySource): void {
  stepDetailModal.value = null;
  if (source.type === 'event') void showEventWhereItLives(source.id);
  else openFilePreview(`artifacts/${source.path}`);
}

function sourceLabel(source: MemorySource): string {
  return source.type === 'event' ? 'Open the conversation' : `Open ${source.path}`;
}

/** The memories a pre-turn recall put in front of the model, each a link to
 *  its source, then the queries that found them. */
export function StepRecallList({ recall }: { recall: StepRecall }) {
  return (
    <>
      {recall.memories.length > 0 && (
        <ul class="step-recall-list" data-role="recalled-memories">
          {recall.memories.map((m) => (
            <li key={m.id}>
              <button
                type="button"
                class="step-recall-item"
                aria-label={`${m.summary}. ${sourceLabel(m.source)}`}
                onClick={() => openRecalledSource(m.source)}
              >
                <span class="step-recall-meta">
                  {formatShortDateWithYear(new Date(m.src_created_at))} · {m.topic}
                </span>
                <span class="step-recall-summary">{m.summary}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {recall.queries.length > 0 && (
        <>
          <div class="step-detail-section-label">Searched for</div>
          <ul class="step-recall-queries" data-role="recall-queries">
            {recall.queries.map((q, i) => <li key={i}>{q}</li>)}
          </ul>
        </>
      )}
    </>
  );
}
