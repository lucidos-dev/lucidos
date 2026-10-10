// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { UI_SCALE_STEP, currentUiScale } from '../../../store/actions/preferences';
import { installScaleWheel } from '../scaleWheelListener';
import { scaleModalOpen, previewScale, _resetScaleTimersForTesting } from '../scaleModalState';

vi.mock('../../../api/client', () => ({
  getPreferences: vi.fn(),
  setPreference: vi.fn().mockResolvedValue(undefined),
  isTransientFetchError: () => false,
}));

let frames: FrameRequestCallback[] = [];

function flushFrame(): void {
  const due = frames;
  frames = [];
  for (const cb of due) cb(0);
}

function wheel(init: WheelEventInit, target: EventTarget = document.body): WheelEvent {
  const e = new WheelEvent('wheel', { bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(e);
  return e;
}

function key(type: 'keydown' | 'keyup', init: KeyboardEventInit): void {
  window.dispatchEvent(new KeyboardEvent(type, init));
}

/**
 * Cmd/Ctrl + wheel drives the UI scale with the panel closed too, and the
 * cancelling listener exists only while a wheel can be a zoom.
 */
describe('Cmd/Ctrl + wheel steps the UI scale', () => {
  let teardown: () => void;

  beforeEach(() => {
    frames = [];
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb));
    vi.stubGlobal('cancelAnimationFrame', () => {});
    teardown = installScaleWheel();
  });

  afterEach(() => {
    teardown();
    _resetScaleTimersForTesting();
    scaleModalOpen.value = false;
    vi.unstubAllGlobals();
  });

  it('opens the panel and steps from closed while Cmd is held', () => {
    const before = currentUiScale();
    key('keydown', { key: 'Meta', metaKey: true });
    const notch = wheel({ deltaY: -100, metaKey: true });
    expect(notch.defaultPrevented).toBe(true);
    flushFrame();
    expect(scaleModalOpen.value).toBe(true);
    expect(previewScale.value).toBe(before + UI_SCALE_STEP);
  });

  it('works with Ctrl as well as Cmd', () => {
    key('keydown', { key: 'Control', ctrlKey: true });
    expect(wheel({ deltaY: 100, ctrlKey: true }).defaultPrevented).toBe(true);
  });

  it('leaves a plain scroll alone', () => {
    expect(wheel({ deltaY: 100 }).defaultPrevented).toBe(false);
  });

  it('is not listening at all with no modifier held and the panel closed', () => {
    // Chrome's trackpad pinch: ctrl-wheel with no keydown. The browser keeps it.
    expect(wheel({ deltaY: -8, ctrlKey: true }).defaultPrevented).toBe(false);
  });

  it('stops listening when the modifier is released', () => {
    key('keydown', { key: 'Meta', metaKey: true });
    key('keyup', { key: 'Meta' });
    expect(wheel({ deltaY: -100, metaKey: true }).defaultPrevented).toBe(false);
  });

  it('stops listening when the window loses focus with the modifier down', () => {
    key('keydown', { key: 'Meta', metaKey: true });
    window.dispatchEvent(new Event('blur'));
    expect(wheel({ deltaY: -100, metaKey: true }).defaultPrevented).toBe(false);
  });

  it('heals a missed keyup on the next unmodified wheel', () => {
    key('keydown', { key: 'Meta', metaKey: true });
    wheel({ deltaY: 100 });
    expect(wheel({ deltaY: -100, metaKey: true }).defaultPrevented).toBe(false);
  });

  it('hears a pinch the moment the panel opens', () => {
    scaleModalOpen.value = true;
    expect(wheel({ deltaY: -8, ctrlKey: true }).defaultPrevented).toBe(true);
  });

  it('a sub-notch pinch, with no key down, does not step', () => {
    scaleModalOpen.value = true;
    const before = previewScale.value;
    for (let i = 0; i < 20; i++) wheel({ deltaY: -4, ctrlKey: true });
    flushFrame();
    expect(previewScale.value).toBe(before);
  });

  it('a slow Cmd tick steps even when the window missed the keydown', () => {
    // Cmd held across a window switch, or pressed while an app iframe had focus.
    scaleModalOpen.value = true;
    const before = previewScale.value;
    wheel({ deltaY: -4, metaKey: true });
    flushFrame();
    expect(previewScale.value).toBe(before + UI_SCALE_STEP);
  });

  it('a slow tick with the key held steps once', () => {
    const before = currentUiScale();
    key('keydown', { key: 'Control', ctrlKey: true });
    wheel({ deltaY: -4, ctrlKey: true });
    flushFrame();
    expect(previewScale.value).toBe(before + UI_SCALE_STEP);
  });

  it('leaves a wheel another surface already claimed', () => {
    key('keydown', { key: 'Meta', metaKey: true });
    // The image popup's strip zooms its image on any wheel.
    const strip = document.body.appendChild(document.createElement('div'));
    strip.addEventListener('wheel', e => e.preventDefault());
    wheel({ deltaY: -100, metaKey: true }, strip);
    strip.remove();
    flushFrame();
    expect(scaleModalOpen.value).toBe(false);
  });

  it('drops the rest of a burst once the panel closes', () => {
    key('keydown', { key: 'Control', ctrlKey: true });
    for (let i = 0; i < 3; i++) wheel({ deltaY: -100, ctrlKey: true });
    flushFrame();
    expect(scaleModalOpen.value).toBe(true);
    // The release, then the dismiss it triggers, with two notches still banked.
    key('keyup', { key: 'Control' });
    scaleModalOpen.value = false;
    flushFrame();
    flushFrame();
    expect(scaleModalOpen.value).toBe(false);
  });

  it('drops the rest of a burst on Escape with the modifier still held', () => {
    key('keydown', { key: 'Control', ctrlKey: true });
    for (let i = 0; i < 3; i++) wheel({ deltaY: -100, ctrlKey: true });
    flushFrame();
    scaleModalOpen.value = false;
    flushFrame();
    flushFrame();
    expect(scaleModalOpen.value).toBe(false);
  });

  it('drops a step still queued when focus leaves before its frame', () => {
    key('keydown', { key: 'Meta', metaKey: true });
    wheel({ deltaY: -100, metaKey: true });
    window.dispatchEvent(new Event('blur'));
    flushFrame();
    expect(scaleModalOpen.value).toBe(false);
  });

  it('removes everything on teardown', () => {
    key('keydown', { key: 'Meta', metaKey: true });
    teardown();
    expect(wheel({ deltaY: -100, metaKey: true }).defaultPrevented).toBe(false);
    teardown = () => {};
  });
});
