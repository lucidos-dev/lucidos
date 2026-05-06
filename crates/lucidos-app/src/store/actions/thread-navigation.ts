import { signal, computed } from '@preact/signals';
import { focusedThreadId, focusedDraftId } from '../store';
import { focusThread } from './threads';
import { focusDraft } from './drafts';

export type ThreadNavEntry =
  | { type: 'thread'; id: string }
  | { type: 'draft'; id: string };

const MAX_THREAD_NAV_STACK = 50;

function entriesEqual(a: ThreadNavEntry, b: ThreadNavEntry): boolean {
  return a.type === b.type && a.id === b.id;
}

export function pushThreadEntry(
  stack: ThreadNavEntry[],
  cursor: number,
  entry: ThreadNavEntry,
): { stack: ThreadNavEntry[]; cursor: number } | null {
  if (cursor < stack.length && entriesEqual(stack[cursor], entry)) return null;
  let newStack = [...stack.slice(0, cursor + 1), entry];
  let newCursor = newStack.length - 1;
  if (newStack.length > MAX_THREAD_NAV_STACK) {
    const overflow = newStack.length - MAX_THREAD_NAV_STACK;
    newStack = newStack.slice(overflow);
    newCursor -= overflow;
  }
  return { stack: newStack, cursor: newCursor };
}

const THREAD_NAV_STORAGE_KEY = 'lucidos-thread-nav-history';

const threadNavStack = signal<ThreadNavEntry[]>([]);
const threadNavCursor = signal(-1);
let _restoring = false;
let _initialized = false;

function isValidEntry(v: unknown): v is ThreadNavEntry {
  if (!v || typeof v !== 'object') return false;
  const o = v as { type?: unknown; id?: unknown };
  return (o.type === 'thread' || o.type === 'draft') && typeof o.id === 'string';
}

function saveThreadNavState(): void {
  try {
    localStorage.setItem(THREAD_NAV_STORAGE_KEY, JSON.stringify({
      stack: threadNavStack.value,
      cursor: threadNavCursor.value,
    }));
  } catch { /* localStorage full or unavailable — non-critical */ }
}

function currentEntry(): ThreadNavEntry {
  return focusedThreadId.value
    ? { type: 'thread', id: focusedThreadId.value }
    : { type: 'draft', id: focusedDraftId.value };
}

function ensureInitialized(): void {
  if (_initialized) return;
  _initialized = true;
  try {
    const saved = localStorage.getItem(THREAD_NAV_STORAGE_KEY);
    if (saved) {
      const parsed = JSON.parse(saved) as { stack: unknown; cursor: unknown };
      const stack = parsed.stack;
      const cursor = parsed.cursor;
      if (Array.isArray(stack) && stack.length > 0 && stack.every(isValidEntry)
          && typeof cursor === 'number' && cursor >= 0 && cursor < stack.length) {
        threadNavStack.value = stack as ThreadNavEntry[];
        threadNavCursor.value = cursor;
        return;
      }
    }
  } catch { /* corrupt data — fall through to fresh init */ }
  threadNavStack.value = [currentEntry()];
  threadNavCursor.value = 0;
}

export const canGoBackThread = computed(() => {
  ensureInitialized();
  return threadNavCursor.value > 0;
});

export const canGoForwardThread = computed(() => {
  ensureInitialized();
  return threadNavCursor.value < threadNavStack.value.length - 1;
});

export function pushThreadNavState(entry: ThreadNavEntry): void {
  ensureInitialized();
  if (_restoring) return;
  const result = pushThreadEntry(threadNavStack.value, threadNavCursor.value, entry);
  if (result) {
    threadNavStack.value = result.stack;
    threadNavCursor.value = result.cursor;
    saveThreadNavState();
  }
}

function restore(entry: ThreadNavEntry): void {
  _restoring = true;
  try {
    if (entry.type === 'thread') focusThread(entry.id);
    else focusDraft(entry.id);
  } finally {
    _restoring = false;
  }
}

export function threadNavBack(): void {
  ensureInitialized();
  if (!canGoBackThread.value) return;
  threadNavCursor.value--;
  restore(threadNavStack.value[threadNavCursor.value]);
  saveThreadNavState();
}

export function threadNavForward(): void {
  ensureInitialized();
  if (!canGoForwardThread.value) return;
  threadNavCursor.value++;
  restore(threadNavStack.value[threadNavCursor.value]);
  saveThreadNavState();
}

/** Test-only: reset module state so each test starts with a clean stack. */
export function _resetThreadNavForTesting(): void {
  threadNavStack.value = [];
  threadNavCursor.value = -1;
  _initialized = false;
  _restoring = false;
}
