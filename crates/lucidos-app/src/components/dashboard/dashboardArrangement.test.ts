import { describe, it, expect } from 'vitest';
import type { AppDashboardTile, DashboardTile, WidgetDashboardTile } from '../../api/client/widgets';
import { inOrder, moveTile, withTile } from './dashboardArrangement';
import { toArranged } from '../../store/actions/dashboards';
import { dashboardTileKey } from '../../store/widgets';

function tile(appId: string, extra: Partial<WidgetDashboardTile> = {}): WidgetDashboardTile {
  return { kind: 'widget', app_id: appId, name: appId, reusable: true, reveal: 'on-load', sizes: ['small', 'medium'], pinned: true, size: 'medium', ...extra };
}

function shortcut(appId: string, extra: Partial<AppDashboardTile> = {}): AppDashboardTile {
  return { kind: 'app', app_id: appId, name: appId, size: 'quarter', ...extra };
}

describe('moveTile', () => {
  it('moves one item and keeps the rest in order', () => {
    expect(moveTile(['a', 'b', 'c', 'd'], 'd', 1)).toEqual(['a', 'd', 'b', 'c']);
    expect(moveTile(['a', 'b', 'c', 'd'], 'a', 3)).toEqual(['b', 'c', 'd', 'a']);
    expect(moveTile(['a', 'b', 'c'], 'b', 1)).toEqual(['a', 'b', 'c']);
  });

  it('clamps past either end and ignores an unknown item', () => {
    expect(moveTile(['a', 'b', 'c'], 'b', -4)).toEqual(['b', 'a', 'c']);
    expect(moveTile(['a', 'b', 'c'], 'b', 99)).toEqual(['a', 'c', 'b']);
    expect(moveTile(['a', 'b'], 'z', 0)).toEqual(['a', 'b']);
  });
});

describe('arrangement edits', () => {
  const tiles = [tile('a'), tile('b', { params: { clip: 'x.mp3' } }), tile('c')];
  const key = dashboardTileKey;

  it('orders tiles by key', () => {
    expect(inOrder(tiles, [key(tiles[2]), key(tiles[0]), key(tiles[1])]).map((t) => t.app_id)).toEqual(['c', 'a', 'b']);
  });

  /** Invariant: a widget is only offered sizes it declares. */
  it('changes a size the widget declares, and refuses one it does not', () => {
    expect(withTile(tiles, key(tiles[0]), { size: 'small' })[0].size).toBe('small');
    expect(withTile(tiles, key(tiles[0]), { size: 'large' })[0].size).toBe('medium');
  });

  it('sends identity and size only', () => {
    expect(toArranged(withTile(tiles, key(tiles[2]), { size: 'small' }))).toEqual([
      { app_id: 'a', size: 'medium' },
      { app_id: 'b', params: { clip: 'x.mp3' }, size: 'medium' },
      { app_id: 'c', size: 'small' },
    ]);
  });

  /** Invariant: an app shortcut has one size, quarter, and no params. */
  it('leaves an app shortcut unchanged by a size edit', () => {
    const mixed: DashboardTile[] = [tile('a'), shortcut('notes')];
    expect(withTile(mixed, key(mixed[1]), { size: 'small' })).toEqual(mixed);
  });

  it('sends a shortcut as an app id and quarter size, with no params', () => {
    expect(toArranged([tile('a'), shortcut('notes', { label: 'Jot' })])).toEqual([
      { app_id: 'a', size: 'medium' },
      { app_id: 'notes', size: 'quarter' },
    ]);
  });

  it('keys a widget instance by its params and an app shortcut by its id alone', () => {
    expect(dashboardTileKey(tile('clip', { params: { clip: 'x.mp3' } }))).not.toBe(dashboardTileKey(tile('clip')));
    expect(dashboardTileKey(shortcut('clip'))).toBe(dashboardTileKey(tile('clip')));
  });
});
