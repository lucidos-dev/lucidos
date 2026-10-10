import type { RecallLine } from '../../api/types';
import { formatShortDateWithYear, formatShortTime, isSameDayInUserTz } from '../../utils/formatTime';

/** How many entries a line covers, read from the `start+span` end of its id.
 *  `null` for an id that names no node, such as the pending line. */
export function lineSpan(id: string): number | null {
  const match = /\/\d+\+(\d+)$/.exec(id);
  return match ? Number(match[1]) : null;
}

/** The pending line in the browser's words. The engine writes it for the
 *  agent, pointing at the `search` recall tool, which the browser lacks. */
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

/** Whether a zoom of `id` came back as the line's source rather than finer
 *  lines. A leaf opens into one line under its own id: the exact message,
 *  or the artifact text. */
export function isSourceText(id: string, lines: RecallLine[]): boolean {
  return lines.length === 1 && lines[0].id === id;
}

/** When a line's entries happened: a moment, hours of one day, or days. */
export function dateRangeLabel(from: string, to: string): string {
  const start = new Date(from);
  const end = new Date(to);
  if (!isSameDayInUserTz(start, end)) {
    return `${formatShortDateWithYear(start)} – ${formatShortDateWithYear(end)}`;
  }
  const day = formatShortDateWithYear(start);
  const [first, last] = [formatShortTime(start), formatShortTime(end)];
  return first === last ? `${day}, ${first}` : `${day}, ${first} – ${last}`;
}
