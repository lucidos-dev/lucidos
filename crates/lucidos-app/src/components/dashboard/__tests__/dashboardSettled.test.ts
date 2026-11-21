import { describe, it, expect } from 'vitest';
import type { FrameState } from '../../widgets/widgetFrameState';
import { dashboardSettled } from '../useDashboardSettled';

const states = (entries: Record<string, FrameState>) => new Map(Object.entries(entries));

describe('dashboardSettled', () => {
  it('is settled with no widget tile', () => {
    expect(dashboardSettled([], states({}), true)).toBe(true);
  });

  it('is held by a loading tile', () => {
    expect(dashboardSettled(['a', 'b'], states({ a: 'revealed', b: 'loading' }), true)).toBe(false);
  });

  it('is held by a tile that has not reported, as by one that reports unknown', () => {
    expect(dashboardSettled(['a'], states({}), true)).toBe(false);
    expect(dashboardSettled(['a'], states({ a: 'unknown' }), true)).toBe(false);
  });

  it('is not held by an off-screen tile', () => {
    expect(dashboardSettled(['a', 'b'], states({ a: 'revealed', b: 'off-screen' }), true)).toBe(true);
  });

  it('ignores a report for a tile that is gone', () => {
    expect(dashboardSettled(['a'], states({ a: 'revealed', gone: 'loading' }), true)).toBe(true);
  });

  it('holds an off-screen tile while the dashboard slides in', () => {
    expect(dashboardSettled(['a'], states({ a: 'off-screen' }), false)).toBe(false);
    expect(dashboardSettled(['a'], states({ a: 'revealed' }), false)).toBe(true);
  });
});
