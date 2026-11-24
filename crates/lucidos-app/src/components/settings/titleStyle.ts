export type TitleStyleCommit =
  | { kind: 'none' }
  | { kind: 'save'; value: string }
  | { kind: 'clear' }
  | { kind: 'too-long' };

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
