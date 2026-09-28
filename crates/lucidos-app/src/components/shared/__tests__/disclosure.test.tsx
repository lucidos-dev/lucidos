// @vitest-environment jsdom
/**
 * `<Disclosure>`: the thread drawer's roll for any block in normal flow.
 * Plan: `docs/plans/2026-09-26-one-disclosure-roll.md`.
 *
 * jsdom has no Web Animations, so `Element.animate` is stubbed. Each stub
 * records its keyframes and timing, and a test finishes it by hand.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { useLayoutEffect } from 'preact/hooks';
import { act } from 'preact/test-utils';

import { Disclosure } from '../Disclosure';
import { motionPreference } from '../../../utils/motion';
import { EASING_DISCLOSURE, FADE_REACH, disclosureDurationMs } from '../../../utils/disclosureMotion';

interface FakeAnimation {
  el: HTMLElement;
  keyframes: Keyframe[];
  timing: KeyframeAnimationOptions;
  cancel: () => void;
  finish: () => void;
  finished: Promise<void>;
}

let anims: FakeAnimation[] = [];
const originalRect = HTMLElement.prototype.getBoundingClientRect;
let host: HTMLElement;
let bodyHeight = 120;

function stubAnimate() {
  anims = [];
  (HTMLElement.prototype as unknown as { animate: unknown }).animate = function (
    this: HTMLElement, keyframes: Keyframe[], timing: KeyframeAnimationOptions,
  ) {
    let resolve!: () => void;
    let reject!: (e: unknown) => void;
    const finished = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
    finished.catch(() => {});
    const anim: FakeAnimation = {
      el: this, keyframes, timing, finished,
      cancel: () => reject(new DOMException('cancelled', 'AbortError')),
      finish: () => resolve(),
    };
    anims.push(anim);
    return anim;
  };
}

/** Where the box sits in the viewport. On screen unless a test moves it. */
let boxTop = 0;

/** jsdom lays nothing out, so the body reports a fixed height and the box a
 *  place in the viewport. `offsetHeight` rounds, as a browser's does. */
function stubLayout() {
  Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
    configurable: true,
    get(this: HTMLElement) { return this.classList.contains('disclosure-body') ? Math.round(bodyHeight) : 0; },
  });
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    const rolls = this.classList.contains('disclosure') || this.classList.contains('disclosure-body');
    const height = rolls ? bodyHeight : 0;
    return { top: boxTop, bottom: boxTop + height, height } as DOMRect;
  };
}

async function show(open: boolean, instant = false) {
  await act(() => {
    render(
      <Disclosure open={open} instant={instant}>
        <div class="row">a</div>
      </Disclosure>,
      host,
    );
  });
}

async function finishAll() {
  await act(async () => {
    for (const a of anims) a.finish();
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  bodyHeight = 120;
  boxTop = 0;
  motionPreference.value = 'full';
  stubAnimate();
  stubLayout();
});

afterEach(() => {
  render(null, host);
  host.remove();
  delete (HTMLElement.prototype as unknown as { animate?: unknown }).animate;
  delete (HTMLElement.prototype as unknown as { offsetHeight?: unknown }).offsetHeight;
  HTMLElement.prototype.getBoundingClientRect = originalRect;
});

describe('Disclosure', () => {
  it('starts every roll of one commit only after all of them have read the layout', async () => {
    // A transcript-wide toggle rolls many rows in one commit. A start between
    // two reads makes the second read lay the whole page out again.
    const log: string[] = [];
    const animate = HTMLElement.prototype.animate;
    HTMLElement.prototype.animate = function (this: HTMLElement, ...args: Parameters<typeof animate>) {
      log.push('start');
      return animate.apply(this, args);
    };
    const rect = HTMLElement.prototype.getBoundingClientRect;
    HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
      if (this.classList.contains('disclosure-body')) log.push('read');
      return rect.call(this);
    };
    const pair = (open: boolean) => (
      <div>
        <Disclosure open={open}><div class="row">a</div></Disclosure>
        <Disclosure open={open}><div class="row">b</div></Disclosure>
      </div>
    );
    await act(() => { render(pair(false), host); });
    await act(() => { render(pair(true), host); });
    expect(log).toEqual(['read', 'read', 'start', 'start', 'start', 'start']);
  });

  it('measures once the commit has settled, after the scroll anchor corrects', async () => {
    // In the commit, rows opening above push this box below the fold. The
    // anchor's correction, queued ahead of the roll, brings it back on screen.
    function Anchor({ tick }: { tick: number }) {
      useLayoutEffect(() => {
        if (tick > 0) queueMicrotask(() => { boxTop = 0; });
      }, [tick]);
      return null;
    }
    const view = (open: boolean, tick: number) => (
      <div>
        <Anchor tick={tick} />
        <Disclosure open={open}><div class="row">a</div></Disclosure>
      </div>
    );
    await act(() => { render(view(false, 0), host); });
    boxTop = window.innerHeight + 10;
    await act(() => { render(view(true, 1), host); });
    expect(anims).toHaveLength(2);
  });

  it('snaps a roll nobody can see', async () => {
    // The turn toggles roll every turn in the transcript at once. Off screen,
    // a roll is work with nothing to show for it.
    await show(false);
    boxTop = window.innerHeight + 10;
    await show(true);
    expect(host.querySelector('.row')).not.toBeNull();
    expect(anims).toHaveLength(0);
    await show(false);
    expect(host.querySelector('.row')).toBeNull();
    expect(anims).toHaveLength(0);
  });

  it('shows its content on the first render without rolling', async () => {
    await show(true);
    expect(host.querySelector('.row')).not.toBeNull();
    expect(anims).toHaveLength(0);
  });

  it('renders nothing while closed', async () => {
    await show(false);
    expect(host.innerHTML).toBe('');
  });

  it('mounts the content in the render that opens it, and rolls it down', async () => {
    await show(false);
    await show(true);
    expect(host.querySelector('.row')).not.toBeNull();
    const outer = anims.find(a => a.el.classList.contains('disclosure'))!;
    const body = anims.find(a => a.el.classList.contains('disclosure-body'))!;
    expect(outer.keyframes.map(k => k.height)).toEqual(['0px', '120px']);
    expect(body.keyframes[0].transform).toBe('translateY(-120px)');
    expect(body.keyframes[1].transform).toBe('translateY(0px)');
    expect(body.keyframes.map(k => k.opacity)).toEqual(['0', '1']);
  });

  it('lands at the fractional height the content lays out at', async () => {
    // A roll to a rounded height snaps by the rounding as it lands, and every
    // row above the pressed control adds its own snap.
    bodyHeight = 120.4;
    await show(false);
    await show(true);
    const opening = anims.find(a => a.el.classList.contains('disclosure'))!;
    expect(opening.keyframes.map(k => k.height)).toEqual(['0px', '120.4px']);
    await finishAll();
    await show(false);
    const closing = anims.filter(a => a.el.classList.contains('disclosure')).pop()!;
    expect(closing.keyframes.map(k => k.height)).toEqual(['120.4px', '0px']);
  });

  it('runs the drawer curve and length, shared by height and roll', async () => {
    await show(false);
    await show(true);
    expect(anims).toHaveLength(2);
    for (const a of anims) {
      expect(a.timing.easing).toBe(EASING_DISCLOSURE);
      expect(a.timing.duration).toBe(disclosureDurationMs(120));
    }
  });

  it('fades under the line only as far as the content has slid under it', async () => {
    await show(false);
    await show(true);
    const opening = anims.find(a => a.el.classList.contains('disclosure'))!;
    expect(opening.keyframes.map(k => k[FADE_REACH])).toEqual(['120px', '0px']);
    await finishAll();
    await show(false);
    const closing = anims.filter(a => a.el.classList.contains('disclosure')).pop()!;
    expect(closing.keyframes.map(k => k[FADE_REACH])).toEqual(['0px', '120px']);
  });

  it('clips only while rolling', async () => {
    await show(false);
    await show(true);
    expect(host.querySelector('.disclosure')!.classList.contains('is-rolling')).toBe(true);
    await finishAll();
    expect(host.querySelector('.disclosure')!.classList.contains('is-rolling')).toBe(false);
  });

  it('keeps the content through its exit, inert, then unmounts it', async () => {
    await show(true);
    await show(false);
    const body = host.querySelector<HTMLElement>('.disclosure-body');
    expect(body).not.toBeNull();
    expect(body!.inert === true || body!.hasAttribute('inert')).toBe(true);
    const outer = anims.find(a => a.el.classList.contains('disclosure'))!;
    expect(outer.keyframes.map(k => k.height)).toEqual(['120px', '0px']);
    await finishAll();
    expect(host.innerHTML).toBe('');
  });

  it('rolls out what it last showed open, even when the closing render empties it', async () => {
    // A folded turn stops computing its rows in the same render that folds it.
    // Rolling out the emptied body would measure 0px and snap.
    await show(true);
    await act(() => {
      render(<Disclosure open={false}>{null}</Disclosure>, host);
    });
    expect(host.querySelector('.row')).not.toBeNull();
    const outer = anims.find(a => a.el.classList.contains('disclosure'))!;
    expect(outer.keyframes.map(k => k.height)).toEqual(['120px', '0px']);
  });

  it('reverses from where a running roll is, without a jump', async () => {
    await show(false);
    await show(true);
    const opening = anims.find(a => a.el.classList.contains('disclosure'))!;
    const outerEl = opening.el;
    outerEl.getBoundingClientRect = () => ({ top: 0, height: 30 } as DOMRect);
    await show(false);
    const closing = anims.filter(a => a.el === outerEl).pop()!;
    expect(closing).not.toBe(opening);
    expect(closing.keyframes[0].height).toBe('30px');
    expect(host.querySelector('.row')).not.toBeNull();
  });

  it('snaps under reduced motion: no roll, and close unmounts at once', async () => {
    motionPreference.value = 'reduce';
    await show(false);
    await show(true);
    expect(host.querySelector('.row')).not.toBeNull();
    await show(false);
    expect(host.innerHTML).toBe('');
    expect(anims).toHaveLength(0);
  });

  it('toggles at once when asked to be instant', async () => {
    await show(false);
    await show(true, true);
    expect(host.querySelector('.row')).not.toBeNull();
    await show(false, true);
    expect(host.innerHTML).toBe('');
    expect(anims).toHaveLength(0);
  });

  it('drops a running roll when an instant toggle interrupts it', async () => {
    await show(true);
    await show(false);
    const exit = anims.find(a => a.el.classList.contains('disclosure'))!;
    let cancelled = false;
    exit.finished.catch(() => { cancelled = true; });
    await show(true, true);
    await Promise.resolve();
    expect(cancelled).toBe(true);
    expect(host.querySelector('.disclosure')!.classList.contains('is-rolling')).toBe(false);
    expect(host.querySelector('.row')).not.toBeNull();
  });

  it('snaps where the browser has no Web Animations', async () => {
    delete (HTMLElement.prototype as unknown as { animate?: unknown }).animate;
    await show(true);
    await show(false);
    expect(host.innerHTML).toBe('');
  });
});
