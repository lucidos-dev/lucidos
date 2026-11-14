import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { readWindowItem, writeWindowItem } from '../../utils/windowStorage';
import { collapseDashboardEntries, pushThreadEntry, removeEntriesById, MAX_THREAD_NAV_STACK, type ThreadNavEntry } from './thread-navigation';

const T = (id: string): ThreadNavEntry => ({ type: 'thread', id });

describe('pushThreadEntry', () => {
  it('seeds an empty stack from cursor=-1 — initial focus on a fresh tab', () => {
    // Regression: ensureInitialized leaves stack=[] and cursor=-1 when no thread is
    // focused at startup. The first focusThread() call lands here. Before the
    // guard, `cursor < stack.length` was `-1 < 0` (true), so entriesEqual read
    // stack[-1].type and threw, aborting focusThread before navigateToPane ran.
    // Symptom: tapping a thread on mobile focused it but never switched panes.
    const result = pushThreadEntry([], -1, T('thread-1'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([T('thread-1')]);
    expect(result!.cursor).toBe(0);
  });

  it('returns null for duplicate thread entry at cursor', () => {
    expect(pushThreadEntry([T('thread-1')], 0, T('thread-1'))).toBeNull();
  });

  it('pushes a new thread after an existing one', () => {
    const result = pushThreadEntry([T('thread-1')], 0, T('thread-2'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([T('thread-1'), T('thread-2')]);
    expect(result!.cursor).toBe(1);
  });

  it('truncates forward history when pushing from middle', () => {
    const stack: ThreadNavEntry[] = [T('t0'), T('t1'), T('t2')];
    const result = pushThreadEntry(stack, 1, T('t3'));
    expect(result).not.toBeNull();
    expect(result!.stack).toEqual([T('t0'), T('t1'), T('t3')]);
    expect(result!.cursor).toBe(2);
  });

  it('caps stack at MAX_THREAD_NAV_STACK', () => {
    let stack: ThreadNavEntry[] = [T('seed')];
    let cursor = 0;
    for (let i = 0; i < MAX_THREAD_NAV_STACK; i++) {
      const r = pushThreadEntry(stack, cursor, T(`thread-${i}`));
      stack = r!.stack; cursor = r!.cursor;
    }
    expect(stack).toHaveLength(MAX_THREAD_NAV_STACK);
    expect(cursor).toBe(MAX_THREAD_NAV_STACK - 1);
  });
});

describe('removeEntriesById', () => {
  it('returns null when id is not in stack', () => {
    expect(removeEntriesById([T('a'), T('b')], 1, 'missing')).toBeNull();
  });

  it('removes the cursor entry and shifts cursor back to the previous entry', () => {
    // Discarding a draft that the user is currently sitting on: cursor should
    // land on the entry before the removed one so the next Back/Forward feels
    // contiguous from the user's last "real" position.
    const r = removeEntriesById([T('x'), T('draft')], 1, 'draft');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([T('x')]);
    expect(r!.cursor).toBe(0);
  });

  it('removes an entry before the cursor and decrements cursor', () => {
    const r = removeEntriesById([T('draft'), T('x'), T('y')], 2, 'draft');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([T('x'), T('y')]);
    expect(r!.cursor).toBe(1);
  });

  it('removes an entry after the cursor and leaves cursor alone', () => {
    const r = removeEntriesById([T('x'), T('y'), T('draft')], 0, 'draft');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([T('x'), T('y')]);
    expect(r!.cursor).toBe(0);
  });

  it('removes every occurrence of the id', () => {
    const r = removeEntriesById([T('a'), T('dup'), T('b'), T('dup')], 2, 'dup');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([T('a'), T('b')]);
    expect(r!.cursor).toBe(1);
  });

  it('empties the stack and resets cursor when the only entry is removed', () => {
    const r = removeEntriesById([T('only')], 0, 'only');
    expect(r).not.toBeNull();
    expect(r!.stack).toEqual([]);
    expect(r!.cursor).toBe(-1);
  });
});

describe('thread back/forward simulation', () => {
  it('full navigation cycle', () => {
    let stack: ThreadNavEntry[] = [T('seed')];
    let cursor = 0;

    let r = pushThreadEntry(stack, cursor, T('thread-1'));
    stack = r!.stack; cursor = r!.cursor;
    r = pushThreadEntry(stack, cursor, T('thread-2'));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toEqual([T('seed'), T('thread-1'), T('thread-2')]);
    expect(cursor).toBe(2);

    cursor--;
    expect(stack[cursor]).toEqual(T('thread-1'));
    cursor--;
    expect(stack[cursor]).toEqual(T('seed'));

    cursor++;
    expect(stack[cursor]).toEqual(T('thread-1'));

    r = pushThreadEntry(stack, cursor, T('thread-3'));
    stack = r!.stack; cursor = r!.cursor;
    expect(stack).toEqual([T('seed'), T('thread-1'), T('thread-3')]);
  });
});

describe('the dashboard panel in the thread history', () => {
  const D = (id: string): ThreadNavEntry => ({ type: 'dashboard', id });
  const KEY = 'lucidos-thread-nav-history';
  let nav: typeof import('./thread-navigation');
  let dashboards: typeof import('../dashboards');
  let store: typeof import('../store');
  let viewport: typeof import('../../utils/viewport');

  beforeEach(async () => {
    vi.resetModules();
    localStorage.clear();
    // The real focusThread loads events over the network. This stand-in keeps
    // the three effects the history depends on: focus moves, the panel closes,
    // a thread row is pushed.
    vi.doMock('./threads', async () => {
      const s = await import('../store');
      const d = await import('../dashboards');
      return {
        focusThread: (id: string) => {
          s.focusedThreadId.value = id;
          d.closeDashboardPanel();
          nav.pushThreadNavState({ type: 'thread', id });
        },
      };
    });
    nav = await import('./thread-navigation');
    dashboards = await import('../dashboards');
    store = await import('../store');
    viewport = await import('../../utils/viewport');
    viewport.viewportIsMobile.value = true;
    store.focusedThreadId.value = 'a';
    nav._resetThreadNavForTesting();
    dashboards.closeDashboardPanel();
  });

  afterEach(() => {
    vi.doUnmock('./threads');
    store.focusedThreadId.value = null;
  });

  it('opening pushes a dashboard row for the thread under it', () => {
    nav.setDashboardPanelOpen(true);
    expect(dashboards.dashboardPanelOpen.value).toBe(true);
    expect(nav._threadNavStateForTesting()).toEqual({ stack: [T('a'), D('a')], cursor: 1 });
  });

  it('Back closes the panel on the thread row, and Forward reopens it', () => {
    nav.setDashboardPanelOpen(true);
    nav.threadNavBack();
    expect(dashboards.dashboardPanelOpen.value).toBe(false);
    expect(nav._threadNavStateForTesting().cursor).toBe(0);
    nav.threadNavForward();
    expect(dashboards.dashboardPanelOpen.value).toBe(true);
    expect(store.focusedThreadId.value).toBe('a');
    expect(nav._threadNavStateForTesting().cursor).toBe(1);
  });

  it('closing with the button steps back onto the thread row, so Forward reopens the panel', () => {
    nav.setDashboardPanelOpen(true);
    nav.setDashboardPanelOpen(false);
    expect(nav._threadNavStateForTesting()).toEqual({ stack: [T('a'), D('a')], cursor: 0 });
    nav.threadNavForward();
    expect(dashboards.dashboardPanelOpen.value).toBe(true);
  });

  /** Invariant: toggling the panel never grows the history past one thread row and one dashboard row. */
  it('toggling the panel repeatedly leaves one thread row and one dashboard row', () => {
    for (let i = 0; i < 5; i++) {
      nav.setDashboardPanelOpen(true);
      nav.setDashboardPanelOpen(false);
    }
    expect(nav._threadNavStateForTesting()).toEqual({ stack: [T('a'), D('a')], cursor: 0 });
    nav.setDashboardPanelOpen(true);
    expect(nav._threadNavStateForTesting()).toEqual({ stack: [T('a'), D('a')], cursor: 1 });
  });

  it('closing a dashboard row with no thread row before it pushes a thread row', () => {
    writeWindowItem(KEY, JSON.stringify({ stack: [T('b'), D('a')], cursor: 1 }));
    dashboards.openDashboardPanel();
    nav.setDashboardPanelOpen(false);
    expect(nav._threadNavStateForTesting()).toEqual({ stack: [T('b'), D('a'), T('a')], cursor: 2 });
  });

  it('a toggle that changes nothing pushes nothing', () => {
    nav.setDashboardPanelOpen(true);
    nav.setDashboardPanelOpen(true);
    expect(nav._threadNavStateForTesting()).toEqual({ stack: [T('a'), D('a')], cursor: 1 });
    nav.setDashboardPanelOpen(false);
    nav.setDashboardPanelOpen(false);
    expect(nav._threadNavStateForTesting()).toEqual({ stack: [T('a'), D('a')], cursor: 0 });
  });

  it('opens with no thread open without pushing a row', () => {
    store.focusedThreadId.value = null;
    nav._resetThreadNavForTesting();
    nav.setDashboardPanelOpen(true);
    expect(dashboards.dashboardPanelOpen.value).toBe(true);
    expect(nav._threadNavStateForTesting().stack).toEqual([]);
  });

  /** Invariant: off the phone layout no dashboard row survives, so every Back step changes the screen. */
  it('folding the live history rewrites it and saves it', () => {
    nav.setDashboardPanelOpen(true);
    nav.setDashboardPanelOpen(false);
    nav.collapseDashboardNavEntries();
    expect(nav._threadNavStateForTesting()).toEqual({ stack: [T('a')], cursor: 0 });
    expect(JSON.parse(readWindowItem(KEY)!)).toEqual({ stack: [T('a')], cursor: 0 });
  });

  /** Invariant: an iOS cold launch can read the layout wrong at first, so loading never folds. */
  it('a reload off the phone layout keeps the saved dashboard rows', () => {
    viewport.viewportIsMobile.value = false;
    writeWindowItem(KEY, JSON.stringify({ stack: [T('a'), D('a')], cursor: 1 }));
    expect(nav.threadNavHistory.value).toEqual({ stack: [T('a'), D('a')], cursor: 1 });
  });

  it('a reload with the cursor on a dashboard row reopens the panel', () => {
    writeWindowItem(KEY, JSON.stringify({ stack: [T('a'), D('a')], cursor: 1 }));
    nav.reopenDashboardPanelFromHistory();
    expect(dashboards.dashboardPanelOpen.value).toBe(true);
  });

  it('a reload with the cursor on a thread row leaves the panel closed', () => {
    writeWindowItem(KEY, JSON.stringify({ stack: [T('a'), D('a'), T('a')], cursor: 2 }));
    nav.reopenDashboardPanelFromHistory();
    expect(dashboards.dashboardPanelOpen.value).toBe(false);
  });

  it('a dashboard row for a thread that is not open does not reopen the panel', () => {
    writeWindowItem(KEY, JSON.stringify({ stack: [T('b'), D('b')], cursor: 1 }));
    nav.reopenDashboardPanelFromHistory();
    expect(dashboards.dashboardPanelOpen.value).toBe(false);
  });

  it('a saved history of thread rows only still loads', () => {
    writeWindowItem(KEY, JSON.stringify({ stack: [T('x'), T('y')], cursor: 1 }));
    expect(nav.canGoBackThread.value).toBe(true);
    expect(nav._threadNavStateForTesting().stack).toEqual([T('x'), T('y')]);
  });

  it('folds each dashboard row into its thread and keeps the cursor on that thread', () => {
    expect(collapseDashboardEntries([T('a'), D('a'), T('a'), T('b'), D('b')], 2))
      .toEqual({ stack: [T('a'), T('b')], cursor: 0 });
    expect(collapseDashboardEntries([T('a'), D('a'), T('b'), D('b')], 3))
      .toEqual({ stack: [T('a'), T('b')], cursor: 1 });
  });

  it('folds nothing when no dashboard row is there', () => {
    expect(collapseDashboardEntries([T('a'), T('b')], 1)).toBeNull();
  });

  it('discarding a thread drops its dashboard rows too', () => {
    const r = removeEntriesById([T('a'), D('a'), T('b')], 2, 'a');
    expect(r!.stack).toEqual([T('b')]);
  });
});
