import { describe, it, expect, beforeEach } from 'vitest';
import { focusedPane } from '../../store/store';
import { handleDrawerPointerDown, isThreadRowTarget, pickInitialHighlight, navKeyDomId } from './ThreadDrawer';

// The test environment has no real DOM, so we stand in minimal `closest`-bearing
// stubs for the pointer-down target. A thread row is any element inside a
// `[data-thread-nav]`; drawer chrome (section headers, empty space) is not.
const rowTarget = {
  closest: (sel: string) => (sel === '[data-thread-nav]' ? ({} as unknown) : null),
} as unknown as EventTarget;
const chromeTarget = { closest: () => null } as unknown as EventTarget;

describe('handleDrawerPointerDown', () => {
  beforeEach(() => {
    // Desktop — focusPane is a no-op on mobile.
    (globalThis as any).innerWidth = 1024;
    focusedPane.value = 'thread';
  });

  it('does NOT focus the drawer when the pointer-down lands on a thread row', () => {
    // A row click focuses a thread, which focuses the thread pane. Pre-focusing
    // the drawer here would flash the focused pane drawer→thread.
    handleDrawerPointerDown(rowTarget);
    expect(focusedPane.value).toBe('thread');
  });

  it('focuses the drawer when the pointer-down lands on drawer chrome', () => {
    handleDrawerPointerDown(chromeTarget);
    expect(focusedPane.value).toBe('drawer');
  });

  it('focuses the drawer when the pointer-down lands on empty space (null target)', () => {
    handleDrawerPointerDown(null);
    expect(focusedPane.value).toBe('drawer');
  });
});

describe('isThreadRowTarget', () => {
  it('detects a target inside a thread row', () => {
    expect(isThreadRowTarget(rowTarget)).toBe(true);
  });

  it('rejects drawer chrome, null, and non-Element targets', () => {
    expect(isThreadRowTarget(chromeTarget)).toBe(false);
    expect(isThreadRowTarget(null)).toBe(false);
    expect(isThreadRowTarget({} as EventTarget)).toBe(false);
  });
});

describe('pickInitialHighlight', () => {
  it('seeds the open thread when it is navigable', () => {
    expect(pickInitialHighlight('b', ['a', 'b', 'c'])).toBe('b');
  });
  it('falls back to the first row when the open thread is not in the list', () => {
    expect(pickInitialHighlight('z', ['a', 'b', 'c'])).toBe('a');
  });
  it('falls back to the first row when no thread is open', () => {
    expect(pickInitialHighlight(null, ['a', 'b', 'c'])).toBe('a');
  });
  it('returns null for an empty list', () => {
    expect(pickInitialHighlight('a', [])).toBeNull();
    expect(pickInitialHighlight(null, [])).toBeNull();
  });
});

describe('navKeyDomId', () => {
  // The drawer container's aria-activedescendant points at the highlighted node's
  // DOM id; rows + section headers carry the matching id. They must agree, so the
  // derivation lives in one pure place.
  it('namespaces a thread id', () => {
    expect(navKeyDomId('thread-abc')).toBe('drawer-nav-thread-abc');
  });
  it('namespaces a section nav key', () => {
    expect(navKeyDomId('__section_saved')).toBe('drawer-nav-__section_saved');
  });
  it('returns undefined for a null (no) highlight so the attribute is omitted', () => {
    expect(navKeyDomId(null)).toBeUndefined();
  });
});
