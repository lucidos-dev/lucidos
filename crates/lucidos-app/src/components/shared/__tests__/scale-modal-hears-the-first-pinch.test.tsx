// @vitest-environment jsdom
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { ScaleModal } from '../ScaleModal';
import { scaleModalOpen, previewScale, _resetScaleTimersForTesting } from '../scaleModalState';

vi.mock('../../../api/client', () => ({
  getPreferences: vi.fn(),
  setPreference: vi.fn().mockResolvedValue(undefined),
  isTransientFetchError: () => false,
}));

/**
 * The panel mounts only while it is open, so its wheel listener has to exist by
 * the time it is drawn. A passive effect runs after paint, and a pinch landing
 * in that gap goes to the browser's own zoom.
 */
describe('the scale panel hears a pinch the moment it is drawn', () => {
  afterEach(() => {
    _resetScaleTimersForTesting();
    scaleModalOpen.value = false;
  });

  it('takes a ctrl-wheel dispatched right after its render', () => {
    scaleModalOpen.value = true;
    previewScale.value = 100;
    const host = document.createElement('div');
    document.body.appendChild(host);
    render(<ScaleModal />, host);

    const pinch = new WheelEvent('wheel', { deltaY: -8, ctrlKey: true, bubbles: true, cancelable: true });
    document.dispatchEvent(pinch);

    expect(pinch.defaultPrevented).toBe(true);
    render(null, host);
    host.remove();
  });
});
