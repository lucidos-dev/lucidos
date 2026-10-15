// @vitest-environment jsdom
/**
 * The bars hook against a DOM, where useHideOnScroll.test.ts covers its pure
 * rules. jsdom has no layout, so every height the hook measures is stubbed.
 *
 * A viewport that leaves the phone layout (a tablet's split, a widened window)
 * swaps App to the desktop layout and back. The header that carries the hook
 * outlives the swap, and the mobile tree does not. Bound to the old tree, the
 * keyboard left the new title bar on screen and the down chevron fell onto
 * Send. e2e/dynamic-bars-survive-rotation-mobile.spec.ts drives the real swap.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { APP_PANEL_BARS_AWAY_ATTR, useHideOnScroll } from '../useHideOnScroll';
import { APP_SCROLL_MESSAGE_TYPE, APP_TOUCH_MESSAGE_TYPE } from '@lucidos/app-scroll';
import { mobileView, preferences } from '../../store/store';
import { viewportIsMobile } from '../../utils/viewport';
import { findSurface } from '../../store/actions/find-bar';

const HEADER_PX = 100;
let headerPx = HEADER_PX;
const TITLE_PX = 36;
const PROMPT_PX = 92;

function Header() {
  const ref = useRef<HTMLElement>(null);
  useHideOnScroll(ref);
  return <header class="app-header" ref={ref} />;
}

/** The parts of a mobile thread pane the hook binds. */
function mountMobileTree(): HTMLElement {
  const wrapper = document.createElement('div');
  wrapper.className = 'mobile-swipe-wrapper';
  wrapper.innerHTML = `
    <div class="mobile-swipe-pane">
      <div class="thread-pane"><div class="thread-pane-body">
        <div class="thread-content-wrap">
          <div class="thread-content visible"><div class="mobile-thread-title-row"></div></div>
          <button class="scroll-to-top"></button>
          <button class="scroll-to-bottom"></button>
        </div>
        <div class="prompt-area"><textarea></textarea></div>
      </div></div>
    </div>`;
  document.body.appendChild(wrapper);
  return wrapper;
}

async function frames(n = 3): Promise<void> {
  for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 60));
}

let host: HTMLElement;
const desktopWidth = window.innerWidth;

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    const height = this.classList.contains('app-header') ? headerPx
      : this.classList.contains('mobile-thread-title-row') ? TITLE_PX
        : this.classList.contains('prompt-area') ? PROMPT_PX : 0;
    return { top: 0, left: 0, right: 0, bottom: height, width: 0, height, x: 0, y: 0, toJSON() {} } as DOMRect;
  });
  headerPx = HEADER_PX;
  // A phone's width, so the hook's own breakpoint read agrees with the signal.
  Object.defineProperty(window, 'innerWidth', { value: 390, configurable: true });
  preferences.value = { status: 'loaded', data: { mobile_dynamic_bars: 'true' } };
  mobileView.value = 'thread';
  viewportIsMobile.value = true;
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  findSurface.value = null;
  document.body.innerHTML = '';
  Object.defineProperty(window, 'innerWidth', { value: desktopWidth, configurable: true });
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('the bars hook across a layout swap', () => {
  it('sends the title bar away for the keyboard, with no swap', async () => {
    const tree = mountMobileTree();
    render(<Header />, host);
    await frames();
    tree.querySelector('textarea')!.focus();
    await frames();
    const title = tree.querySelector<HTMLElement>('.mobile-thread-title-row')!;
    expect(parseFloat(title.style.getPropertyValue('--mobile-header-offset'))).toBeLessThan(0);
  });

  it('binds the mobile tree that comes back, not the one the swap destroyed', async () => {
    mountMobileTree();
    render(<Header />, host);
    await frames();

    viewportIsMobile.value = false;
    document.querySelector('.mobile-swipe-wrapper')!.remove();
    await frames();
    // The desktop layout gets the header with none of the mobile writes.
    expect(document.documentElement.hasAttribute('data-mobile-dynamic-bars')).toBe(false);
    expect(document.documentElement.style.getPropertyValue('--mobile-header-height')).toBe('');
    viewportIsMobile.value = true;
    const fresh = mountMobileTree();
    await frames();

    fresh.querySelector('textarea')!.focus();
    await frames();

    const title = fresh.querySelector<HTMLElement>('.mobile-thread-title-row')!;
    expect(parseFloat(title.style.getPropertyValue('--mobile-header-offset'))).toBeLessThan(0);
    expect(document.documentElement.style.getPropertyValue('--mobile-prompt-height')).toBe(`${PROMPT_PX / 16}rem`);
  });
});

describe('a header that grows while away', () => {
  it('moves by its new height, so none of it peeks in', async () => {
    const tree = mountMobileTree();
    render(<Header />, host);
    await frames();
    tree.querySelector('textarea')!.focus();
    await frames();

    // A banner mounts in the header, or the top inset comes back.
    headerPx = 150;
    window.dispatchEvent(new Event('resize'));
    await frames();

    expect(document.querySelector<HTMLElement>('.app-header')!.style.translate).toBe(`0 ${-150 / 16}rem`);
  });
});

/** A mobile Canvas pane showing an app, plus a widget frame elsewhere. */
function mountAppPane(): { panel: HTMLElement; frame: HTMLIFrameElement; widget: HTMLIFrameElement } {
  const wrapper = document.createElement('div');
  wrapper.className = 'mobile-swipe-wrapper';
  wrapper.innerHTML = `
    <div class="mobile-swipe-pane">
      <div class="content-pane-body has-app-ui">
        <div data-role="app-ui-panel" class="app-ui-inline"><iframe data-role="app-ui-frame"></iframe></div>
      </div>
    </div>
    <div class="mobile-swipe-pane"><iframe data-role="app-ui-frame" class="widget"></iframe></div>`;
  document.body.appendChild(wrapper);
  return {
    panel: wrapper.querySelector<HTMLElement>('[data-role="app-ui-panel"]')!,
    frame: wrapper.querySelector<HTMLIFrameElement>('.app-ui-inline iframe')!,
    widget: wrapper.querySelector<HTMLIFrameElement>('iframe.widget')!,
  };
}

function reportScroll(frame: HTMLIFrameElement, delta: number, top: number) {
  window.dispatchEvent(new MessageEvent('message', {
    data: { type: APP_SCROLL_MESSAGE_TYPE, delta, top, max: 3000 },
    source: frame.contentWindow,
  }));
}

function reportTouch(frame: HTMLIFrameElement, touching: boolean) {
  window.dispatchEvent(new MessageEvent('message', {
    data: { type: APP_TOUCH_MESSAGE_TYPE, touching },
    source: frame.contentWindow,
  }));
}

describe('the bars over an app', () => {
  const headerTranslate = () => document.querySelector<HTMLElement>('.app-header')!.style.translate;

  beforeEach(() => {
    mobileView.value = 'content';
  });

  it("follow the app's reported scroll, and take the panel with them", async () => {
    const { panel, frame } = mountAppPane();
    render(<Header />, host);
    await frames();

    reportScroll(frame, 300, 300);
    expect(headerTranslate()).toBe(`0 ${-HEADER_PX / 16}rem`);
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(true);

    reportScroll(frame, -50, 250);
    expect(headerTranslate()).toBe('');
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(false);
  });

  it('stay shown near the top of the app', async () => {
    const { frame } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportScroll(frame, 60, 60);
    expect(headerTranslate()).toBe('');
  });

  it("ignore a widget frame's scroll", async () => {
    const { panel, widget } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportScroll(widget, 300, 300);
    expect(headerTranslate()).toBe('');
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(false);
  });

  it('stay pinned with dynamic bars off', async () => {
    preferences.value = { status: 'loaded', data: { mobile_dynamic_bars: 'false' } };
    const { panel, frame } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportScroll(frame, 300, 300);
    expect(headerTranslate()).toBe('');
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(false);
  });

  it('come back when the app reloads into a new frame', async () => {
    const { panel, frame } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportScroll(frame, 300, 300);
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(true);

    const reloaded = document.createElement('iframe');
    reloaded.setAttribute('data-role', 'app-ui-frame');
    frame.replaceWith(reloaded);
    await frames();
    expect(headerTranslate()).toBe('');
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(false);
  });

  it('hold still while the finger is on the app, and move once it lifts', async () => {
    const { panel, frame } = mountAppPane();
    render(<Header />, host);
    await frames();

    reportTouch(frame, true);
    reportScroll(frame, 300, 300);
    expect(headerTranslate()).toBe('');
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(false);
    reportTouch(frame, false);
    expect(headerTranslate()).toBe(`0 ${-HEADER_PX / 16}rem`);
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(true);

    reportTouch(frame, true);
    reportScroll(frame, -50, 250);
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(true);
    reportTouch(frame, false);
    expect(headerTranslate()).toBe('');
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(false);
  });

  it("ignore a widget frame's touch", async () => {
    const { panel, frame, widget } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportTouch(widget, true);
    reportScroll(frame, 300, 300);
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(true);
  });

  it('forget the finger when the app reloads into a new frame', async () => {
    const { panel, frame } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportTouch(frame, true);

    const reloaded = document.createElement('iframe');
    reloaded.setAttribute('data-role', 'app-ui-frame');
    frame.replaceWith(reloaded);
    await frames();
    reportScroll(reloaded, 300, 300);
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(true);
  });

  it('forget the finger on a pane swipe', async () => {
    mountMobileTree();
    const { panel, frame } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportTouch(frame, true);

    mobileView.value = 'thread';
    await frames();
    mobileView.value = 'content';
    await frames();
    reportScroll(frame, 300, 300);
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(true);
  });

  it('show the bars when the find bar pins them under a finger', async () => {
    const { panel, frame } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportScroll(frame, 300, 300);
    reportTouch(frame, true);

    findSurface.value = 'content';
    await frames();
    expect(headerTranslate()).toBe('');
    expect(panel.hasAttribute(APP_PANEL_BARS_AWAY_ATTR)).toBe(false);
  });

  it('write the header only when its place changes', async () => {
    const { frame } = mountAppPane();
    render(<Header />, host);
    await frames();
    reportScroll(frame, 300, 300);
    const header = document.querySelector<HTMLElement>('.app-header')!;
    let value = header.style.translate;
    let writes = 0;
    Object.defineProperty(header.style, 'translate', {
      configurable: true,
      get: () => value,
      set: (next: string) => { writes++; value = next; },
    });
    reportScroll(frame, 40, 340);
    reportScroll(frame, 40, 380);
    expect(writes).toBe(0);
    expect(value).toBe(`0 ${-HEADER_PX / 16}rem`);
  });
});
