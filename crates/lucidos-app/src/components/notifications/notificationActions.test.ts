import { describe, it, expect } from 'vitest';
import { notificationActions, notificationTriggerId } from './notificationActions';
import type { NavigateUi, Tap } from '@lucidos/sdk';
import type { Notification } from '../../store/types';

function n(over: Partial<Notification> = {}): Notification {
  return {
    id: 'n1',
    title: 't',
    message: 'm',
    read: false,
    created_at: new Date(0).toISOString(),
    ...over,
  };
}

describe('notificationTriggerId', () => {
  it('reads the trigger id off the legacy task_id column', () => {
    expect(notificationTriggerId(n({ task_id: 'trig-1' }))).toBe('trig-1');
  });

  it('is null when the notification is about no trigger', () => {
    expect(notificationTriggerId(n())).toBeNull();
  });
});

describe('notificationActions', () => {
  it('offers no actions row for a plain informational notification', () => {
    const a = notificationActions(n(), false);
    expect(a.any).toBe(false);
    expect(a).toMatchObject({ openThread: false, openTrigger: false, navTap: null });
  });

  it('offers "Open trigger" for a trigger-failure notification', () => {
    // The command-guard block case: the user reads what was tried, then jumps
    // to the trigger's settings to grant the side-effect it needs.
    const a = notificationActions(n({ task_id: 'trig-1' }), false);
    expect(a.openTrigger).toBe(true);
    expect(a.any).toBe(true);
  });

  it('opens the actions row for a linked app even with no other button', () => {
    // The panel narrows its own LinkedAppResult to render "Open <app>", so the
    // helper only has to keep `any` true for it.
    expect(notificationActions(n({ app_id: 'habit-tracker' }), true).any).toBe(true);
    expect(notificationActions(n({ thread_id: 'th-1' }), true).openThread).toBe(true);
  });

  it('keeps a navigate tap that no dedicated button covers', () => {
    const a = notificationActions(n({ tap: { kind: 'navigate', to: { target: 'changes' } } }), false);
    expect(a.navTap).toEqual({ target: 'changes' });
    expect(a.any).toBe(true);
  });

  it('drops a navigate tap duplicated by the dedicated thread button', () => {
    const a = notificationActions(
      n({ thread_id: 'th-1', tap: { kind: 'navigate', to: { target: 'thread', id: 'th-1' } } }),
      false,
    );
    expect(a.openThread).toBe(true);
    expect(a.navTap).toBeNull();
  });

  it('drops a navigate tap duplicated by the dedicated trigger button', () => {
    const a = notificationActions(
      n({ task_id: 'trig-1', tap: { kind: 'navigate', to: { target: 'trigger', id: 'trig-1' } } }),
      false,
    );
    expect(a.openTrigger).toBe(true);
    expect(a.navTap).toBeNull();
  });

  it('drops a navigate tap duplicated by the dedicated app button', () => {
    const to: NavigateUi = { target: 'app', app_id: 'habit-tracker' };
    const tap: Tap = { kind: 'navigate', to };
    // Linked app resolved: the dedicated button covers it, so the tap is dropped.
    expect(notificationActions(n({ app_id: 'habit-tracker', tap }), true).navTap).toBeNull();
    // Unresolved (stale id, or the list hasn't loaded): the tap is the only way
    // there, so it survives.
    expect(notificationActions(n({ app_id: 'habit-tracker', tap }), false).navTap).toEqual(to);
  });

  it('keeps a thread-targeted navigate tap when there is no dedicated thread button', () => {
    // The row carries no thread_id, so the tap is the only way there.
    const a = notificationActions(n({ tap: { kind: 'navigate', to: { target: 'thread', id: 'th-9' } } }), false);
    expect(a.openThread).toBe(false);
    expect(a.navTap).toEqual({ target: 'thread', id: 'th-9' });
  });

  it('ignores a modal tap', () => {
    const a = notificationActions(n({ task_id: 'trig-1', tap: { kind: 'modal' } }), false);
    expect(a.navTap).toBeNull();
    expect(a.openTrigger).toBe(true);
  });
});
