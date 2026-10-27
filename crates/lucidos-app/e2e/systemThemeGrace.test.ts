import { describe, expect, it } from 'vitest';
import {
  SYSTEM_THEME_MODE_CONFIRM_MS,
  SYSTEM_THEME_MODE_SETTLE_MS,
} from '../../../packages/lucidos-sdk/src/appearance';
import { SYSTEM_THEME_GRACE_MS } from './systemThemeGrace';

describe('the system theme grace', () => {
  it('outlasts both follower reads with room for a repaint', () => {
    const followerLatency = SYSTEM_THEME_MODE_SETTLE_MS + SYSTEM_THEME_MODE_CONFIRM_MS;
    expect(SYSTEM_THEME_GRACE_MS).toBeGreaterThan(followerLatency);
    expect(SYSTEM_THEME_GRACE_MS - followerLatency).toBeGreaterThanOrEqual(1_000);
  });
});
