
import { preferences } from '../store';
import { savePreference } from './preferences';
import { isMac } from '../../utils/platform';
import { TOOLTIP_SHORTCUT_SEPARATOR } from '@lucidos/tooltip';
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
  isComposerEnterChord,
  type Binding,
  type ShortcutId,
} from '../../utils/shortcuts';

export const KEYBINDINGS_PREF_KEY = 'keybindings';

const LEGACY_SHORTCUT_IDS: Partial<Record<ShortcutId, string>> = {
  historyBack: 'previousThread',
  historyForward: 'nextThread',
  prevTurnOrNotification: 'prevThreadTurn',
  nextTurnOrNotification: 'nextThreadTurn',
};

type EventLike = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>;

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

export function forwardableBindings(omit: readonly ShortcutId[] = []): Binding[] {
  const o = overrides();
  return SHORTCUT_DEFS
    .filter((def) => !def.hostOnly && !omit.includes(def.id))
    .map((def) => o[def.id] ?? def.defaultBinding);
}

export function bindingFor(id: ShortcutId): Binding {
  return overrides()[id] ?? shortcutDef(id).defaultBinding;
}

export function isCustomized(id: ShortcutId): boolean {
  const o = overrides()[id];
  return !!o && !bindingsEqual(o, shortcutDef(id).defaultBinding);
}

export function matchShortcut(e: EventLike, exclude?: ShortcutId): ShortcutId | null {
  const o = overrides();
  for (const def of SHORTCUT_DEFS) {
    const own = o[def.id];
    if (def.id !== exclude && own && matchesEvent(e, own)) return def.id;
  }
  for (const def of SHORTCUT_DEFS) {
    if (def.id !== exclude && !o[def.id] && matchesEvent(e, def.defaultBinding)) return def.id;
  }
  return null;
}

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

export async function resetBinding(id: ShortcutId): Promise<void> {
  await setBinding(id, shortcutDef(id).defaultBinding);
}

export function displayBinding(id: ShortcutId): string {
  return formatBinding(bindingFor(id), isMac);
}

export function tooltipWithShortcut(text: string, id: ShortcutId): string {
  return `${text}${TOOLTIP_SHORTCUT_SEPARATOR}${displayBinding(id)}`;
}

export type RecordOutcome =
  | { kind: 'modifier' }
  | { kind: 'cancel' }
  | { kind: 'invalid' }
  | { kind: 'composer' }
  | { kind: 'conflict'; withId: ShortcutId }
  | { kind: 'ok'; binding: Binding };

const BARE_MODIFIERS = new Set(['Control', 'Meta', 'Shift', 'Alt']);

export function recordChord(e: EventLike & { key: string }, id: ShortcutId): RecordOutcome {
  if (BARE_MODIFIERS.has(e.key)) return { kind: 'modifier' };
  if (e.key === 'Escape') return { kind: 'cancel' };
  const binding = eventToBinding(e);
  if (!isBindableChord(binding)) return { kind: 'invalid' };
  if (isComposerEnterChord(binding)) return { kind: 'composer' };
  const conflict = matchShortcut(e, id);
  if (conflict) return { kind: 'conflict', withId: conflict };
  return { kind: 'ok', binding };
}
