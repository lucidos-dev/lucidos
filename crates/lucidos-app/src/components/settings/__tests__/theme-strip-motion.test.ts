// @vitest-environment jsdom
/** A family chip re-lays the theme strip out. Every piece moves from where it
 *  was to where it lands, and nothing appears or vanishes in one frame. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { motionPreference } from '../../../utils/motion';
import { playStripMove, snapshotStrip } from '../themeStripMotion';

interface Box { left: number; top: number; width: number; height: number }

const boxes = new Map<Element, Box>();
let played: { target: Element; frames: Keyframe[] }[] = [];
/** Ends every animation played so far, as the browser would at its end. */
let finishAll: () => void = () => {};

function rect({ left, top, width, height }: Box): DOMRect {
  return { left, top, width, height, right: left + width, bottom: top + height, x: left, y: top } as DOMRect;
}

/** A strip 300px wide at the viewport's origin, holding one piece per key. */
function strip(height: number, pieces: Record<string, Box>): HTMLElement {
  const el = document.createElement('div');
  boxes.set(el, { left: 0, top: 0, width: 300, height });
  for (const [key, box] of Object.entries(pieces)) el.append(piece(key, box));
  document.body.append(el);
  return el;
}

function piece(key: string, box: Box): HTMLElement {
  const el = document.createElement('button');
  el.dataset.stripKey = key;
  el.setAttribute('role', 'radio');
  el.style.gridColumn = '1';
  el.textContent = key;
  boxes.set(el, box);
  return el;
}

function keyed(el: HTMLElement, key: string): HTMLElement {
  return el.querySelector<HTMLElement>(`[data-strip-key="${key}"]`)!;
}

function framesFor(target: Element): Keyframe[][] {
  return played.filter(p => p.target === target).map(p => p.frames);
}

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    return rect(boxes.get(this) ?? { left: 0, top: 0, width: 0, height: 0 });
  });
  const ends: (() => void)[] = [];
  finishAll = () => { for (const end of ends) end(); };
  HTMLElement.prototype.animate = vi.fn(function (this: HTMLElement, frames: Keyframe[]) {
    played.push({ target: this, frames });
    let end: () => void = () => {};
    let drop: () => void = () => {};
    const finished = new Promise<void>((resolve, reject) => { end = resolve; drop = () => reject(new Error('cancelled')); });
    ends.push(end);
    return { cancel: vi.fn(drop), finished } as unknown as Animation;
  }) as unknown as typeof HTMLElement.prototype.animate;
});

afterEach(() => {
  vi.restoreAllMocks();
  delete (HTMLElement.prototype as { animate?: unknown }).animate;
  boxes.clear();
  played = [];
  document.body.replaceChildren();
});

it('slides a card that stays from where it was to where it lands', () => {
  const el = strip(200, { 'theme:nord': { left: 150, top: 100, width: 100, height: 90 } });
  const before = snapshotStrip(el);
  boxes.set(keyed(el, 'theme:nord'), { left: 0, top: 10, width: 100, height: 90 });
  boxes.set(el, { left: 0, top: 0, width: 300, height: 100 });
  playStripMove(el, before, () => {});

  expect(framesFor(keyed(el, 'theme:nord'))).toEqual([
    [{ transform: 'translate(150px, 90px)' }, { transform: 'none' }],
  ]);
  expect(framesFor(el)).toEqual([[{ height: '200px' }, { height: '100px' }]]);
});

it('fades in a piece that arrives, a family name included', () => {
  const el = strip(100, {});
  const before = snapshotStrip(el);
  el.append(piece('family:Cool', { left: 0, top: 0, width: 100, height: 16 }));
  playStripMove(el, before, () => {});

  expect(framesFor(keyed(el, 'family:Cool'))).toEqual([[{ opacity: 0 }, { opacity: 1 }]]);
});

it('fades a card in rather than flying it across, when it was scrolled out of sight', () => {
  const el = strip(100, { 'theme:far': { left: 900, top: 0, width: 100, height: 90 } });
  const before = snapshotStrip(el);
  boxes.set(keyed(el, 'theme:far'), { left: 0, top: 0, width: 100, height: 90 });
  playStripMove(el, before, () => {});

  expect(framesFor(keyed(el, 'theme:far'))).toEqual([[{ opacity: 0 }, { opacity: 1 }]]);
});

it('fades a piece that leaves out where it stood, as an inert copy, and clears it on cancel', () => {
  const el = strip(100, { 'family:Warm': { left: 120, top: 4, width: 80, height: 16 } });
  const before = snapshotStrip(el);
  keyed(el, 'family:Warm').remove();
  const cancel = playStripMove(el, before, () => {});

  const ghost = el.querySelector<HTMLElement>('.theme-strip-ghost')!;
  expect(ghost.textContent).toBe('family:Warm');
  expect([ghost.style.left, ghost.style.top, ghost.style.width, ghost.style.height]).toEqual(['120px', '4px', '80px', '16px']);
  // A copy is never a piece, a radio or a grid item of its own.
  expect(ghost.hasAttribute('data-strip-key')).toBe(false);
  expect(ghost.hasAttribute('role')).toBe(false);
  expect(ghost.style.gridColumn).toBe('');
  expect(ghost.inert).toBe(true);
  expect(framesFor(ghost)).toEqual([[{ opacity: 1 }, { opacity: 0 }]]);

  cancel();
  expect(el.querySelector('.theme-strip-ghost')).toBeNull();
});

it('says it settled once the move ends and its copies are gone, and never after a cancel', async () => {
  const pieces = { 'family:Warm': { left: 120, top: 4, width: 80, height: 16 } };
  let el = strip(100, pieces);
  let before = snapshotStrip(el);
  keyed(el, 'family:Warm').remove();
  const settled = vi.fn(() => el.querySelector('.theme-strip-ghost'));
  playStripMove(el, before, settled);
  finishAll();
  await vi.waitFor(() => expect(settled).toHaveBeenCalledOnce());
  expect(settled).toHaveReturnedWith(null);

  el = strip(100, pieces);
  before = snapshotStrip(el);
  keyed(el, 'family:Warm').remove();
  const dropped = vi.fn();
  playStripMove(el, before, dropped)();
  await Promise.resolve();
  await Promise.resolve();
  expect(dropped).not.toHaveBeenCalled();
});

it('leaves no copy of a piece that left from out of sight', () => {
  const el = strip(100, { 'theme:far': { left: 900, top: 0, width: 100, height: 90 } });
  const before = snapshotStrip(el);
  keyed(el, 'theme:far').remove();
  playStripMove(el, before, () => {});

  expect(el.querySelector('.theme-strip-ghost')).toBeNull();
});

it('snaps under reduced motion', () => {
  motionPreference.value = 'reduce';
  try {
    const el = strip(200, { 'theme:nord': { left: 150, top: 100, width: 100, height: 90 } });
    const before = snapshotStrip(el);
    boxes.set(keyed(el, 'theme:nord'), { left: 0, top: 10, width: 100, height: 90 });
    boxes.set(el, { left: 0, top: 0, width: 300, height: 100 });
    playStripMove(el, before, () => {});

    expect(played).toEqual([]);
  } finally {
    motionPreference.value = 'system';
  }
});
