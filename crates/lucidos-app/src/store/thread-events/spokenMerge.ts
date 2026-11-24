
export const MERGE_GAP_SECS = 5;

const CLITIC_OPENERS = new Set([
  '.', ',', '!', '?', ';', ':', '…', ')', ']', '}', '%', "'", '’', '"', '”',
]);

export function isOneUtterance(gapSecs: number, sameSpeaker: boolean): boolean {
  return sameSpeaker && gapSecs >= 0 && gapSecs <= MERGE_GAP_SECS;
}

export function joinSpoken(first: string, second: string): string {
  const a = first.trim();
  const b = second.trim();
  if (!a) return b;
  if (!b) return a;
  return CLITIC_OPENERS.has(b[0]) ? `${a}${b}` : `${a} ${b}`;
}
