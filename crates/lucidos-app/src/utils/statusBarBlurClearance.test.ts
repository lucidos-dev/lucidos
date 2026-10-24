// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./platform', () => ({ isIOSPwa: vi.fn() }));

import { isIOSPwa } from './platform';
import { CLEARANCE_PX, installStatusBarBlurClearance } from './statusBarBlurClearance';

const root = document.documentElement;
const clearanceVar = () => root.style.getPropertyValue('--status-bar-blur-clearance');

beforeEach(() => {
  vi.mocked(isIOSPwa).mockReturnValue(true);
});

afterEach(() => {
  root.style.removeProperty('--status-bar-blur-clearance');
});

describe('installStatusBarBlurClearance', () => {
  it('drops the header controls by the constant clearance in the installed iOS app', () => {
    installStatusBarBlurClearance();
    expect(clearanceVar()).toBe(`${CLEARANCE_PX}px`);
    expect(CLEARANCE_PX).toBe(24);
  });

  it('leaves every other device at the stylesheet default of 0', () => {
    vi.mocked(isIOSPwa).mockReturnValue(false);
    installStatusBarBlurClearance();
    expect(clearanceVar()).toBe('');
  });
});
