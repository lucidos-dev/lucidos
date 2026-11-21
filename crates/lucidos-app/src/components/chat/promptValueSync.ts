import { signal } from '@preact/signals';

export type PromptCaretSide = 'start' | 'end';

/** One-shot "place the caret on the next sync" ticket. A programmatic write
 *  that REPLACES the draft, such as a seeded suggestion, leaves the old caret
 *  offset indexing characters that are gone. Restoring it would drop the user
 *  mid-sentence in copy they did not write. An append leaves the prefix intact,
 *  so it keeps the caret and needs no ticket.
 *
 *  `seq` is a plain monotonic counter, so a repeat replace with the same text
 *  still places the caret. */
export const promptCaretRequest = signal<{ seq: number; side: PromptCaretSide }>({ seq: 0, side: 'end' });

/** Ask the composer to put the caret at `side` when it next shows the draft.
 *  Call AFTER writing the draft, so the sync observes both in one render. */
export function requestPromptCaret(side: PromptCaretSide): void {
  promptCaretRequest.value = { seq: promptCaretRequest.peek().seq + 1, side };
}

export function requestPromptCaretToEnd(): void {
  requestPromptCaret('end');
}

/** Put a collapsed caret at the start or end of the box, scrolled into view. */
export function placeTextareaCaret(el: HTMLTextAreaElement, side: PromptCaretSide): void {
  const at = side === 'start' ? 0 : el.value.length;
  el.setSelectionRange(at, at);
  el.scrollTop = side === 'start' ? 0 : el.scrollHeight;
}

/** Sync a textarea's value to `text`. Returns true when the DOM changed.
 *
 *  Browsers snap selectionStart/End to the end of the new value on every
 *  `el.value =` reassignment. With `preserveCursor` we read selection before
 *  the write and restore it (clamped to text.length) after. Pass `false` on
 *  thread switch — the previous offset is meaningless on the new text. */
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
