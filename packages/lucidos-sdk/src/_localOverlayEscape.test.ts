// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { pushOverlay, _resetOverlayStackForTesting } from '../../../crates/lucidos-app/src/store/overlayStack';
import { consumeLocalOverlayEscape, installLocalOverlayEscape } from './_localOverlayEscape';
afterEach(_resetOverlayStackForTesting);
describe('SDK local overlay Escape ownership', () => {
  it('dismisses one stack entry before host forwarding and consumes the event once', () => {
    const off = installLocalOverlayEscape();
    const dismiss = vi.fn();
    pushOverlay({ id: 'sdk-test', hasPanel: true, dismiss });
    const event = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true });
    expect(consumeLocalOverlayEscape(event)).toBe(true);
    expect(consumeLocalOverlayEscape(event)).toBe(true);
    expect(event.defaultPrevented).toBe(true);
    expect(dismiss).toHaveBeenCalledTimes(1);
    off();
  });
  it('releases the window listener only after the last instance is destroyed', () => {
    const offA = installLocalOverlayEscape();
    const offB = installLocalOverlayEscape();
    const dismiss = vi.fn();
    pushOverlay({ id: 'sdk-test', hasPanel: true, dismiss });
    offA(); offA();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect(dismiss).toHaveBeenCalledTimes(1);
    offB();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect(dismiss).toHaveBeenCalledTimes(1);
  });
});
