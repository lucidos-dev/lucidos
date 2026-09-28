import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { dismissSideQuestion, sideQuestions, sideQuestionsFor, type SideQuestion } from '../../store/sideQuestions';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { CloseIcon } from '../shared/icons';
import { MarkdownBlock } from './chat-exchange-parts';

/** Said on every card, so nobody mistakes the answer for part of the thread. */
export const SIDE_QUESTION_NOTE = 'Not added to the conversation';

/** How long a pending card waits before it says "Thinking". A live answer
 *  often lands sooner, and a flash of the word reads as a glitch. */
export const SIDE_QUESTION_THINKING_DELAY_MS = 600;

function Thinking() {
  const shown = useDelayedFlag(true, SIDE_QUESTION_THINKING_DELAY_MS);
  return (
    <div class="side-question-thinking" aria-live="polite">
      {shown ? 'Thinking…' : null}
    </div>
  );
}

export function SideQuestionCard({ item }: { item: SideQuestion }) {
  return (
    <div class="side-question-card" data-role="side-question-card" data-status={item.status}>
      <div class="side-question-head">
        <span class="side-question-label">Side question</span>
        <span class="side-question-note">{SIDE_QUESTION_NOTE}</span>
        <button
          class="icon-btn side-question-dismiss"
          onClick={() => dismissSideQuestion(item.id)}
          aria-label="Dismiss side question"
          data-tooltip="Dismiss side question"
        >
          <CloseIcon />
        </button>
      </div>
      <div class="side-question-question">{item.question}</div>
      {item.status === 'pending' && <Thinking />}
      {item.status === 'answered' && <MarkdownBlock html={renderMarkdown(item.answer)} />}
      {item.status === 'failed' && (
        <div class="side-question-error" role="alert">{item.error}</div>
      )}
    </div>
  );
}

/** The thread's side-question cards, closing its feed. Renders nothing when
 *  there are none, so a thread without side questions is unchanged. */
export function SideQuestionCards({ threadId }: { threadId: string }) {
  const items = sideQuestionsFor(sideQuestions.value, threadId);
  if (items.length === 0) return null;
  return (
    <div class="side-questions" data-role="side-questions">
      {items.map((item) => <SideQuestionCard key={item.id} item={item} />)}
    </div>
  );
}
