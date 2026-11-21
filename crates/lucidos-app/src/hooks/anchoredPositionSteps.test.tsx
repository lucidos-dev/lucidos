// @vitest-environment jsdom
/**
 * `useAnchoredPosition` against a panel whose content changes while open: the
 * position lands before the next frame, and a step change glides.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { useAnchoredPosition, type AnchorBox, type AnchorPosition } from './useAnchoredPopover';

/** An anchor near the viewport's bottom, so the panel opens upward and its
 *  `top` depends on its height. */
const anchor: AnchorBox = {
  getBoundingClientRect: () => ({
    left: 50, right: 80, top: 700, bottom: 720, width: 30, height: 20, x: 50, y: 700, toJSON: () => ({}),
  }),
};

/** The box a running morph draws, which jsdom cannot animate. */
let animatedBox: { top: number; height: number } | null = null;

/** jsdom lays nothing out, so a panel's height is its `data-h`, and its top is
 *  the `top` the hook wrote. */
function layOut(panel: HTMLElement): void {
  Object.defineProperty(panel, 'offsetHeight', { configurable: true, get: () => Number(panel.dataset.h) });
  Object.defineProperty(panel, 'offsetWidth', { configurable: true, get: () => 200 });
  panel.getBoundingClientRect = () => {
    if (animatedBox) {
      const { top, height } = animatedBox;
      return { left: 50, right: 250, top, bottom: top + height, width: 200, height, x: 50, y: top, toJSON: () => ({}) };
    }
    const top = parseFloat(panel.style.top || '0');
    const height = Number(panel.dataset.h);
    return { left: 50, right: 250, top, bottom: top + height, width: 200, height, x: 50, y: top, toJSON: () => ({}) };
  };
}

let positions: (AnchorPosition | null)[] = [];
let frames: FrameRequestCallback[] = [];

const lastTop = () => positions[positions.length - 1]?.top;

function Host({ step, height }: { step: string; height: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(anchor, ref);
  positions.push(pos);
  return (
    <div
      ref={(el) => { if (el) layOut(el); ref.current = el; }}
      data-h={height}
      style={pos ? { top: `${pos.top}px` } : undefined}
    >
      {/* The content is what changes, and the height follows it. */}
      <div data-surface-step={step}>{`${step} ${height}`}</div>
    </div>
  );
}

const flushMicrotasks = () => new Promise<void>((resolve) => queueMicrotask(resolve));

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await flushMicrotasks();
}

let root: HTMLElement;

/** Records every animation the panel and its children start. */
function recordAnimations(): { el: Element; frames: Keyframe[]; animation: { onfinish: (() => void) | null } }[] {
  const calls: { el: Element; frames: Keyframe[]; animation: { onfinish: (() => void) | null } }[] = [];
  HTMLElement.prototype.animate = function (this: Element, kf: Keyframe[]) {
    const animation = { cancel: () => {}, onfinish: null };
    calls.push({ el: this, frames: kf, animation });
    return animation as unknown as Animation;
  } as HTMLElement['animate'];
  return calls;
}

const runFrames = () => { for (const cb of frames.splice(0)) cb(0); };

beforeEach(() => {
  animatedBox = null;
  positions = [];
  frames = [];
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb));
  vi.stubGlobal('cancelAnimationFrame', () => {});
  root = document.createElement('div');
  document.body.append(root);
});

afterEach(() => {
  render(null, root);
  document.body.innerHTML = '';
  vi.unstubAllGlobals();
  delete (HTMLElement.prototype as { animate?: unknown }).animate;
});

describe('useAnchoredPosition when the content changes', () => {
  it('places the new content before the next frame', async () => {
    render(<Host step="list" height={100} />, root);
    await settle();
    expect(lastTop()).toBe(700 - 4 - 100);

    render(<Host step="list" height={200} />, root);
    await settle();
    // No frame has run, so only the mutation callback can have re-measured.
    expect(lastTop()).toBe(700 - 4 - 200);
  });

  it('glides a step change from the box as it was drawn', async () => {
    const calls: { el: Element; frames: Keyframe[] }[] = [];
    HTMLElement.prototype.animate = function (this: Element, kf: Keyframe[]) {
      calls.push({ el: this, frames: kf });
      return { cancel: () => {}, onfinish: null } as unknown as Animation;
    } as HTMLElement['animate'];

    render(<Host step="list" height={100} />, root);
    await settle();
    render(<Host step="condition" height={200} />, root);
    await settle();
    expect(calls).toEqual([]);

    for (const cb of frames.splice(0)) cb(0);
    const panel = root.firstElementChild!;
    const box = calls.find((c) => c.el === panel);
    expect(box?.frames[0]).toEqual({ width: '200px', height: '100px', translate: '0px 100px' });
    expect(box?.frames[1]).toEqual({ width: '200px', height: '200px', translate: '0px 0px' });
  });

  it('holds its position while a morph runs, and settles after', async () => {
    const calls = recordAnimations();
    render(<Host step="list" height={100} />, root);
    await settle();
    render(<Host step="condition" height={200} />, root);
    await settle();
    runFrames();
    const box = calls.find((c) => c.el === root.firstElementChild);

    render(<Host step="condition" height={300} />, root);
    await settle();
    runFrames();
    expect(lastTop()).toBe(700 - 4 - 200);

    box?.animation.onfinish?.();
    await settle();
    expect(lastTop()).toBe(700 - 4 - 300);
  });

  it('starts a step change mid-morph from the box as drawn', async () => {
    const calls = recordAnimations();
    render(<Host step="list" height={100} />, root);
    await settle();
    render(<Host step="condition" height={200} />, root);
    await settle();
    runFrames();

    animatedBox = { top: 546, height: 150 };
    render(<Host step="list" height={120} />, root);
    await settle();
    animatedBox = null;
    runFrames();
    const boxes = calls.filter((c) => c.el === root.firstElementChild);
    expect(boxes).toHaveLength(2);
    // The new step rests at top 576, so the glide starts 30px above it.
    expect(boxes[1].frames[0]).toEqual({ width: '200px', height: '150px', translate: '0px -30px' });
  });

  it('does not glide a change within one step', async () => {
    const animate = vi.fn();
    HTMLElement.prototype.animate = animate as unknown as HTMLElement['animate'];
    render(<Host step="list" height={100} />, root);
    await settle();
    render(<Host step="list" height={200} />, root);
    await settle();
    for (const cb of frames.splice(0)) cb(0);
    expect(animate).not.toHaveBeenCalled();
  });
});
