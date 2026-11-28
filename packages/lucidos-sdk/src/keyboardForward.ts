
import { consumeLocalOverlayEscape } from './_localOverlayEscape';
import { onHostPush, postToHost } from './_bridge';

export const FORWARD_KEYDOWN_TYPE = 'lucidos:keydown';

export const KEYBINDINGS_CHANNEL = 'keybindings';

interface HostBinding {
  mod: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

let hostBindings: readonly HostBinding[] = [];

function isHostBinding(v: unknown): v is HostBinding {
  const b = v as HostBinding | null;
  return !!b && typeof b.mod === 'boolean' && typeof b.shift === 'boolean'
    && typeof b.alt === 'boolean' && typeof b.key === 'string';
}

export function adoptHostBindings(data: unknown): void {
  const list = (data as { bindings?: unknown } | null)?.bindings;
  hostBindings = Array.isArray(list) ? list.filter(isHostBinding) : [];
}

function normalizeKey(key: string): string {
  if (key === '+') return '=';
  return key.length === 1 ? key.toLowerCase() : key;
}

type HostChord = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>
  & Partial<Pick<KeyboardEvent, 'code'>>;

function isPunctuationKey(key: string): boolean {
  return key.length === 1 && !/[\p{L}\p{N}\s]/u.test(key);
}

function digitOfCode(code: string | undefined): string | undefined {
  return code === undefined ? undefined : /^Digit([0-9])$/.exec(code)?.[1];
}

function matchesOne(e: HostChord, b: HostBinding): boolean {
  if ((e.metaKey || e.ctrlKey) !== b.mod || e.altKey !== b.alt) return false;
  const shiftFits = e.shiftKey === b.shift;
  if (normalizeKey(e.key) === b.key) return shiftFits || (!b.shift && isPunctuationKey(b.key));
  return shiftFits && digitOfCode(e.code) === b.key;
}

export function matchesHostBinding(e: HostChord, bindings: readonly HostBinding[] = hostBindings): boolean {
  return bindings.some((b) => matchesOne(e, b));
}

type ChordSource = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'altKey' | 'key'>
  & Partial<Pick<KeyboardEvent, 'shiftKey' | 'target'>>;

const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);

function isEditable(target: EventTarget | null | undefined): boolean {
  const el = target as HTMLElement | null | undefined;
  return !!el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName ?? ''));
}

export function isForwardableKeydown(e: ChordSource, mac: boolean = IS_MAC): boolean {
  const macTextEdit = mac && e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey
    && /^[a-z]$/i.test(e.key) && isEditable(e.target);
  if (macTextEdit) return false;
  return e.metaKey || e.ctrlKey || e.altKey || e.key === 'Escape' || /^F\d{1,2}$/.test(e.key);
}

export interface ForwardedKeydown {
  type: typeof FORWARD_KEYDOWN_TYPE;
  key: string;
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export function toForwardedKeydown(e: KeyboardEvent): ForwardedKeydown {
  return {
    type: FORWARD_KEYDOWN_TYPE,
    key: e.key,
    code: e.code,
    metaKey: e.metaKey,
    ctrlKey: e.ctrlKey,
    shiftKey: e.shiftKey,
    altKey: e.altKey,
  };
}

export function installKeyboardForwarding(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  const offBindings = onHostPush(KEYBINDINGS_CHANNEL, adoptHostBindings);
  const onKeyDown = (e: KeyboardEvent) => {
    if (consumeLocalOverlayEscape(e) || !isForwardableKeydown(e)) return;
    if (matchesHostBinding(e)) e.preventDefault();
    postToHost(toForwardedKeydown(e));
  };
  window.addEventListener('keydown', onKeyDown, true);
  return () => {
    window.removeEventListener('keydown', onKeyDown, true);
    offBindings();
  };
}
