// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
const postClientLog = vi.hoisted(() => vi.fn());
vi.mock('../utils/clientLog', () => ({ postClientLog }));

import { installFocusedFieldVisible } from './useFocusedFieldVisible';

/** The scroll container's box: 600px tall, showing y=100 to y=700. Roughly
 *  `.content-pane-body` under an open mobile keyboard. */
const VIEW_TOP = 100;
const VIEW_HEIGHT = 600;
const FIELD_HEIGHT = 40;

/** A scroll container with one field in it. jsdom runs no layout, so the model
 *  supplies it: the field's box is derived from the container's `scrollTop`, so
 *  a write really does move the field the way a browser would. */
function build(scrollHeight = 1400) {
  const pane = document.createElement('div');
  pane.style.overflowY = 'auto';
  /** Every position the reveal asked for, clamped or not. */
  const writes: number[] = [];
  const maxScroll = Math.max(0, scrollHeight - VIEW_HEIGHT);
  let scrollTop = 0;
  Object.defineProperty(pane, 'scrollTop', {
    get: () => scrollTop,
    set: (v: number) => {
      writes.push(v);
      scrollTop = Math.min(Math.max(0, v), maxScroll);
    },
    configurable: true,
  });
  Object.defineProperty(pane, 'scrollHeight', { value: scrollHeight, configurable: true });
  Object.defineProperty(pane, 'clientHeight', { value: VIEW_HEIGHT, configurable: true });
  Object.defineProperty(pane, 'clientTop', { value: 0, configurable: true });
  pane.getBoundingClientRect = () => ({ top: VIEW_TOP, bottom: VIEW_TOP + VIEW_HEIGHT }) as DOMRect;

  const field = document.createElement('input');
  /** The field's offset inside the scrolled content. */
  let contentTop = 0;
  /** Put the field's box at `top` in the viewport, as it stands right now. */
  const place = (top: number) => { contentTop = top - VIEW_TOP + scrollTop; };
  field.getBoundingClientRect = () => {
    const top = VIEW_TOP + contentTop - scrollTop;
    return ({ top, bottom: top + FIELD_HEIGHT }) as DOMRect;
  };
  place(300);
  pane.appendChild(field);
  document.body.appendChild(pane);
  return { pane, field, place, writes };
}

/** Focus the field the way a tap does, and announce it. jsdom's own `focus()`
 *  may or may not emit `focusin`; a duplicate coalesces into the same frame. */
function focus(field: HTMLElement) {
  field.focus();
  field.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
}

/** jsdom's layout viewport is 768 tall. A visual viewport well under that is
 *  the software keyboard, which is what makes the reveal reserve the strip iOS
 *  floats its accessory bar over. */
const KEYBOARD_UP_HEIGHT = 400;
const KEYBOARD_DOWN_HEIGHT = 768;

describe('the focused-field reveal', () => {
  let frames: FrameRequestCallback[] = [];
  let viewport: EventTarget & { height: number };
  let teardown: () => void;
  /** Drives `nowMs`, so a test can close the settle window on demand. */
  let clock = 0;

  /** Run every frame the reveal has queued, as the browser would before paint.
   *  The settle window re-queues one, so this drains the batch rather than
   *  looping on whatever the batch adds. */
  const flush = () => {
    const due = frames;
    frames = [];
    for (const cb of due) cb(0);
  };
  const resize = () => viewport.dispatchEvent(new Event('resize'));

  beforeEach(() => {
    frames = [];
    clock = 0;
    postClientLog.mockClear();
    vi.spyOn(performance, 'now').mockImplementation(() => clock);
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb));
    vi.stubGlobal('cancelAnimationFrame', () => { frames = []; });
    viewport = Object.assign(new EventTarget(), { height: KEYBOARD_UP_HEIGHT });
    Object.defineProperty(window, 'visualViewport', { value: viewport, configurable: true });
    teardown = installFocusedFieldVisible();
  });

  afterEach(() => {
    teardown();
    document.body.innerHTML = '';
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  /** Let the settle window close, and run the frame that ends the episode. */
  const settleOut = () => {
    clock += 10_000;
    flush();
  };

  it('reports one breadcrumb per keyboard open, at the end of the window', () => {
    const { field, place } = build();
    place(720);
    focus(field);
    flush();
    flush();
    expect(postClientLog).not.toHaveBeenCalled();

    settleOut();
    expect(postClientLog).toHaveBeenCalledTimes(1);
    const [category, message, data] = postClientLog.mock.calls[0];
    expect(category).toBe('mobile');
    expect(message).toBe('focus-reveal');
    // The whole diagnostic in two fields. `want: null` on the LAST frame means
    // the field is placed and nothing more is owed. A number there would mean
    // the reveal ended still asking for an offset it could not reach.
    expect(data).toMatchObject({ want: null, landed: 140, keyboardUp: true });
    // The viewport numbers iOS is known to report wrongly.
    expect(data).toHaveProperty('vvOffsetTop');
    expect(data).toHaveProperty('pageScrollY');
  });

  it('reports nothing for an episode the reader took over', () => {
    const { field, place } = build();
    place(720);
    focus(field);
    document.dispatchEvent(new Event('touchmove'));
    settleOut();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('lifts a field the keyboard pushed out of the shrunk container', () => {
    const { pane, field, place } = build();
    viewport.height = KEYBOARD_DOWN_HEIGHT;
    focus(field);
    flush();
    // Visible when tapped, so the focus frame owes nothing.
    expect(pane.scrollTop).toBe(0);

    // The keyboard opens: the shell shrinks and the field is now below the fold.
    viewport.height = KEYBOARD_UP_HEIGHT;
    place(720);
    resize();
    flush();
    // 60px below the fold, plus the 64px strip the accessory bar covers.
    expect(pane.scrollTop).toBe(140);
  });

  it('reveals a lower field tapped while the keyboard is already up', () => {
    const { pane, field, place } = build();
    place(720);
    focus(field);
    flush();
    expect(pane.scrollTop).toBe(140);
  });

  it('reserves no accessory strip while the keyboard is down', () => {
    const { pane, field, place } = build();
    viewport.height = KEYBOARD_DOWN_HEIGHT;
    place(720);
    focus(field);
    flush();
    expect(pane.scrollTop).toBe(76);
  });

  it('puts the field back when another writer moves it', () => {
    // WebKit runs its own scroll-into-view as the keyboard settles, aiming at a
    // clearance of a few pixels. Whoever writes last wins, which is why the
    // reveal keeps measuring instead of firing once per resize.
    const { pane, field, place } = build();
    place(720);
    focus(field);
    flush();
    expect(pane.scrollTop).toBe(140);

    pane.scrollTop = 76;
    flush();
    expect(pane.scrollTop).toBe(140);
  });

  it('keeps a frame queued through the settle window', () => {
    const { field, place } = build();
    place(720);
    focus(field);
    flush();
    expect(frames).toHaveLength(1);
  });

  it('writes nothing more once the field is placed', () => {
    const { field, place, writes } = build();
    place(720);
    focus(field);
    flush();
    flush();
    flush();
    expect(writes).toEqual([140]);
  });

  it('lands at the limit when the container cannot scroll far enough', () => {
    // Only 100px of scroll exists, so the field can never clear the strip. The
    // DOM clamps the ask, and the position is stable: an unchanged `scrollTop`
    // fires no scroll event, so re-asking each frame moves nobody.
    const { pane, field, place, writes } = build(VIEW_HEIGHT + 100);
    place(720);
    focus(field);
    flush();
    expect(pane.scrollTop).toBe(100);
    flush();
    expect(pane.scrollTop).toBe(100);
    expect(writes.every((w) => w === 140)).toBe(true);
  });

  it('coalesces a burst of resize events into one measurement', () => {
    const { field, place } = build();
    place(720);
    focus(field);
    resize();
    resize();
    expect(frames).toHaveLength(1);
  });

  it('stops revealing once the reader drags the container', () => {
    const { pane, field, place } = build();
    focus(field);
    flush();
    document.dispatchEvent(new Event('touchmove'));
    place(720);
    resize();
    flush();
    expect(pane.scrollTop).toBe(0);
  });

  it('stops revealing when the field loses focus', () => {
    const { pane, field, place } = build();
    focus(field);
    field.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
    place(720);
    resize();
    flush();
    expect(pane.scrollTop).toBe(0);
  });

  it('stops revealing when focus moves to a control with no keyboard', () => {
    const { pane, field, place } = build();
    focus(field);
    const button = pane.appendChild(document.createElement('button'));
    button.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
    place(720);
    resize();
    flush();
    expect(pane.scrollTop).toBe(0);
  });

  it('skips a frame whose field is no longer the focused element', () => {
    // Removing the focused element moves focus to `<body>` without a focusout,
    // in WebKit and Chromium alike, so the frame has to check for itself.
    const { pane, field, place } = build();
    place(720);
    focus(field);
    Object.defineProperty(document, 'activeElement', { value: pane, configurable: true });
    flush();
    // @ts-expect-error: hand the getter back to Document.prototype.
    delete document.activeElement;
    expect(pane.scrollTop).toBe(0);
  });

  it('leaves the container alone after teardown', () => {
    const { pane, field, place } = build();
    teardown();
    place(720);
    focus(field);
    resize();
    flush();
    expect(pane.scrollTop).toBe(0);
  });
});

describe('the reveal writes through the anchor marker', () => {
  // A bare `scrollTop` write is invisible to the scroll consumers. The mobile
  // header would spend the delta as sliding chrome, and the transcript's render
  // window would read one near the top as a request for older turns.
  // Source-scanned because the regression is about WHICH writer is used, and
  // both consumers live outside this module.
  const source = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), 'useFocusedFieldVisible.ts'),
    'utf8',
  );

  it('calls markAnchorScroll', () => {
    expect(source).toMatch(/markAnchorScroll\(container, next\)/);
  });

  it('never assigns scrollTop directly', () => {
    // Assignment only. The module both reads `scrollTop` and compares it, and
    // `=(?!=)` is what keeps a `===` out of the match.
    expect(source).not.toMatch(/\.scrollTop\s*=(?!=)/);
  });

  it('never subtracts visualViewport.offsetTop from a client rect', () => {
    // On iOS WebKit the LAYOUT viewport slides with the visual one for the
    // keyboard. So `getBoundingClientRect` is already in the right frame, and
    // subtracting would double-count by a whole keyboard. Vuetify gates this on
    // a WebKit check for the same reason (vuetifyjs/vuetify#22923). The reveal
    // has no Blink case to serve, so it simply never subtracts.
    expect(source).not.toMatch(/offsetTop/);
  });
});
