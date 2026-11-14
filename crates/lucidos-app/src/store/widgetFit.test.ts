import { afterEach, describe, expect, it } from 'vitest';
import { FIT_RENDERER_SESSION_KEY, isSelectedFitRenderer } from './widgetFit';

afterEach(() => sessionStorage.clear());

describe('fit renderer shell selection', () => {
  it('requires an explicit session handle and the selected logical device', () => {
    expect(isSelectedFitRenderer('session', 'renderer', 'renderer')).toBe(false);
    sessionStorage.setItem(FIT_RENDERER_SESSION_KEY, 'session');
    expect(isSelectedFitRenderer('session', 'renderer', 'renderer')).toBe(true);
    expect(isSelectedFitRenderer('other-session', 'renderer', 'renderer')).toBe(false);
    expect(isSelectedFitRenderer('session', 'other-device', 'renderer')).toBe(false);
    expect(isSelectedFitRenderer(undefined, undefined, 'renderer')).toBe(false);
    expect(isSelectedFitRenderer('session', undefined, null)).toBe(false);
  });
});
