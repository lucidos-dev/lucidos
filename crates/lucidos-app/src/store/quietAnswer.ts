import type { AnswerKind, QuestionOption, ThreadState } from './thread-events';

/** Whether `answer` picks only *quiet options*, so the turn ends without
 *  waking the agent. Mirrors the engine's `agent_question::answer_is_quiet`,
 *  which stamps the confirmed `UserQuestionAnswered` and has the final say. */
export function answerIsQuiet(answer: AnswerKind, options: readonly QuestionOption[]): boolean {
  const isQuiet = (id: string) => options.some(o => o.id === id && o.quiet === true);
  switch (answer.kind) {
    case 'Selected':
      return isQuiet(answer.option_id);
    case 'MultiSelected':
      return answer.option_ids.length > 0
        && answer.option_ids.every(isQuiet)
        && !answer.text?.trim()
        && !answer.image_hashes?.length;
    default:
      return false;
  }
}

/** The options of the question card `toolUseId` asked on `thread`. */
export function askedOptions(thread: ThreadState | undefined, toolUseId: string): readonly QuestionOption[] {
  for (const event of thread?.events.values() ?? []) {
    if (event.type === 'UserQuestionAsked' && event.tool_use_id === toolUseId) return event.options ?? [];
  }
  return [];
}
