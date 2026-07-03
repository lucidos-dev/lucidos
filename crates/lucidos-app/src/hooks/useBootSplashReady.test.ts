import { describe, it, expect } from 'vitest';
import { isWorkspaceReady, delayedBootStatus } from './useBootSplashReady';

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

describe('delayedBootStatus', () => {
  it('shows "Loading…" once connected, regardless of context', () => {
    expect(delayedBootStatus(true, false)).toBe('Loading…');
    expect(delayedBootStatus(true, true)).toBe('Loading…');
  });

  it('shows "Workspace not started" on a stalled DIRECT engine port (no auto-start)', () => {
    expect(delayedBootStatus(false, true)).toBe('Workspace not started');
  });

  it('keeps "Connecting…" behind the gateway (which lazy-starts the engine)', () => {
    expect(delayedBootStatus(false, false)).toBe('Connecting…');
  });
});
