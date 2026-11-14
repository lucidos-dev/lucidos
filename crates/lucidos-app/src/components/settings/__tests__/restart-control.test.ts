import { describe, it, expect } from 'vitest';
import { restartControlHome, type RestartControlHome } from '../restartControl';

/** The engine-restart control has exactly one home, chosen by whether the
 *  install is packaged. These cases pin the user-reported confusion: the DMG app
 *  showed "Rebuild & Restart" in System > Overview, on an install that ships its
 *  binary and can never rebuild anything.
 *
 *  The invariant worth protecting is not "packaged hides the Overview button" on
 *  its own, but that the two surfaces stay complementary: never both, never
 *  neither. Both call sites compare this result against their own name, so the
 *  cases below cover every rendering outcome. */
describe('restartControlHome', () => {
  it('keeps the restart in System > Overview on a dev install, where it rebuilds', () => {
    expect(restartControlHome(false)).toBe('overview');
  });

  it('moves the restart to System > Debugging on a packaged install', () => {
    expect(restartControlHome(true)).toBe('debugging');
  });

  it('names exactly one home per mode, so the control can never render twice or vanish', () => {
    const homes: RestartControlHome[] = [restartControlHome(false), restartControlHome(true)];
    // Both surfaces are covered, and neither mode claims both of them.
    expect(new Set(homes).size).toBe(2);
    expect(homes).toContain('overview');
    expect(homes).toContain('debugging');
  });
});
