// @vitest-environment jsdom
/**
 * The file-preview content pane's right-click menu. It must open with the
 * same actions the header toolbar offers. It must stay out of the way of a
 * selection, a link, or anything else with its own native menu. See
 * `docs/plans/2026-10-04-file-preview-context-menu.md`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { FilePreviewContextMenu } from '../FilePreviewContextMenu';

const platform = vi.hoisted(() => ({ isTauri: false, isIOSPwa: false, clipboardCopy: true }));
vi.mock('../../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/platform')>()),
  isTauri: () => platform.isTauri,
  isIOSPwa: () => platform.isIOSPwa,
  clipboardAbilities: () => ({ copy: platform.clipboardCopy, paste: false }),
}));

const { workspacePath, repositories, filePreviewEditing } = await import('../../../store/store');

const PATH = 'artifacts/notes.md';
let host: HTMLDivElement;

beforeEach(() => {
  (globalThis as unknown as { window: { location: unknown } }).window.location =
    { pathname: '/dev/', search: '', href: 'https://localhost:5251/dev/' };
  platform.isTauri = false;
  platform.isIOSPwa = false;
  platform.clipboardCopy = true;
  workspacePath.value = '/home/user/workspaces/dev';
  repositories.value = { status: 'loaded', data: [] };
  filePreviewEditing.value = false;
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  vi.restoreAllMocks();
});

function show(layout: 'desktop' | 'mobile' = 'desktop') {
  act(() => {
    render(
      <FilePreviewContextMenu path={PATH} layout={layout}>
        <p data-pick="prose">Hello</p>
        <a href="https://example.com" data-pick="link">docs</a>
      </FilePreviewContextMenu>,
      host,
    );
  });
}

/** A right-click at `(x, y)`, over `target`, with no selection under the
 *  pointer before the press — the ordinary case. Dispatched inside `act()`
 *  so the resulting `setMenu` state update renders before the test reads
 *  the DOM, matching the real browser's own paint-before-next-task. */
function rightClick(target: Element, x = 30, y = 28): MouseEvent {
  let e!: MouseEvent;
  act(() => {
    target.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 2, clientX: x, clientY: y }));
    e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: x, clientY: y });
    target.dispatchEvent(e);
  });
  return e;
}

const menu = () => host.ownerDocument.querySelector('.thread-overflow-menu');
const menuLabels = () =>
  [...(menu()?.querySelectorAll('[role="menuitem"]') ?? [])].map((n) => n.textContent);

describe('FilePreviewContextMenu', () => {
  it('opens with the same actions the header toolbar would offer', () => {
    show();
    rightClick(host.querySelector('[data-pick="prose"]')!);

    expect(menu()).not.toBeNull();
    expect(menuLabels().some((t) => t?.includes('Copy path'))).toBe(true);
  });

  it('claims the event, so the browser draws nothing of its own', () => {
    show();
    const e = rightClick(host.querySelector('[data-pick="prose"]')!);
    expect(e.defaultPrevented).toBe(true);
  });

  it('opens on a bare right-click even though WebKit auto-selects the word on the way in', () => {
    show();
    const target = host.querySelector('[data-pick="prose"]')!;
    const before = { isCollapsed: true, rangeCount: 0 } as unknown as Selection;
    const after = {
      isCollapsed: false,
      rangeCount: 1,
      getRangeAt: () => ({ getClientRects: () => [{ left: 10, right: 60, top: 20, bottom: 36 }] }),
    } as unknown as Selection;
    const spy = vi.spyOn(window, 'getSelection').mockReturnValue(before);
    act(() => {
      target.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 2, clientX: 30, clientY: 28 }));
      spy.mockReturnValue(after);
      target.dispatchEvent(new MouseEvent('contextmenu', {
        bubbles: true, cancelable: true, button: 2, clientX: 30, clientY: 28,
      }));
    });

    expect(menu()).not.toBeNull();
  });

  it('defers to the native menu over a selection already under the pointer', () => {
    show();
    const target = host.querySelector('[data-pick="prose"]')!;
    const held = {
      isCollapsed: false,
      rangeCount: 1,
      getRangeAt: () => ({ getClientRects: () => [{ left: 10, right: 60, top: 20, bottom: 36 }] }),
    } as unknown as Selection;
    vi.spyOn(window, 'getSelection').mockReturnValue(held);
    target.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 2, clientX: 30, clientY: 28 }));
    const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: 30, clientY: 28 });
    target.dispatchEvent(e);

    expect(e.defaultPrevented).toBe(false);
    expect(menu()).toBeNull();
  });

  it('opens for a keyboard-invoked menu, not inheriting a stale selection from an earlier, unrelated right-click', () => {
    show();
    const target = host.querySelector('[data-pick="prose"]')!;
    const held = {
      isCollapsed: false,
      rangeCount: 1,
      getRangeAt: () => ({ getClientRects: () => [{ left: 10, right: 60, top: 20, bottom: 36 }] }),
    } as unknown as Selection;
    vi.spyOn(window, 'getSelection').mockReturnValue(held);
    // First, an ordinary right-click lands ON that selection and defers.
    target.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 2, clientX: 30, clientY: 28 }));
    target.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: 30, clientY: 28 }));
    expect(menu()).toBeNull();

    // Then Shift+F10 (or VoiceOver) raises a menu with NO preceding
    // pointerdown. It must not inherit the earlier press's held selection.
    const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    act(() => { target.dispatchEvent(e); });

    expect(menu()).not.toBeNull();
  });

  it('defers to the native menu over a link', () => {
    show();
    const e = rightClick(host.querySelector('[data-pick="link"]')!);
    expect(e.defaultPrevented).toBe(false);
    expect(menu()).toBeNull();
  });

  it('never opens with zero actions, as while the file is being edited', () => {
    filePreviewEditing.value = true;
    show();
    const e = rightClick(host.querySelector('[data-pick="prose"]')!);
    expect(e.defaultPrevented).toBe(false);
    expect(menu()).toBeNull();
  });

  it('installs nothing on the mobile layout', () => {
    show('mobile');
    const e = rightClick(host.querySelector('[data-pick="prose"]')!);
    expect(e.defaultPrevented).toBe(false);
    expect(menu()).toBeNull();
  });
});
