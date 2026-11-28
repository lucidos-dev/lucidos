// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { h, render } from 'preact';
import { paneCentre, paneOfFocus, paneUnder, usePaneCentre, type Pane } from './usePaneCentre';

function boxed<T extends HTMLElement>(el: T, left: number, width: number): T {
  el.getBoundingClientRect = () => ({
    left, right: left + width, width, top: 0, bottom: 40, height: 40, x: left, y: 0, toJSON: () => ({}),
  });
  return el;
}

function mountSplit(conversationWidth = 600): void {
  const split = document.createElement('div');
  split.className = 'split-layout';
  const thread = boxed(document.createElement('div'), 300, conversationWidth);
  thread.className = 'pane pane-thread';
  const content = boxed(document.createElement('div'), 300 + conversationWidth, 1300 - conversationWidth);
  content.className = 'pane pane-content';
  split.append(thread, content);
  document.body.append(split);
}

function buttonAt(left: number): HTMLElement {
  return boxed(document.createElement('button'), left, 20);
}

afterEach(() => {
  document.body.innerHTML = '';
  vi.unstubAllGlobals();
});

describe('paneUnder', () => {
  it('names the pane whose column holds the button', () => {
    mountSplit();
    expect(paneUnder(buttonAt(800))).toBe('conversation');
    expect(paneUnder(buttonAt(1500))).toBe('canvas');
  });

  it('answers nothing for a button over no pane, or no button at all', () => {
    mountSplit();
    expect(paneUnder(buttonAt(100))).toBeUndefined();
    expect(paneUnder(null)).toBeUndefined();
  });

  it('skips a collapsed pane, so a button never lands on a zero-width column', () => {
    mountSplit(0);
    expect(paneUnder(buttonAt(270))).toBeUndefined();
    expect(paneUnder(buttonAt(1000))).toBe('canvas');
  });

  it('answers nothing on mobile, where there is no split', () => {
    expect(paneUnder(buttonAt(100))).toBeUndefined();
  });
});

describe('paneCentre', () => {
  it('measures the middle of the named pane', () => {
    mountSplit();
    expect(paneCentre('conversation')).toBe(600);
    expect(paneCentre('canvas')).toBe(1250);
  });

  it('is undefined for a collapsed pane', () => {
    mountSplit(0);
    expect(paneCentre('conversation')).toBeUndefined();
  });
});

describe('paneOfFocus', () => {
  it('maps the drawer to the Conversation pane it sits beside', () => {
    expect(paneOfFocus('drawer')).toBe('conversation');
    expect(paneOfFocus('thread')).toBe('conversation');
    expect(paneOfFocus('content')).toBe('canvas');
  });
});

describe('usePaneCentre', () => {
  function stubResizeObserver(): { resize: () => void; disconnected: () => boolean } {
    let callback: () => void = () => {};
    let gone = false;
    vi.stubGlobal('ResizeObserver', class {
      constructor(cb: () => void) { callback = cb; }
      observe() {}
      disconnect() { gone = true; }
    });
    return { resize: () => callback(), disconnected: () => gone };
  }

  function mountHook(pane: Pane | undefined): { centre: () => string; fit: () => string; host: HTMLElement } {
    const host = document.createElement('div');
    document.body.append(host);
    let style: ReturnType<typeof usePaneCentre>;
    function Probe() {
      style = usePaneCentre(pane);
      return null;
    }
    render(h(Probe, null), host);
    const read = (name: string) => String(style?.[name as keyof typeof style] ?? '');
    return { centre: () => read('--pane-centre-x'), fit: () => read('--pane-fit'), host };
  }

  it('re-measures when a pane resizes without the window resizing', async () => {
    const observer = stubResizeObserver();
    mountSplit();
    const { centre, fit } = mountHook('conversation');
    expect(centre()).toBe('600px');
    expect(fit()).toBe('calc(600px - 2rem)');

    boxed(document.querySelector<HTMLElement>('.pane-thread')!, 300, 800);
    observer.resize();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(centre()).toBe('700px');
  });

  it('publishes nothing and watches nothing while closed', () => {
    const observer = stubResizeObserver();
    mountSplit();
    const { centre, fit, host } = mountHook(undefined);
    expect(centre()).toBe('');
    expect(fit()).toBe('');
    render(null, host);
    expect(observer.disconnected()).toBe(false);
  });
});
