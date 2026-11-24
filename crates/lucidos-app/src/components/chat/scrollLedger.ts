import { nowMs } from '../../utils/scrollActivity';

export type NavScrollKind = 'placement' | 'held' | 'carry' | 'anchor';

export const WRITE_EVENT_WINDOW_MS = 64;

interface LedgerEntry {
  count: number;
  at: number;
  top: number | null;
  kind: NavScrollKind;
  uncarriedAt: number;
}

const _entries = new WeakMap<HTMLElement, LedgerEntry>();

let _lastWritten: HTMLElement | null = null;

let _hold: { el: HTMLElement; top: number } | null = null;

function entryFor(el?: HTMLElement | null): LedgerEntry | undefined {
  const target = el ?? _lastWritten;
  return target ? _entries.get(target) : undefined;
}

const _rebasedScrollListeners = new Set<(el: HTMLElement, kind: NavScrollKind) => void>();

export function onRebasedScroll(listener: (el: HTMLElement, kind: NavScrollKind) => void): () => void {
  _rebasedScrollListeners.add(listener);
  return () => { _rebasedScrollListeners.delete(listener); };
}

export function writeLedgerScroll(el: HTMLElement, top: number, kind: NavScrollKind): void {
  const now = nowMs();
  const previous = _entries.get(el);
  const uncarriedAt = previous?.uncarriedAt ?? -Infinity;
  const sharesAnEvent = kind === 'carry' && now - uncarriedAt < WRITE_EVENT_WINDOW_MS;
  const recorded: NavScrollKind = sharesAnEvent ? 'held' : kind;
  const entry: LedgerEntry = {
    count: (previous?.count ?? 0) + 1,
    at: now,
    top: previous?.top ?? null,
    kind: recorded,
    uncarriedAt: kind === 'carry' ? uncarriedAt : now,
  };
  _entries.set(el, entry);
  _lastWritten = el;
  el.scrollTop = top;
  entry.top = el.scrollTop;
  for (const listener of _rebasedScrollListeners) listener(el, recorded);
}

export function writeCount(el: HTMLElement): number {
  return _entries.get(el)?.count ?? 0;
}

export function wroteWithinEventWindow(el?: HTMLElement | null): boolean {
  const entry = entryFor(el);
  return entry !== undefined && nowMs() - entry.at < WRITE_EVENT_WINDOW_MS;
}

export function lastWriteKind(el?: HTMLElement | null): NavScrollKind | null {
  return entryFor(el)?.kind ?? null;
}

export function isWhereWeLastScrolledIt(el: HTMLElement): boolean {
  const top = _entries.get(el)?.top ?? null;
  return top !== null && Math.abs(el.scrollTop - top) <= 1;
}

export function forgetLeftPosition(el: HTMLElement): void {
  const entry = _entries.get(el);
  if (entry && !isWhereWeLastScrolledIt(el)) entry.top = null;
}

export function holdPosition(el: HTMLElement | null): void {
  _hold = el ? { el, top: el.scrollTop } : null;
}

export function holdsReaderOn(el: HTMLElement): boolean {
  return _hold?.el === el;
}

export function isWhereWeHeldIt(el: HTMLElement): boolean {
  return _hold?.el === el && Math.abs(el.scrollTop - _hold.top) <= 1;
}
