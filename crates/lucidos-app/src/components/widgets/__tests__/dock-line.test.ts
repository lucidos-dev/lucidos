/**
 * Where docked widget windows hang (ADR 0419): under the app header on a
 * desktop, under the thread's title row on a phone.
 */
import { describe, expect, it } from 'vitest';
import { dockLineFor } from '../useDockLine';

const view = {
  phone: false,
  viewportWidth: 1200,
  row: { left: 300, right: 900, bottom: 96 },
  rowOnScreen: true,
  gap: 8,
};

describe('dockLineFor', () => {
  it('docks a desktop under the app header and its banners, across the window, not the title row', () => {
    expect(dockLineFor(view)).toEqual({ left: 8, right: 1192, top: 'var(--app-header-bottom)' });
  });

  it('docks a phone under the thread title row, so it never covers the title', () => {
    expect(dockLineFor({ ...view, phone: true })).toEqual({ left: 308, right: 892, top: '96px' });
  });

  it('hides a phone dock while the title row is off screen', () => {
    expect(dockLineFor({ ...view, phone: true, rowOnScreen: false })).toBeNull();
  });

  it('docks a phone with no title row under the header', () => {
    expect(dockLineFor({ ...view, phone: true, row: null })).toEqual(dockLineFor(view));
  });
});
