import type { ContextCapture } from '../../store/types';

export type TokenScale = (chars: number) => number;

export function headlineTokens(snap: ContextCapture): number {
  return snap.usage?.input_tokens ?? snap.estimated_total_tokens;
}

export function sectionTokenScale(snap: ContextCapture): TokenScale {
  const headline = headlineTokens(snap);
  const totalChars = snap.sections.reduce((a, s) => a + s.budget_delta_chars, 0);
  if (totalChars <= 0 || !Number.isFinite(headline) || headline <= 0) return () => 0;
  return chars => Math.round((chars / totalChars) * headline);
}
