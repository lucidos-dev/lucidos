import type { AnswerKind, QuestionOption, ThreadState } from './thread-events';

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

export function askedOptions(thread: ThreadState | undefined, toolUseId: string): readonly QuestionOption[] {
  for (const event of thread?.events.values() ?? []) {
    if (event.type === 'UserQuestionAsked' && event.tool_use_id === toolUseId) return event.options ?? [];
  }
  return [];
}

export function questionAnswerText(answer: AnswerKind, options: readonly QuestionOption[]): string {
  const ids = answer.kind === 'MultiSelected' ? answer.option_ids : answer.kind === 'Selected' ? [answer.option_id] : [];
  const labels = ids.map(id => options.find(option => option.id === id)?.label ?? id);
  return [...labels, ...('text' in answer && answer.text ? [answer.text] : [])].join('\n\n');
}
