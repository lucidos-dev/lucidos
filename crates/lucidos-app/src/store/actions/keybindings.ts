// Override-aware keybinding layer: merges the user's custom bindings (persisted
// as the workspace `keybindings` preference — a JSON map of shortcutId →
// serialized binding) over the registry defaults. The pure registry + binding
// math live in `utils/shortcuts.ts`; this module adds the stateful read/write
// and the override-aware tooltip helper.

import { preferences } from '../store';
import { savePreference } from './preferences';
import { isMac } from '../../utils/platform';
import {
  SHORTCUT_DEFS,
  shortcutDef,
  serializeBinding,
  parseBinding,
  formatBinding,
  bindingsEqual,
  matchesEvent,
  eventToBinding,
  isBindableChord,
  type Binding,
  type ShortcutId,
} from '../../utils/shortcuts';

/** Workspace preference key holding the JSON override map. Synced across
 *  devices via PreferencesChanged like any other preference. */
export const KEYBINDINGS_PREF_KEY = 'keybindings';

/** Shortcut ids that were renamed after users may have persisted overrides under
 *  the old key. `overrides()` falls back to the legacy key so a rename never
 *  silently drops a user's custom binding; the next `setBinding` rewrites the map
 *  with the current id (legacy keys aren't re-emitted), so the old key fades out
 *  naturally. `previousThread`/`nextThread` became `historyBack`/`historyForward`
 *  when the shortcut went from thread-only to focused-pane-aware. */
const LEGACY_SHORTCUT_IDS: Partial<Record<ShortcutId, string>> = {
  historyBack: 'previousThread',
  historyForward: 'nextThread',
};

type EventLike = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>;

/** Parse the persisted override map from the preferences signal. Tolerant of a
 *  missing/corrupt value or unknown ids — bad data falls back to defaults
 *  rather than throwing. */
function overrides(): Partial<Record<ShortcutId, Binding>> {
  if (preferences.value.status !== 'loaded') return {};
  const raw = preferences.value.data[KEYBINDINGS_PREF_KEY];
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!parsed || typeof parsed !== 'object') return {};
  const map = parsed as Record<string, unknown>;
  const out: Partial<Record<ShortcutId, Binding>> = {};
  for (const def of SHORTCUT_DEFS) {
    const legacyKey = LEGACY_SHORTCUT_IDS[def.id];
    // Prefer the current id; fall back to the pre-rename key so a renamed
    // shortcut keeps the user's custom binding.
    const v = typeof map[def.id] === 'string'
      ? map[def.id]
      : (legacyKey ? map[legacyKey] : undefined);
    if (typeof v === 'string') {
      const b = parseBinding(v);
      if (b) out[def.id] = b;
    }
  }
  return out;
}

/** The current (override-or-default) binding for a shortcut. */
export function bindingFor(id: ShortcutId): Binding {
  return overrides()[id] ?? shortcutDef(id).defaultBinding;
}

/** True when the user has rebound this shortcut away from its default. */
export function isCustomized(id: ShortcutId): boolean {
  const o = overrides()[id];
  return !!o && !bindingsEqual(o, shortcutDef(id).defaultBinding);
}

/** The shortcut whose current binding matches this keydown, or null. `exclude`
 *  skips one id — used by the recorder to detect a conflict with a DIFFERENT
 *  shortcut while ignoring the one being rebound. */
export function matchShortcut(e: EventLike, exclude?: ShortcutId): ShortcutId | null {
  // Parse the override map once per event, not once per def — this runs on
  // EVERY keydown (including plain typing), and the registry is 16 entries.
  const o = overrides();
  for (const def of SHORTCUT_DEFS) {
    if (def.id === exclude) continue;
    if (matchesEvent(e, o[def.id] ?? def.defaultBinding)) return def.id;
  }
  return null;
}

/** Persist a new binding for `id`. Bindings equal to their default are omitted
 *  from the stored map (so a reset shrinks the preference back toward `{}`). */
export async function setBinding(id: ShortcutId, binding: Binding): Promise<void> {
  const current = overrides();
  const next: Record<string, string> = {};
  for (const def of SHORTCUT_DEFS) {
    const b = def.id === id ? binding : current[def.id];
    if (b && !bindingsEqual(b, def.defaultBinding)) {
      next[def.id] = serializeBinding(b);
    }
  }
  await savePreference(KEYBINDINGS_PREF_KEY, JSON.stringify(next));
}

/** Reset a shortcut to its default binding (drops it from the override map). */
export async function resetBinding(id: ShortcutId): Promise<void> {
  await setBinding(id, shortcutDef(id).defaultBinding);
}

/** Platform-correct display string for the shortcut's current binding. */
export function displayBinding(id: ShortcutId): string {
  return formatBinding(bindingFor(id), isMac);
}

/** Tooltip suffix reflecting the CURRENT (possibly customized) binding,
 *  e.g. "New thread · ⌘K". */
export function tooltipWithShortcut(text: string, id: ShortcutId): string {
  return `${text} · ${displayBinding(id)}`;
}

/** Outcome of a keydown captured while recording a new binding for `id`. The
 *  recorder UI keeps listening on `modifier` (a bare Ctrl/Cmd/Shift/Alt), and
 *  acts on the rest. Pure decision logic — exported for unit testing. */
export type RecordOutcome =
  | { kind: 'modifier' }
  | { kind: 'cancel' }
  | { kind: 'invalid' }
  | { kind: 'conflict'; withId: ShortcutId }
  | { kind: 'ok'; binding: Binding };

const BARE_MODIFIERS = new Set(['Control', 'Meta', 'Shift', 'Alt']);

export function recordChord(e: EventLike & { key: string }, id: ShortcutId): RecordOutcome {
  if (BARE_MODIFIERS.has(e.key)) return { kind: 'modifier' };
  if (e.key === 'Escape') return { kind: 'cancel' };
  const binding = eventToBinding(e);
  if (!isBindableChord(binding)) return { kind: 'invalid' };
  const conflict = matchShortcut(e, id);
  if (conflict) return { kind: 'conflict', withId: conflict };
  return { kind: 'ok', binding };
}
