import { describe, it, expect, beforeEach } from 'vitest';
import { shouldShowSwUpdateToast, markSwUpdateDismissed } from './sw-update';

describe('SW update toast guard', () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it('skips toast on initial install (no prior controller)', () => {
    expect(shouldShowSwUpdateToast(false)).toBe(false);
  });

  it('shows toast on genuine update (had controller at startup)', () => {
    expect(shouldShowSwUpdateToast(true)).toBe(true);
  });

  it('skips toast after dismiss flag is set', () => {
    markSwUpdateDismissed();
    expect(shouldShowSwUpdateToast(true)).toBe(false);
  });

  it('consumes dismiss flag (one-time guard)', () => {
    markSwUpdateDismissed();
    expect(shouldShowSwUpdateToast(true)).toBe(false); // consumed
    expect(shouldShowSwUpdateToast(true)).toBe(true);  // flag gone, next genuine update shows
  });

  it('dismiss flag has no effect on initial install', () => {
    markSwUpdateDismissed();
    expect(shouldShowSwUpdateToast(false)).toBe(false); // still blocked by hadController
  });
});
