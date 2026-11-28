import { PREF_RESPONSE_STYLE } from '@lucidos/preference-catalog';
import type { ResponseStyle } from '../../api/types';


export const STANDARD_ID = PREF_RESPONSE_STYLE.fallback;

export const MAX_ID_CHARS = 40;
export const MAX_LABEL_CHARS = 40;
export const MAX_INSTRUCTION_CHARS = 1000;
export const MAX_STYLES = 20;

export interface StyleEntry {
  id: string;
  label: string;
  instruction: string;
}

export function isValidStyleId(id: string): boolean {
  return Boolean(id) && [...id].length <= MAX_ID_CHARS && slugify(id) === id;
}

function slugify(text: string): string {
  let out = '';
  let prevDash = true;
  for (const ch of text.normalize('NFKD').replace(/\p{M}/gu, '')) {
    if (/[a-z0-9]/i.test(ch)) {
      out += ch.toLowerCase();
      prevDash = false;
    } else if (!prevDash) {
      out += '-';
      prevDash = true;
    }
  }
  return out.replace(/^-+|-+$/g, '');
}

export function nextStyleId(label: string, taken: readonly string[]): string {
  const base = [...slugify(label)].slice(0, MAX_ID_CHARS).join('').replace(/-+$/g, '');
  if (base && !taken.includes(base)) return base;
  const stem = base || 'style';
  for (let n = 2; ; n++) {
    const suffix = `-${n}`;
    const room = MAX_ID_CHARS - suffix.length;
    const candidate = `${[...stem].slice(0, room).join('').replace(/-+$/g, '')}${suffix}`;
    if (!taken.includes(candidate)) return candidate;
  }
}

export function describeStyleProblem(
  draft: { id: string; label: string; instruction: string },
  existingIds: readonly string[],
  nextDocumentSize = 0,
): string | null {
  if (draft.id === STANDARD_ID) return 'Standard is the off switch and cannot be edited.';
  if (!isValidStyleId(draft.id)) {
    return `An id is lowercase letters, digits and dashes, up to ${MAX_ID_CHARS} characters.`;
  }
  if (existingIds.includes(draft.id)) return `A style called "${draft.id}" already exists.`;
  const label = draft.label.trim();
  if (!label) return 'Give the style a name.';
  if ([...label].length > MAX_LABEL_CHARS) {
    return `A name is at most ${MAX_LABEL_CHARS} characters.`;
  }
  const instruction = draft.instruction.trim();
  if (!instruction) return 'Write how answers should read in this style.';
  if ([...instruction].length > MAX_INSTRUCTION_CHARS) {
    return `Instructions are at most ${MAX_INSTRUCTION_CHARS} characters.`;
  }
  if (nextDocumentSize > MAX_STYLES) {
    return `Saving this would store ${nextDocumentSize} styles, and the limit is ${MAX_STYLES}. Delete one first.`;
  }
  return null;
}

function storedRows(library: readonly ResponseStyle[]): StyleEntry[] {
  return library
    .filter((s) => s.id !== STANDARD_ID && s.source !== 'builtin')
    .map((s) => ({ id: s.id, label: s.label, instruction: s.instruction }));
}

export function documentWithout(
  library: readonly ResponseStyle[],
  id: string,
): StyleEntry[] {
  return storedRows(library).filter((s) => s.id !== id);
}

export function documentWithEdit(
  library: readonly ResponseStyle[],
  edit: StyleEntry,
): StyleEntry[] {
  const entry = {
    id: edit.id,
    label: edit.label.trim(),
    instruction: edit.instruction.trim(),
  };
  const rows = storedRows(library);
  const at = rows.findIndex((s) => s.id === edit.id);
  if (at === -1) return [...rows, entry];
  return rows.map((row, i) => (i === at ? entry : row));
}

export function libraryIsFull(library: readonly ResponseStyle[]): boolean {
  return storedRows(library).length >= MAX_STYLES;
}
