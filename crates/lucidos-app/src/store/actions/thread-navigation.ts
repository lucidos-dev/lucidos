import { readWindowItem, removeWindowItem, writeWindowItem } from '../../utils/windowStorage';
import { signal, computed } from '@preact/signals';
import { focusedThreadId } from '../store';
import { closeDashboardPanel, dashboardPanelOpen, openDashboardPanel } from '../dashboards';
import { viewportIsMobile } from '../../utils/viewport';
import { focusThread } from './threads';

/** A thread, or the dashboard panel open over that thread. */
export type ThreadNavEntry = { type: 'thread' | 'dashboard'; id: string };

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
    if (stack[i].id === threadId) {
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

/** Fold every dashboard row into a row for its thread, then merge runs of the
 *  same thread. The cursor stays on the row its old one folded into. Returns
 *  null when there is no dashboard row. */
export function collapseDashboardEntries(
  stack: ThreadNavEntry[],
  cursor: number,
): { stack: ThreadNavEntry[]; cursor: number } | null {
  if (!stack.some((e) => e.type === 'dashboard')) return null;
  const newStack: ThreadNavEntry[] = [];
  let newCursor = -1;
  stack.forEach((entry, i) => {
    const last = newStack[newStack.length - 1];
    if (last?.id !== entry.id) newStack.push({ type: 'thread', id: entry.id });
    if (i === cursor) newCursor = newStack.length - 1;
  });
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
  return (o.type === 'thread' || o.type === 'dashboard') && typeof o.id === 'string';
}

function saveThreadNavState(): void {
  try {
    writeWindowItem(THREAD_NAV_STORAGE_KEY, JSON.stringify({
      stack: threadNavStack.value,
      cursor: threadNavCursor.value,
    }));
  } catch { /* localStorage full or unavailable — non-critical */ }
}

function ensureInitialized(): void {
  if (_initialized) return;
  _initialized = true;
  try {
    const saved = readWindowItem(THREAD_NAV_STORAGE_KEY);
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

/** Off the phone layout there is no dashboard panel, so its rows would be
 *  Back steps that change nothing on screen. */
export function collapseDashboardNavEntries(): void {
  ensureInitialized();
  const result = collapseDashboardEntries(threadNavStack.value, threadNavCursor.value);
  if (!result) return;
  threadNavStack.value = result.stack;
  threadNavCursor.value = result.cursor;
  saveThreadNavState();
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

/** The full thread-nav stack + cursor — read by the long-press history menu on
 *  the thread chevrons to list the threads back/forward walk toward. */
export const threadNavHistory = computed(() => {
  ensureInitialized();
  return { stack: threadNavStack.value, cursor: threadNavCursor.value };
});

/** Jump straight to an absolute index in the thread-nav stack (the long-press
 *  history menu). Always restores (re-focuses) the target thread — including
 *  the unfocused case where the cursor still points at the thread just left.
 *  Out-of-range is a no-op. */
export function threadNavGoTo(index: number): void {
  ensureInitialized();
  const stack = threadNavStack.value;
  if (index < 0 || index >= stack.length) return;
  threadNavCursor.value = index;
  restore(stack[index]);
  saveThreadNavState();
}

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
    // Focusing closes the panel, so a dashboard row opens it afterwards.
    focusThread(entry.id);
    if (entry.type === 'dashboard' && viewportIsMobile.value) openDashboardPanel();
  } finally {
    _restoring = false;
  }
}

/** Open or close the dashboard panel, and record it in the thread history so
 *  Back closes it and Forward reopens it. Closing steps back onto the thread
 *  row under the panel when there is one, so toggling never grows the history.
 *  With no thread open there is no row to record. */
export function setDashboardPanelOpen(open: boolean): void {
  if (dashboardPanelOpen.value === open) return;
  if (open) openDashboardPanel();
  else closeDashboardPanel();
  const id = focusedThreadId.value;
  if (!id) return;
  const threadRow: ThreadNavEntry = { type: 'thread', id };
  if (!open && stepBackOnto(threadRow)) return;
  pushThreadNavState(open ? { type: 'dashboard', id } : threadRow);
}

/** Move the cursor back one row when that row is `entry`. */
function stepBackOnto(entry: ThreadNavEntry): boolean {
  ensureInitialized();
  const previous = threadNavStack.value[threadNavCursor.value - 1];
  if (!previous || !entriesEqual(previous, entry)) return false;
  threadNavCursor.value--;
  saveThreadNavState();
  return true;
}

export function toggleDashboardPanel(): void {
  setDashboardPanelOpen(!dashboardPanelOpen.value);
}

/** Open the panel when the history sits on a dashboard row for the open
 *  thread: at a reload, and when the layout turns phone. */
export function reopenDashboardPanelFromHistory(): void {
  ensureInitialized();
  const row = threadNavStack.value[threadNavCursor.value];
  if (row?.type === 'dashboard' && row.id === focusedThreadId.value && viewportIsMobile.value) {
    openDashboardPanel();
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
  try { removeWindowItem(THREAD_NAV_STORAGE_KEY); } catch { /* noop */ }
}

