// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { installShellScrollGuard, type ShellSnapBack } from './shellScrollGuard';
import { viewportIsMobile } from './viewport';

/** jsdom lays nothing out, so its scroll offsets are always 0 and ignore
 *  writes. Give the box real, writable offsets. */
function scrollable(el: Element, top: number, left: number): void {
  let t = top;
  let l = left;
  Object.defineProperty(el, 'scrollTop', { configurable: true, get: () => t, set: (v: number) => { t = v; } });
  Object.defineProperty(el, 'scrollLeft', { configurable: true, get: () => l, set: (v: number) => { l = v; } });
}

describe('installShellScrollGuard', () => {
  let shell: HTMLElement;
  let stop: () => void;
  let snaps: ShellSnapBack[];

  beforeEach(() => {
    viewportIsMobile.value = false;
    shell = document.createElement('div');
    shell.className = 'app-shell';
    document.body.appendChild(shell);
    snaps = [];
    stop = installShellScrollGuard((s) => snaps.push(s));
  });

  afterEach(() => {
    stop();
    shell.remove();
    viewportIsMobile.value = false;
    vi.unstubAllGlobals();
  });

  it('snaps a scrolled document back to the origin and reports where it was', () => {
    const root = document.scrollingElement ?? document.documentElement;
    scrollable(root, 400, 12);
    document.dispatchEvent(new Event('scroll'));
    expect([root.scrollTop, root.scrollLeft]).toEqual([0, 0]);
    expect(snaps).toEqual([{ box: 'document', top: 400, left: 12 }]);
  });

  it('snaps a scrolled .app-shell back, though its scroll event never bubbles', () => {
    scrollable(shell, 37, 0);
    shell.dispatchEvent(new Event('scroll', { bubbles: false }));
    expect(shell.scrollTop).toBe(0);
    expect(snaps).toEqual([{ box: 'app-shell', top: 37, left: 0 }]);
  });

  it('leaves a pane that scrolls inside the shell alone', () => {
    const pane = document.createElement('div');
    shell.appendChild(pane);
    scrollable(pane, 900, 0);
    pane.dispatchEvent(new Event('scroll', { bubbles: false }));
    expect(pane.scrollTop).toBe(900);
    expect(snaps).toEqual([]);
  });

  it('says nothing about the scroll event its own snap-back causes', () => {
    scrollable(shell, 0, 0);
    shell.dispatchEvent(new Event('scroll', { bubbles: false }));
    expect(snaps).toEqual([]);
  });

  it('leaves the phone layout its own keyboard scroll', () => {
    viewportIsMobile.value = true;
    const root = document.scrollingElement ?? document.documentElement;
    scrollable(root, 300, 0);
    document.dispatchEvent(new Event('scroll'));
    expect(root.scrollTop).toBe(300);
    expect(snaps).toEqual([]);
  });

  it('leaves a finger-driven tablet its keyboard scroll', () => {
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === '(pointer: coarse)' }));
    const root = document.scrollingElement ?? document.documentElement;
    scrollable(root, 300, 0);
    document.dispatchEvent(new Event('scroll'));
    expect(root.scrollTop).toBe(300);
    expect(snaps).toEqual([]);
  });

  it('stops guarding once torn down', () => {
    stop();
    scrollable(shell, 50, 0);
    shell.dispatchEvent(new Event('scroll', { bubbles: false }));
    expect(shell.scrollTop).toBe(50);
    expect(snaps).toEqual([]);
    stop = vi.fn();
  });
});
