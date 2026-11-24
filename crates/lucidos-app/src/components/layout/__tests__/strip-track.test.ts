// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { RUBBER_BAND, stripTrack } from '../MobileSwipeContainer';
import { mobileView, PANE_INDEX } from '../../../store/store';

const WIDTH = 300;

function strip(transform: string) {
  const container = document.createElement('div');
  Object.defineProperty(container, 'offsetWidth', { value: WIDTH });
  const track = document.createElement('div');
  track.style.transform = transform;
  container.appendChild(track);
  document.body.appendChild(container);
  let held = false;
  const t = stripTrack({ current: container }, { current: track }, () => held);
  return { container, track, t, hold: (v: boolean) => { held = v; } };
}

afterEach(() => {
  document.body.innerHTML = '';
  vi.unstubAllGlobals();
});

describe('stripTrack', () => {
  it('catches a drag where the strip is on screen, mid-slide included', () => {
    const { track, t } = strip('matrix(1, 0, 0, 1, -150, 0)');
    t.grab();
    t.follow(4);
    expect(track.style.transition).toBe('none');
    expect(track.style.transform).toBe('translateX(-146px)');
  });

  it('rubber-bands a held drag past the first pane', () => {
    const { track, t } = strip('matrix(1, 0, 0, 1, 0, 0)');
    t.grab();
    t.follow(30);
    expect(track.style.transform).toBe(`translateX(${30 * RUBBER_BAND}px)`);
  });

  it('rests on the visible pane, resetting the scroll offset', () => {
    const { container, track, t } = strip('matrix(1, 0, 0, 1, -40, 0)');
    container.scrollLeft = 25;
    t.rest(true);
    expect(container.scrollLeft).toBe(0);
    expect(track.style.transform).toBe(`translateX(${-PANE_INDEX[mobileView.value] * WIDTH}px)`);
    expect(track.style.transition).toBe('');
  });

  it('turns a snapped transition back on next frame, unless a drag holds the strip', () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frames.push(cb); return frames.length; });
    const { track, t, hold } = strip('none');

    t.rest(false);
    expect(track.style.transition).toBe('none');
    hold(true);
    frames.shift()!(0);
    expect(track.style.transition).toBe('none');

    hold(false);
    t.rest(false);
    frames.shift()!(0);
    expect(track.style.transition).toBe('');
  });
});
