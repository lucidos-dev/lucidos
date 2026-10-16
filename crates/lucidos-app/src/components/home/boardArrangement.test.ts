import { describe, it, expect } from 'vitest';
import type { HomeBoardTile } from '../../api/client/widgets';
import { inOrder, moveTile, withTile } from './boardArrangement';
import { toArranged } from '../../store/actions/homeBoard';
import { widgetInstanceKey } from '../../utils/widgetParams';

function tile(appId: string, extra: Partial<HomeBoardTile> = {}): HomeBoardTile {
  return { app_id: appId, name: appId, reusable: true, reveal: 'on-load', sizes: ['small', 'medium'], pinned: true, size: 'medium', ...extra };
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
  const key = (t: HomeBoardTile) => widgetInstanceKey(t.app_id, t.params);

  it('orders tiles by key', () => {
    expect(inOrder(tiles, [key(tiles[2]), key(tiles[0]), key(tiles[1])]).map((t) => t.app_id)).toEqual(['c', 'a', 'b']);
  });

  /** Invariant: a widget is only offered sizes it declares. */
  it('changes a size the widget declares, and refuses one it does not', () => {
    expect(withTile(tiles, key(tiles[0]), { size: 'small' })[0].size).toBe('small');
    expect(withTile(tiles, key(tiles[0]), { size: 'large' })[0].size).toBe('medium');
  });

  it('hides a tile on this layout', () => {
    expect(withTile(tiles, key(tiles[2]), { hidden: true })[2].hidden).toBe(true);
  });

  it('sends identity, size and hidden only', () => {
    expect(toArranged(withTile(tiles, key(tiles[2]), { hidden: true }))).toEqual([
      { app_id: 'a', size: 'medium' },
      { app_id: 'b', params: { clip: 'x.mp3' }, size: 'medium' },
      { app_id: 'c', size: 'medium', hidden: true },
    ]);
  });
});
