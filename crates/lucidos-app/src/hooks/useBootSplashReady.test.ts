import { describe, it, expect } from 'vitest';
import { isWorkspaceReady } from './useBootSplashReady';

describe('isWorkspaceReady', () => {
  it('is ready only when connected AND threads are loaded', () => {
    expect(isWorkspaceReady('connected', true)).toBe(true);
  });

  it('is not ready while still connecting, even once threads loaded', () => {
    expect(isWorkspaceReady('connecting', true)).toBe(false);
  });

  it('is not ready while disconnected', () => {
    expect(isWorkspaceReady('disconnected', true)).toBe(false);
  });

  it('is not ready when connected but threads have not loaded yet', () => {
    expect(isWorkspaceReady('connected', false)).toBe(false);
  });
});
