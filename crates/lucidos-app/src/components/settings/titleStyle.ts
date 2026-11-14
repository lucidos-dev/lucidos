/** What committing the Title style field does, given what is typed and what is
 *  stored. Blank clears: the engine refuses an empty write, and "no style" is
 *  a deleted row. */
export type TitleStyleCommit =
  | { kind: 'none' }
  | { kind: 'save'; value: string }
  | { kind: 'clear' }
  | { kind: 'too-long' };

/** The field's text on one line. A title style is one instruction, and the
 *  textarea, unlike a text input, keeps the line breaks of a paste. */
export function titleStyleOneLine(text: string): string {
  return text.replace(/\s*[\r\n]+\s*/g, ' ');
}

export function titleStyleCommit(draft: string, stored: string, maxChars: number): TitleStyleCommit {
  const typed = draft.trim();
  if (typed === stored.trim()) return { kind: 'none' };
  if (typed === '') return { kind: 'clear' };
  if ([...typed].length > maxChars) return { kind: 'too-long' };
  return { kind: 'save', value: typed };
}
