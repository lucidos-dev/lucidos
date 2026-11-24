import { signal } from '@preact/signals';

export type PromptCaretSide = 'start' | 'end';

export const promptCaretRequest = signal<{ seq: number; side: PromptCaretSide }>({ seq: 0, side: 'end' });

export function requestPromptCaret(side: PromptCaretSide): void {
  promptCaretRequest.value = { seq: promptCaretRequest.peek().seq + 1, side };
}

export function requestPromptCaretToEnd(): void {
  requestPromptCaret('end');
}

export function placeTextareaCaret(el: HTMLTextAreaElement, side: PromptCaretSide): void {
  const at = side === 'start' ? 0 : el.value.length;
  el.setSelectionRange(at, at);
  el.scrollTop = side === 'start' ? 0 : el.scrollHeight;
}

export function syncTextareaValue(
  el: HTMLTextAreaElement,
  text: string,
  preserveCursor: boolean,
): boolean {
  if (el.value === text) return false;
  const start = preserveCursor ? el.selectionStart : null;
  const end = preserveCursor ? el.selectionEnd : null;
  el.value = text;
  if (start !== null && end !== null) {
    el.setSelectionRange(
      Math.min(start, text.length),
      Math.min(end, text.length),
    );
  }
  return true;
}
