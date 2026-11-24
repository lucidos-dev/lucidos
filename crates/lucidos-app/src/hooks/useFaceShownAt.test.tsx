// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { useFaceShownAt } from './useTouchActivated';

let host: HTMLDivElement;
let shownAt: (() => number | null) | null = null;
let frames: FrameRequestCallback[] = [];

function Probe({ face }: { face: string }) {
  shownAt = useFaceShownAt(face);
  return null;
}

function draw(face: string): void {
  act(() => { render(<Probe face={face} />, host); });
}

function paint(t: number): void {
  const due = frames;
  frames = [];
  for (const cb of due) cb(t - 50);
  vi.spyOn(performance, 'now').mockReturnValue(t);
  vi.runAllTimers();
}

beforeEach(() => {
  vi.useFakeTimers();
  host = document.createElement('div');
  document.body.appendChild(host);
  frames = [];
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb));
  vi.stubGlobal('cancelAnimationFrame', () => {});
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('useFaceShownAt', () => {
  it('is null until the frame that paints the face', () => {
    draw('send');
    expect(shownAt!()).toBeNull();
    paint(100);
    expect(shownAt!()).toBe(100);
  });

  it('stamps after the paint, not at the start of the frame', () => {
    draw('cancel');
    const due = frames;
    frames = [];
    for (const cb of due) cb(100);
    expect(shownAt!()).toBeNull();
    vi.spyOn(performance, 'now').mockReturnValue(1_400);
    vi.runAllTimers();
    expect(shownAt!()).toBe(1_400);
  });

  it('starts over when the face changes, and keeps one getter', () => {
    draw('send');
    paint(100);
    const getter = shownAt;
    draw('cancel');
    expect(shownAt).toBe(getter);
    expect(shownAt!()).toBeNull();
    paint(900);
    expect(shownAt!()).toBe(900);
  });

  it('keeps the paint time across a render that shows the same face', () => {
    draw('cancel');
    paint(900);
    draw('cancel');
    expect(shownAt!()).toBe(900);
  });
});
