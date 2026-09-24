// @vitest-environment jsdom
/**
 * The changed-files sidebar in the repo file preview resizes from its divider:
 * by pointer drag, by arrow keys, and back to the default on double-click.
 *
 * jsdom has no layout, so the container's width is stubbed and the observer is
 * a no-op. What is under test is the wiring: which width each gesture lands on,
 * and that it persists.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { RepoPreviewSplit } from '../RepoPreviewSplit';
import {
  repoPreviewSidebarRem, SIDEBAR_DEFAULT_REM, SIDEBAR_MIN_REM, DIFF_PANE_MIN_REM,
  SIDEBAR_KEY_STEP_REM, SIDEBAR_WIDTH_KEY,
} from '../repoPreviewSidebarWidth';

const REM = 16;
const CONTAINER_REM = 80;

class NoopObserver {
  observe() {}
  disconnect() {}
}

let host: HTMLDivElement;

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopObserver);
  localStorage.clear();
  repoPreviewSidebarRem.value = SIDEBAR_DEFAULT_REM;
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const width = this.classList.contains('repo-preview-split') ? CONTAINER_REM * REM : 0;
    return { left: 0, top: 0, right: width, bottom: 0, width, height: 0, x: 0, y: 0, toJSON() {} } as DOMRect;
  });
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => {
    render(<RepoPreviewSplit sidebar={<p>files</p>} main={<p>diff</p>} />, host);
  });
});

afterEach(() => {
  act(() => render(null, host));
  host.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function divider(): HTMLElement {
  const el = host.querySelector<HTMLElement>('.repo-preview-split-divider');
  expect(el, 'no divider rendered').not.toBeNull();
  return el as HTMLElement;
}

function sidebarBasis(): string {
  return host.querySelector<HTMLElement>('.repo-preview-split-sidebar')!.style.flexBasis;
}

function pointer(type: string, clientX: number): MouseEvent {
  const e = new MouseEvent(type, { bubbles: true, cancelable: true, clientX, button: 0 });
  Object.defineProperty(e, 'pointerId', { value: 1 });
  return e;
}

function key(k: string): void {
  act(() => {
    divider().dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }));
  });
}

describe('the divider', () => {
  it('is an accessible vertical separator the keyboard can reach', () => {
    const el = divider();
    expect(el.getAttribute('role')).toBe('separator');
    expect(el.getAttribute('aria-orientation')).toBe('vertical');
    expect(el.getAttribute('aria-label')).toBeTruthy();
    expect(el.tabIndex).toBe(0);
    expect(el.getAttribute('aria-valuenow')).toBe(String(SIDEBAR_DEFAULT_REM));
    expect(el.getAttribute('aria-valuemin')).toBe(String(SIDEBAR_MIN_REM));
    expect(el.getAttribute('aria-valuemax')).toBe(String(CONTAINER_REM - DIFF_PANE_MIN_REM));
  });

  it('sizes the sidebar in rem', () => {
    expect(sidebarBasis()).toBe(`${SIDEBAR_DEFAULT_REM}rem`);
  });
});

describe('keyboard resize', () => {
  it('nudges the width with the arrow keys and persists it', () => {
    key('ArrowRight');
    expect(repoPreviewSidebarRem.value).toBe(SIDEBAR_DEFAULT_REM + SIDEBAR_KEY_STEP_REM);
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBe(String(SIDEBAR_DEFAULT_REM + SIDEBAR_KEY_STEP_REM));
    key('ArrowLeft');
    key('ArrowLeft');
    expect(repoPreviewSidebarRem.value).toBe(SIDEBAR_DEFAULT_REM - SIDEBAR_KEY_STEP_REM);
  });

  it('jumps to the bounds with Home and End', () => {
    key('End');
    expect(repoPreviewSidebarRem.value).toBe(CONTAINER_REM - DIFF_PANE_MIN_REM);
    key('Home');
    expect(repoPreviewSidebarRem.value).toBe(SIDEBAR_MIN_REM);
  });

  it('never steps below the minimum', () => {
    key('Home');
    key('ArrowLeft');
    expect(repoPreviewSidebarRem.value).toBe(SIDEBAR_MIN_REM);
  });
});

describe('pointer drag', () => {
  it('tracks the pointer live, clamps it, and persists on release', () => {
    const el = divider();
    const capture = vi.fn();
    el.setPointerCapture = capture;
    el.releasePointerCapture = vi.fn();

    act(() => { el.dispatchEvent(pointer('pointerdown', SIDEBAR_DEFAULT_REM * REM)); });
    expect(capture).toHaveBeenCalledWith(1);
    expect(document.body.style.cursor).toBe('col-resize');
    expect(document.body.style.userSelect).toBe('none');

    act(() => { el.dispatchEvent(pointer('pointermove', 30 * REM)); });
    expect(repoPreviewSidebarRem.value).toBe(30);
    expect(sidebarBasis()).toBe('30rem');
    // Nothing persists mid-drag.
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBeNull();

    act(() => { el.dispatchEvent(pointer('pointermove', 2000 * REM)); });
    expect(repoPreviewSidebarRem.value).toBe(CONTAINER_REM - DIFF_PANE_MIN_REM);

    act(() => { el.dispatchEvent(pointer('pointerup', 2000 * REM)); });
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBe(String(CONTAINER_REM - DIFF_PANE_MIN_REM));
    expect(document.body.style.cursor).toBe('');
    expect(document.body.style.userSelect).toBe('');
  });

  it('restores the body styles if the split unmounts mid-drag', () => {
    const el = divider();
    el.setPointerCapture = vi.fn();
    act(() => { el.dispatchEvent(pointer('pointerdown', 100)); });
    act(() => render(null, host));
    expect(document.body.style.cursor).toBe('');
    expect(document.body.style.userSelect).toBe('');
  });
});

describe('double-click', () => {
  it('resets to the default width', () => {
    key('End');
    act(() => {
      divider().dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    });
    expect(repoPreviewSidebarRem.value).toBe(SIDEBAR_DEFAULT_REM);
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBe(String(SIDEBAR_DEFAULT_REM));
  });
});
