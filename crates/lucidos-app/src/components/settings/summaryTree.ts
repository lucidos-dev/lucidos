import { ENTRY_KINDS } from '@lucidos/engine-constants';
import type { RecallLine } from '../../api/types';
import { formatShortDateWithYear, formatShortTime, isSameDayInUserTz } from '../../utils/formatTime';
import { rangeLabel } from '../../utils/rangeLabel';

export function lineSpan(id: string): number | null {
  const match = /\/\d+\+(\d+)$/.exec(id);
  return match ? Number(match[1]) : null;
}

export function pendingNote(text: string): string {
  const count = Number(/^(\d+)/.exec(text)?.[1]);
  if (!Number.isFinite(count)) return text;
  return count === 1
    ? '1 newer entry is not summarised yet'
    : `${count.toLocaleString()} newer entries are not summarised yet`;
}

export function entriesLabel(count: number): string {
  return `${count.toLocaleString()} ${count === 1 ? 'entry' : 'entries'}`;
}

export type EntryKind = typeof ENTRY_KINDS[number];

export const ENTRY_KIND_LABELS: Record<EntryKind, string> = {
  'prompt (human)': 'Human',
  'prompt (agent)': 'Agent',
  'prompt (engine)': 'Engine',
  response: 'Agent',
  call: 'Call',
  result: 'Result',
  report: 'Report',
  turn: 'Turn',
  artifact: 'Artifact',
};

const ENTRY_KIND_PREFIX = new RegExp(
  `^(${ENTRY_KINDS.map((k) => k.replace(/[()]/g, '\\$&')).join('|')}):\\s*`,
);

export function entryKindOf(text: string): { kind: EntryKind | null; text: string } {
  const match = ENTRY_KIND_PREFIX.exec(text);
  if (!match) return { kind: null, text };
  return { kind: match[1] as EntryKind, text: text.slice(match[0].length) };
}

export function isSourceText(id: string, lines: RecallLine[]): boolean {
  return lines.length === 1 && lines[0].id === id;
}

export function dateRangeLabel(from: string, to: string): string {
  const start = new Date(from);
  const end = new Date(to);
  if (!isSameDayInUserTz(start, end)) {
    return rangeLabel(formatShortDateWithYear(start), formatShortDateWithYear(end));
  }
  const day = formatShortDateWithYear(start);
  const [first, last] = [formatShortTime(start), formatShortTime(end)];
  return `${day}, ${first === last ? first : rangeLabel(first, last)}`;
}
