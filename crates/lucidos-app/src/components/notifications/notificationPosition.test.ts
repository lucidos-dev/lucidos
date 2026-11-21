import { describe, it, expect } from 'vitest';
import { notificationPosition } from './notificationPosition';
import type { Notification } from '../../store/types';

function list(...ids: string[]): Notification[] {
  return ids.map((id) => ({ id, title: id, message: '', created_at: '2026-09-24T08:00:00Z', read: true }));
}

describe('notificationPosition', () => {
  it('has both neighbours in the middle of the list', () => {
    expect(notificationPosition(list('a', 'b', 'c'), 'b', false)).toEqual({
      hasNewer: true,
      hasOlder: true,
    });
  });

  it('has nothing newer at the top of the list', () => {
    const pos = notificationPosition(list('a', 'b'), 'a', false);
    expect(pos.hasNewer).toBe(false);
    expect(pos.hasOlder).toBe(true);
  });

  it('has nothing older at the bottom when the server has no more pages', () => {
    const pos = notificationPosition(list('a', 'b'), 'b', false);
    expect(pos.hasNewer).toBe(true);
    expect(pos.hasOlder).toBe(false);
  });

  // Stepping older past the last loaded row pulls the next page first, so the
  // button stays live.
  it('keeps older live at the last loaded row while more pages exist', () => {
    expect(notificationPosition(list('a', 'b'), 'b', true)).toEqual({
      hasNewer: true,
      hasOlder: true,
    });
  });

  it('offers no neighbours for a notification the list does not hold', () => {
    expect(notificationPosition(list('a'), 'x', true)).toEqual({
      hasNewer: false,
      hasOlder: false,
    });
  });
});
