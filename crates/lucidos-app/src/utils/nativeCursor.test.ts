// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';
import { cursorKeyword, installNativeCursor, resetNativeCursorForTest } from './nativeCursor';

/** Enter `el`, the way a pointer crossing into it reports. */
function enter(el: Element): void {
  el.dispatchEvent(new MouseEvent('pointerover', { bubbles: true }));
}

/** Leave the document, or move on to `to` while staying inside it. */
function out(el: Element, to: Element | null): void {
  el.dispatchEvent(new MouseEvent('pointerout', { bubbles: true, relatedTarget: to }));
}

function withCursor(cursor: string): HTMLElement {
  const el = document.createElement('div');
  el.style.cursor = cursor;
  document.body.appendChild(el);
  return el;
}

describe('cursorKeyword', () => {
  it('takes the plain keyword', () => {
    expect(cursorKeyword('pointer')).toBe('pointer');
    expect(cursorKeyword('  col-resize ')).toBe('col-resize');
  });

  it('takes the fallback after any cursor images', () => {
    expect(cursorKeyword('url("a.png") 2 2, pointer')).toBe('pointer');
  });

  it('answers the initial value for an empty computed value', () => {
    // A real browser always answers something. jsdom does not, and neither does
    // an element the document no longer holds.
    expect(cursorKeyword('')).toBe('auto');
  });

  it('lowercases, since a keyword is case-insensitive', () => {
    expect(cursorKeyword('COL-RESIZE')).toBe('col-resize');
  });
});

describe('installNativeCursor', () => {
  let send: Mock<(cursor: string) => void>;
  let teardown: () => void;

  beforeEach(() => {
    resetNativeCursorForTest();
    send = vi.fn<(cursor: string) => void>();
    (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {
      invoke: vi.fn(),
    };
    teardown = installNativeCursor(send, document);
  });

  afterEach(() => {
    teardown();
    document.body.innerHTML = '';
    delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it('mirrors the hovered element onto the window', () => {
    enter(withCursor('col-resize'));
    expect(send).toHaveBeenCalledWith('col-resize');
  });

  it('costs one call for a run of elements that agree', () => {
    enter(withCursor('pointer'));
    enter(withCursor('pointer'));
    enter(withCursor('pointer'));
    expect(send).toHaveBeenCalledTimes(1);
  });

  it('hands the arrow back on the way off the divider', () => {
    const divider = withCursor('col-resize');
    enter(divider);
    enter(withCursor('default'));
    expect(send.mock.calls.map(([c]) => c)).toEqual(['col-resize', 'default']);
  });

  it('holds the cursor through a drag, where every event retargets to the divider', () => {
    // `setPointerCapture` reports the capture element as the target however far
    // the pointer runs past a clamped divider, so nothing may change.
    const divider = withCursor('col-resize');
    enter(divider);
    enter(divider);
    enter(divider);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it('resets when the pointer leaves the document', () => {
    const divider = withCursor('col-resize');
    enter(divider);
    out(divider, null);
    expect(send).toHaveBeenLastCalledWith('default');
  });

  it('leaves the cursor alone for a crossing that stays inside', () => {
    const divider = withCursor('col-resize');
    const pane = withCursor('default');
    enter(divider);
    out(divider, pane);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it('tries again after a rejected call, instead of trusting the cache', async () => {
    // The cache records the ask, so a failed one must not read as shown.
    teardown();
    const rejecting = vi.fn<(cursor: string) => Promise<void>>()
      .mockRejectedValueOnce(new Error('bridge down'))
      .mockResolvedValue(undefined);
    teardown = installNativeCursor(rejecting, document);

    enter(withCursor('col-resize'));
    await Promise.resolve();
    // A second element wanting the SAME keyword: the de-duplication would
    // swallow this one if the failed ask had stayed in the cache.
    enter(withCursor('col-resize'));

    expect(rejecting.mock.calls.map(([c]) => c)).toEqual(['col-resize', 'col-resize']);
  });

  it('reads the element itself, not an ancestor', () => {
    const pane = withCursor('default');
    const button = document.createElement('button');
    button.style.cursor = 'pointer';
    pane.appendChild(button);
    enter(button);
    expect(send).toHaveBeenLastCalledWith('pointer');
  });
});

describe('installNativeCursor off Tauri', () => {
  it('installs nothing and sends nothing', () => {
    resetNativeCursorForTest();
    delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    const send = vi.fn<(cursor: string) => void>();
    const teardown = installNativeCursor(send, document);
    enter(withCursor('col-resize'));
    expect(send).not.toHaveBeenCalled();
    teardown();
    document.body.innerHTML = '';
  });
});
