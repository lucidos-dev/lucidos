import { describe, it, expect } from 'vitest';
import type { App, Loadable } from '../../store/types';
import {
  liveAppReveal,
  appFrameLoading,
  appFrameRevealed,
  revealFuseMs,
  COVER_MAX_MS,
  READY_MAX_MS,
  type AppFrameSignals,
} from './appFrameReveal';

const signals = (s: Partial<AppFrameSignals> = {}): AppFrameSignals => ({
  loaded: false,
  ready: false,
  fused: false,
  ...s,
});

describe('an on-load app (the default)', () => {
  it('is revealed by load or by the fuse, as before the ready signal existed', () => {
    expect(appFrameRevealed('on-load', signals())).toBe(false);
    expect(appFrameRevealed('on-load', signals({ loaded: true }))).toBe(true);
    expect(appFrameRevealed('on-load', signals({ fused: true }))).toBe(true);
  });

  it('ignores a ready call, since it never opted in', () => {
    expect(appFrameRevealed('on-load', signals({ ready: true }))).toBe(false);
  });

  it('keeps the bar running past the fuse until load, so an early reveal still says loading', () => {
    expect(appFrameLoading('on-load', signals())).toBe(true);
    expect(appFrameLoading('on-load', signals({ fused: true }))).toBe(true);
    expect(appFrameLoading('on-load', signals({ fused: true, loaded: true }))).toBe(false);
  });

  it('keeps the short fuse', () => {
    expect(revealFuseMs('on-load')).toBe(COVER_MAX_MS);
  });
});

describe('an on-ready app', () => {
  it('stays covered after load until it calls ready', () => {
    expect(appFrameRevealed('on-ready', signals({ loaded: true }))).toBe(false);
    expect(appFrameRevealed('on-ready', signals({ loaded: true, ready: true }))).toBe(true);
  });

  it('is revealed by a ready sent before load', () => {
    expect(appFrameRevealed('on-ready', signals({ ready: true }))).toBe(true);
  });

  it('is revealed by the longer fuse when ready never comes', () => {
    expect(appFrameRevealed('on-ready', signals({ loaded: true, fused: true }))).toBe(true);
    expect(revealFuseMs('on-ready')).toBe(READY_MAX_MS);
    expect(READY_MAX_MS).toBeGreaterThan(COVER_MAX_MS);
  });

  it('runs the bar until ready or the fuse, whatever load did', () => {
    expect(appFrameLoading('on-ready', signals({ loaded: true }))).toBe(true);
    expect(appFrameLoading('on-ready', signals({ ready: true }))).toBe(false);
    expect(appFrameLoading('on-ready', signals({ loaded: true, fused: true }))).toBe(false);
  });
});

describe('the open app\'s reveal', () => {
  const opened: App = { id: 'habit-tracker', name: 'Habit Tracker', description: '', reveal: 'on-ready' };
  const listed = (reveal: App['reveal']): Loadable<App[]> => ({ status: 'loaded', data: [{ ...opened, reveal }] });

  it('follows a manifest edited since the app opened', () => {
    expect(liveAppReveal(opened, listed('on-load'))).toBe('on-load');
  });

  it('falls back to the snapshot while the list is not loaded, or lacks the app', () => {
    expect(liveAppReveal(opened, { status: 'loading' })).toBe('on-ready');
    expect(liveAppReveal(opened, { status: 'loaded', data: [] })).toBe('on-ready');
  });
});
