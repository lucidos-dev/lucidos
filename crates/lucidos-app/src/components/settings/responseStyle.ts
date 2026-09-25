import type { ResponseStyle } from '../../api/types';

/** The off switch's id, mirroring `response_style::STANDARD_ID`. The one id
 *  worth spelling out here: the editor has to know which row draws no buttons,
 *  and the picker has to know which one costs nothing. */
export const STANDARD_ID = 'standard';

/** Bounds, mirroring `core/response_style.rs`. The engine refuses a document
 *  that breaks one. These exist to say so BEFORE the user loses a paragraph to
 *  a toast, never instead of the engine's own check.
 *
 *  `responseStyle.mirror.test.ts` reads the Rust source and fails if either
 *  side moves alone. */
export const MAX_ID_CHARS = 40;
export const MAX_LABEL_CHARS = 40;
export const MAX_INSTRUCTION_CHARS = 1000;
export const MAX_STYLES = 20;

/** One entry of the stored `response_styles` document. The library the editor
 *  renders is NOT this: the engine merges these over what it ships, and only
 *  the engine holds the shipped text. */
export interface StyleEntry {
  id: string;
  label: string;
  instruction: string;
}

/** Whether an id is one the engine will accept: bounded, and already what
 *  `slugify` would produce. Mirrors `is_valid_style_id`, which expresses the
 *  same contract as a round trip through `core::slug`. */
export function isValidStyleId(id: string): boolean {
  return Boolean(id) && [...id].length <= MAX_ID_CHARS && slugify(id) === id;
}

/** Fold any text to the engine's kebab-case charset.
 *
 *  NFKD first, then drop combining marks, so an accented name keeps its letters:
 *  "Café report" becomes `cafe-report` rather than `caf-report`. That matches
 *  `core::slug::slugify_kebab`, which normalises the same way. */
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

/** An id for a NEW style, derived from its name and unique against `taken`.
 *
 *  A name in a script the engine's charset cannot hold folds to nothing, and
 *  Lucidos runs in whatever language the `language` preference names. So a
 *  Japanese or Cyrillic name takes a numbered id instead. Without that it
 *  leaves Save dead, over an id the Add form never shows. */
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

/** Why the engine would refuse this edit, or `null` when it would take it.
 *
 *  Checked here so Save can be disabled with the reason on screen. The engine
 *  checks the same things on its one write chokepoint, and that is the check
 *  that counts: this one is courtesy, and it must never be the only one.
 *
 *  `nextDocumentSize` is how many entries the save would store. That bound
 *  belongs to the DOCUMENT rather than to the draft. Overriding an untouched
 *  shipped style grows the document by one, so a full library can refuse an
 *  edit that adds no style of its own. */
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

/** Every row the stored document holds: the user's own styles, plus any
 *  shipped style they have overridden. An untouched shipped row contributes
 *  nothing, which is what lets a later reword of the shipped text still reach
 *  whoever left it alone. */
function storedRows(library: readonly ResponseStyle[]): StyleEntry[] {
  return library
    .filter((s) => s.id !== STANDARD_ID && s.source !== 'builtin')
    .map((s) => ({ id: s.id, label: s.label, instruction: s.instruction }));
}

/** The document with one style's own entry dropped.
 *
 *  It is both buttons. On a shipped style the entry is the override, so
 *  dropping it is Reset and the shipped text comes back from the engine. On a
 *  user style it is the style, so dropping it is Delete. */
export function documentWithout(
  library: readonly ResponseStyle[],
  id: string,
): StyleEntry[] {
  return storedRows(library).filter((s) => s.id !== id);
}

/** The stored document rebuilt from the merged library, with one style's text
 *  replaced.
 *
 *  Derived from the library rather than patched into the old document, so the
 *  two can never disagree about what is saved. An entry that already exists is
 *  replaced IN PLACE, because document order is picker order for a user's own
 *  styles. Appending would move a style to the bottom on every edit. */
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

/** Whether the library is already at the ceiling, so Add is withheld rather
 *  than offered and then refused. */
export function libraryIsFull(library: readonly ResponseStyle[]): boolean {
  return storedRows(library).length >= MAX_STYLES;
}
