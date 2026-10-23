import { describe, it, expect } from 'vitest';
import { widgetFrameState } from '../widgetFrameState';

describe('widgetFrameState', () => {
  it('is unknown until the viewport observer has answered', () => {
    expect(widgetFrameState(null, false, false)).toBe('unknown');
    expect(widgetFrameState(null, true, true)).toBe('unknown');
  });

  it('is off-screen out of view and not yet revealed', () => {
    expect(widgetFrameState(false, false, false)).toBe('off-screen');
  });

  it('is loading in view with its cover up', () => {
    expect(widgetFrameState(true, true, false)).toBe('loading');
  });

  it('is revealed once the cover has lifted, in view or held out of it', () => {
    expect(widgetFrameState(true, true, true)).toBe('revealed');
    expect(widgetFrameState(false, false, true)).toBe('revealed');
  });
});
