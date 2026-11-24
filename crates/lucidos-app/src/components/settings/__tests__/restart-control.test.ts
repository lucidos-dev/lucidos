import { describe, it, expect } from 'vitest';
import { restartControlHome, type RestartControlHome } from '../restartControl';

describe('restartControlHome', () => {
  it('keeps the restart in System > Overview on a dev install, where it rebuilds', () => {
    expect(restartControlHome(false)).toBe('overview');
  });

  it('moves the restart to System > Debugging on a packaged install', () => {
    expect(restartControlHome(true)).toBe('debugging');
  });

  it('names exactly one home per mode, so the control can never render twice or vanish', () => {
    const homes: RestartControlHome[] = [restartControlHome(false), restartControlHome(true)];
    expect(new Set(homes).size).toBe(2);
    expect(homes).toContain('overview');
    expect(homes).toContain('debugging');
  });
});
