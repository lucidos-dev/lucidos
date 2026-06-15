import { signal, computed } from '@preact/signals';
import { focusedThreadId } from '../store';
import { focusThread } from './threads';

export type ThreadNavEntry = { type: 'thread'; id: string };

export const MAX_THREAD_NAV_STACK = 50;

function entriesEqual(a: ThreadNavEntry, b: ThreadNavEntry): boolean {
  return a.type === b.type && a.id === b.id;
}

export function pushThreadEntry(
  stack: ThreadNavEntry[],
  cursor: number,
  entry: ThreadNavEntry,
): { stack: ThreadNavEntry[]; cursor: number } | null {
  if (cursor >= 0 && cursor < stack.length && entriesEqual(stack[cursor], entry)) return null;
  let newStack = [...stack.slice(0, cursor + 1), entry];
  let newCursor = newStack.length - 1;
  if (newStack.length > MAX_THREAD_NAV_STACK) {
    const overflow = newStack.length - MAX_THREAD_NAV_STACK;
    newStack = newStack.slice(overflow);
    newCursor -= overflow;
  }
  return { stack: newStack, cursor: newCursor };
}

/** Remove every entry matching `threadId` from the stack. When a removed
 *  entry sat at or before the cursor, the cursor shifts back to keep the
 *  user's "current" position on whatever real entry remains. Returns null
 *  when nothing matched so callers can skip an unnecessary signal write. */
export function removeEntriesById(
  stack: ThreadNavEntry[],
  cursor: number,
  threadId: string,
): { stack: ThreadNavEntry[]; cursor: number } | null {
  let removedAtOrBeforeCursor = 0;
  const newStack: ThreadNavEntry[] = [];
  for (let i = 0; i < stack.length; i++) {
    if (stack[i].type === 'thread' && stack[i].id === threadId) {
      if (i <= cursor) removedAtOrBeforeCursor++;
    } else {
      newStack.push(stack[i]);
    }
  }
  if (newStack.length === stack.length) return null;
  let newCursor = cursor - removedAtOrBeforeCursor;
  if (newStack.length === 0) newCursor = -1;
  else if (newCursor < 0) newCursor = 0;
  else if (newCursor >= newStack.length) newCursor = newStack.length - 1;
  return { stack: newStack, cursor: newCursor };
}

/** Replace the entry at `cursor` (and drop any forward history) with `entry`.
 *  When the cursor is out of range, falls back to a normal push. */
export function replaceThreadEntry(
  stack: ThreadNavEntry[],
  cursor: number,
  entry: ThreadNavEntry,
): { stack: ThreadNavEntry[]; cursor: number } {
  if (cursor < 0 || cursor >= stack.length) {
    return pushThreadEntry(stack, cursor, entry) ?? { stack, cursor };
  }
  if (entriesEqual(stack[cursor], entry)) {
    return { stack: stack.slice(0, cursor + 1), cursor };
  }
  const newStack = [...stack.slice(0, cursor), entry];
  return { stack: newStack, cursor: newStack.length - 1 };
}

const THREAD_NAV_STORAGE_KEY = 'lucidos-thread-nav-history';

const threadNavStack = signal<ThreadNavEntry[]>([]);
const threadNavCursor = signal(-1);
let _restoring = false;
let _initialized = false;

function isValidEntry(v: unknown): v is ThreadNavEntry {
  if (!v || typeof v !== 'object') return false;
  const o = v as { type?: unknown; id?: unknown };
  return o.type === 'thread' && typeof o.id === 'string';
}

function saveThreadNavState(): void {
  try {
    localStorage.setItem(THREAD_NAV_STORAGE_KEY, JSON.stringify({
      stack: threadNavStack.value,
      cursor: threadNavCursor.value,
    }));
  } catch { /* localStorage full or unavailable — non-critical */ }
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
  // No focused thread on first init = empty nav stack. The first focusThread
  // / sendMessage will seed it.
  const focused = focusedThreadId.value;
  if (focused) {
    threadNavStack.value = [{ type: 'thread', id: focused }];
    threadNavCursor.value = 0;
  } else {
    threadNavStack.value = [];
    threadNavCursor.value = -1;
  }
}

export const canGoBackThread = computed(() => {
  ensureInitialized();
  const cursor = threadNavCursor.value;
  if (cursor < 0) return false;
  // No focused thread (Compose, Discard-of-draft, Esc, close button) — the
  // cursor still points at the thread the user just left, so the first Back
  // press restores that entry. Enabled even at cursor=0 for that case.
  if (focusedThreadId.value === null) return true;
  return cursor > 0;
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

/** Drop every nav entry pointing at `threadId` (e.g. after the user discards
 *  an unsent draft). Without this, Back / Forward would later restore a
 *  thread that no longer exists, leaving the UI on a phantom row. */
export function removeThreadNavEntries(threadId: string): void {
  ensureInitialized();
  const result = removeEntriesById(threadNavStack.value, threadNavCursor.value, threadId);
  if (!result) return;
  threadNavStack.value = result.stack;
  threadNavCursor.value = result.cursor;
  saveThreadNavState();
}

function restore(entry: ThreadNavEntry): void {
  _restoring = true;
  try {
    focusThread(entry.id);
  } finally {
    _restoring = false;
  }
}

export function threadNavBack(): void {
  ensureInitialized();
  if (!canGoBackThread.value) return;
  // See canGoBackThread: unfocused short-circuits without decrementing.
  if (focusedThreadId.value === null) {
    restore(threadNavStack.value[threadNavCursor.value]);
    return;
  }
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

/** Test-only: read the live nav stack/cursor. Underscored to mark it as not
 *  part of the production API — tests assert on signal state without needing
 *  another module export to leak. */
export function _threadNavStateForTesting(): { stack: readonly ThreadNavEntry[]; cursor: number } {
  return { stack: threadNavStack.value, cursor: threadNavCursor.value };
}

/** Test-only: clear the in-module signals + initialization flag. The module
 *  caches `_initialized` so production reads localStorage exactly once; tests
 *  that mutate the stack across cases need a way to start from a clean slate. */
export function _resetThreadNavForTesting(): void {
  threadNavStack.value = [];
  threadNavCursor.value = -1;
  _initialized = false;
  try { localStorage.removeItem(THREAD_NAV_STORAGE_KEY); } catch { /* noop */ }
}

