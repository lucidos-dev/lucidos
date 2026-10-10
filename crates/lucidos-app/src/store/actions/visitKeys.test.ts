import { describe, expect, it } from 'vitest';
import { NAVIGATE_TARGETS } from '@lucidos/sdk';
import type { Tap } from '@lucidos/sdk';
import { navigateVisitKey, notificationTarget } from './visitKeys';

/** Targets that deliberately name no revisitable place. Every other advertised
 *  target must produce a key. A new one landing as a silent null would quietly
 *  stop its own notifications from ever clearing. */
const NO_PLACE = new Set<string>(['url', 'new-app', 'new-trigger', 'new-chat']);

/** Every id-bearing field populated, so each branch finds what it needs. */
function kitchenSink(target: string) {
  return {
    target,
    app_id: 'habit-tracker',
    file_path: 'data/artifacts/x.md',
    id: 'id-1',
    settings_view: 'models',
  };
}

describe('navigateVisitKey', () => {
  it('every advertised navigate target either names a place or is declared placeless', () => {
    for (const target of NAVIGATE_TARGETS) {
      const key = navigateVisitKey(kitchenSink(target));
      if (NO_PLACE.has(target)) {
        expect(key, `target "${target}" should name no place`).toBeNull();
      } else {
        expect(key, `target "${target}" names no place and is not declared placeless`).not.toBeNull();
      }
    }
  });

  it('names each entity target by its own id', () => {
    expect(navigateVisitKey({ target: 'thread', id: 't-1' })).toBe('thread:t-1');
    expect(navigateVisitKey({ target: 'app', app_id: 'habit-tracker' })).toBe('app:habit-tracker');
    expect(navigateVisitKey({ target: 'trigger', id: 'tr-1' })).toBe('trigger:tr-1');
  });

  it('reads the retired app-ui alias as an app', () => {
    expect(navigateVisitKey({ target: 'app-ui', app_id: 'habit-tracker' })).toBe('app:habit-tracker');
  });

  it('drops the detail inside a place', () => {
    // A fragment picks a spot inside the app the reader is already looking at.
    const bare = navigateVisitKey({ target: 'app', app_id: 'habit-tracker' });
    expect(navigateVisitKey({ target: 'app', app_id: 'habit-tracker', fragment: 'day-1' } as never))
      .toBe(bare);
    // The event is measured separately, so it must not split the thread's key.
    expect(navigateVisitKey({ target: 'thread', id: 't-1', event_id: 'e-1' })).toBe('thread:t-1');
  });

  it('resolves a file path the way the preview overlay stores it', () => {
    // A prefix-less path is an artifact, and the overlay holds the prefixed
    // form. Both taps therefore have to reach one key, or the tap and the open
    // file would disagree about being the same place.
    expect(navigateVisitKey({ target: 'file', file_path: 'x.md' })).toBe('file:artifacts/x.md');
    expect(navigateVisitKey({ target: 'file', file_path: 'artifacts/x.md' }))
      .toBe('file:artifacts/x.md');
  });

  it('keeps a repo-encoded file distinct from a workspace file', () => {
    const repo = navigateVisitKey({ target: 'file', file_path: 'repo:r-1:file:src/main.rs' });
    const data = navigateVisitKey({ target: 'file', file_path: 'src/main.rs' });
    expect(repo).not.toBe(data);
  });

  it('lands thread-queue on the settings sub-section the router sends it to', () => {
    expect(navigateVisitKey({ target: 'thread-queue' })).toBe('settings:thread-queue');
  });

  it('gives a bare settings target the home sub-section', () => {
    expect(navigateVisitKey({ target: 'settings' })).toBe('settings:main');
  });

  it('aliases a retired settings sub-section onto the one that absorbed it', () => {
    expect(navigateVisitKey({ target: 'settings', settings_view: 'mobile-access' }))
      .toBe('settings:access');
  });

  it('sends both plugin targets to one panel', () => {
    expect(navigateVisitKey({ target: 'plugins' })).toBe('panel:plugins');
    expect(navigateVisitKey({ target: 'app-store' })).toBe('panel:plugins');
  });

  it('names no place when the target is missing the id it needs', () => {
    expect(navigateVisitKey({ target: 'thread' })).toBeNull();
    expect(navigateVisitKey({ target: 'app' })).toBeNull();
    expect(navigateVisitKey({ target: 'file' })).toBeNull();
    expect(navigateVisitKey({ target: 'trigger' })).toBeNull();
  });

  it('names no place for a target it has never heard of', () => {
    expect(navigateVisitKey({ target: 'teleport' })).toBeNull();
  });
});

describe('notificationTarget', () => {
  const navigate = (to: Record<string, unknown>): Tap =>
    ({ kind: 'navigate', to } as unknown as Tap);

  it('measures a thread tap carrying an event by that event card', () => {
    expect(notificationTarget(navigate({ target: 'thread', id: 't-1', event_id: 'e-1' })))
      .toEqual({ kind: 'event', threadId: 't-1', eventId: 'e-1' });
  });

  it('falls back to the thread itself when the tap names no event', () => {
    expect(notificationTarget(navigate({ target: 'thread', id: 't-1' })))
      .toEqual({ kind: 'place', key: 'thread:t-1' });
  });

  it('measures a card-less target by its place', () => {
    expect(notificationTarget(navigate({ target: 'settings', settings_view: 'backup' })))
      .toEqual({ kind: 'place', key: 'settings:backup' });
  });

  it('gives a modal tap no target at all', () => {
    // Its place IS the notification detail, and opening that already marks it
    // read. Nothing for the seen rule to add.
    expect(notificationTarget({ kind: 'modal' })).toBeNull();
  });

  it('gives a missing tap no target', () => {
    expect(notificationTarget(null)).toBeNull();
    expect(notificationTarget(undefined)).toBeNull();
  });

  it('gives a navigate to nowhere revisitable no target', () => {
    expect(notificationTarget(navigate({ target: 'url', url: 'https://example.com' }))).toBeNull();
  });
});
