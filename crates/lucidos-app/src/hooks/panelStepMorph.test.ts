// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { motionPreference } from '../utils/motion';
import { morphKeyframes, startStepMorph, stepSignature, whenStepSettled, type Box } from './panelStepMorph';

/** jsdom lays nothing out, so each box is stubbed. */
function boxed<T extends HTMLElement>(el: T, box: Box): T {
  el.getBoundingClientRect = () => ({
    ...box, x: box.left, y: box.top, right: box.left + box.width, bottom: box.top + box.height, toJSON: () => ({}),
  });
  return el;
}

interface FakeAnimation { cancel: ReturnType<typeof vi.fn>; onfinish: (() => void) | null }

/** Records every `animate` call on `el`, returning animations the test finishes. */
function recordAnimations(el: HTMLElement): { keyframes: Keyframe[][]; animations: FakeAnimation[] } {
  const keyframes: Keyframe[][] = [];
  const animations: FakeAnimation[] = [];
  el.animate = ((frames: Keyframe[]) => {
    keyframes.push(frames);
    const a: FakeAnimation = { cancel: vi.fn(), onfinish: null };
    animations.push(a);
    return a as unknown as Animation;
  }) as HTMLElement['animate'];
  return { keyframes, animations };
}

/** A panel at `to` holding a head and a body, each with its own final size. */
function panelWithChildren(to: Box) {
  const panel = boxed(document.createElement('div'), to);
  const head = boxed(document.createElement('div'), { ...to, height: 40 });
  const body = boxed(document.createElement('div'), { ...to, top: to.top + 40, height: to.height - 40 });
  panel.append(head, body);
  document.body.append(panel);
  return { panel, head, body };
}

afterEach(() => {
  document.body.innerHTML = '';
  motionPreference.value = 'system';
  vi.useRealTimers();
});

describe('morphKeyframes', () => {
  it('keeps the bottom edge of an upward panel on its anchor', () => {
    const from = { left: 60, top: 400, width: 640, height: 200 };
    const to = { left: 100, top: 480, width: 600, height: 120 };
    const [start, end] = morphKeyframes(from, to);
    const startY = Number(String(start.translate).split(' ')[1].replace('px', ''));
    expect(to.top + startY + parseFloat(String(start.height))).toBe(from.top + from.height);
    expect(to.top + parseFloat(String(end.height))).toBe(to.top + to.height);
  });

  it('starts at the old box and ends at rest', () => {
    const [start, end] = morphKeyframes(
      { left: 60, top: 400, width: 640, height: 200 },
      { left: 100, top: 480, width: 600, height: 120 },
    );
    expect(start).toEqual({ width: '640px', height: '200px', translate: '-40px -80px' });
    expect(end).toEqual({ width: '600px', height: '120px', translate: '0px 0px' });
  });
});

describe('stepSignature', () => {
  it('names every step in the panel, outermost first', () => {
    const panel = document.createElement('div');
    panel.innerHTML = '<div data-surface-step="commands-menu"><div data-surface-step="Opus"></div></div>';
    expect(stepSignature(panel)).toBe('commands-menu\nOpus');
  });

  it('is empty for a panel with no steps', () => {
    expect(stepSignature(document.createElement('div'))).toBe('');
  });
});

describe('startStepMorph', () => {
  const to = { left: 100, top: 480, width: 600, height: 120 };
  const from = { left: 60, top: 400, width: 640, height: 200 };

  it('animates the box and fades the content in', () => {
    const { panel, head, body } = panelWithChildren(to);
    const box = recordAnimations(panel);
    const headFade = recordAnimations(head);
    recordAnimations(body);
    startStepMorph(panel, from, () => {});
    expect(box.keyframes).toEqual([morphKeyframes(from, to)]);
    expect(headFade.keyframes).toEqual([[{ opacity: 0 }, { opacity: 1 }]]);
  });

  it('holds each child at its final size, and clips them, until it finishes', () => {
    const { panel, head, body } = panelWithChildren(to);
    body.style.setProperty('height', '5rem');
    const box = recordAnimations(panel);
    recordAnimations(head);
    recordAnimations(body);
    const onSettled = vi.fn();
    startStepMorph(panel, from, onSettled);
    expect(panel.style.overflow).toBe('hidden');
    expect([head.style.width, head.style.height, head.style.flexGrow, head.style.flexShrink])
      .toEqual(['600px', '40px', '0', '0']);
    expect(body.style.height).toBe('80px');

    box.animations[0].onfinish?.();
    expect(onSettled).toHaveBeenCalledTimes(1);
    expect(panel.getAttribute('style') ?? '').toBe('');
    expect(head.getAttribute('style') ?? '').toBe('');
    // A value the child carried before the morph comes back.
    expect(body.style.height).toBe('5rem');
  });

  it('cancels without settling, and leaves nothing behind', () => {
    const { panel, head, body } = panelWithChildren(to);
    const box = recordAnimations(panel);
    recordAnimations(head);
    recordAnimations(body);
    const onSettled = vi.fn();
    startStepMorph(panel, from, onSettled)?.cancel();
    expect(onSettled).not.toHaveBeenCalled();
    expect(box.animations[0].cancel).toHaveBeenCalled();
    expect(head.getAttribute('style') ?? '').toBe('');
    box.animations[0].onfinish?.();
    expect(onSettled).not.toHaveBeenCalled();
  });

  it('settles on its own when the finish event never arrives', () => {
    vi.useFakeTimers();
    const { panel, head, body } = panelWithChildren(to);
    recordAnimations(panel);
    recordAnimations(head);
    recordAnimations(body);
    const onSettled = vi.fn();
    startStepMorph(panel, from, onSettled);
    vi.advanceTimersByTime(1000);
    expect(onSettled).toHaveBeenCalledTimes(1);
    expect(panel.getAttribute('style') ?? '').toBe('');
  });

  it('runs nothing under reduced motion', () => {
    motionPreference.value = 'reduce';
    const { panel } = panelWithChildren(to);
    const box = recordAnimations(panel);
    expect(startStepMorph(panel, from, () => {})).toBeNull();
    expect(box.keyframes).toEqual([]);
    expect(panel.getAttribute('style') ?? '').toBe('');
  });

  it('runs nothing without Web Animations', () => {
    const { panel } = panelWithChildren(to);
    (panel as { animate?: unknown }).animate = undefined;
    expect(startStepMorph(panel, from, () => {})).toBeNull();
  });
});

describe('whenStepSettled', () => {
  const to = { left: 100, top: 480, width: 600, height: 120 };
  const from = { left: 60, top: 400, width: 640, height: 200 };

  function morphing() {
    const parts = panelWithChildren(to);
    const box = recordAnimations(parts.panel);
    recordAnimations(parts.head);
    recordAnimations(parts.body);
    const row = document.createElement('div');
    parts.body.append(row);
    return { ...parts, box, row };
  }

  it('runs at once when no morph is running', () => {
    const { body } = panelWithChildren(to);
    const fn = vi.fn();
    whenStepSettled(body, fn);
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it('holds work inside a morphing panel until the morph finishes', () => {
    const { panel, box, row } = morphing();
    startStepMorph(panel, from, () => {});
    const fn = vi.fn();
    whenStepSettled(row, fn);
    expect(fn).not.toHaveBeenCalled();
    box.animations[0].onfinish?.();
    expect(fn).toHaveBeenCalledTimes(1);
    // The pins are gone by then, so the work measures the box at rest.
    expect(panel.getAttribute('style') ?? '').toBe('');
  });

  it('runs held work when the morph is cancelled', () => {
    const { panel, row } = morphing();
    const morph = startStepMorph(panel, from, () => {});
    const fn = vi.fn();
    whenStepSettled(row, fn);
    morph?.cancel();
    expect(fn).toHaveBeenCalledTimes(1);
  });
});
