import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, wipPreviewThreadId, threadMap, activeMenuItem } from '../../../store/store';
import type { App } from '../../../store/types';
import { makeOptimisticThreadState, PENDING_TITLE_PLACEHOLDER } from '../../../store/thread-events';
import { getContentTitle } from '../headerHelpers';

vi.mock('../../../api/client', () => ({
  listAppsApi: vi.fn().mockResolvedValue([]),
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listCredentials: vi.fn().mockResolvedValue({ credentials: [] }),
}));

const fakeApp: App = {
  id: 'habit-tracker',
  name: 'Habit Tracker',
  description: 'A test app',
  knowhow: [],
};

function seedThread(id: string, title: string): void {
  const thread = makeOptimisticThreadState({
    id,
    title,
    channel: 'claude_code',
    initiator: 'user',
    eventsLoaded: true,
    codingAgentKind: 'app',
    codingAgentFolder: '/data/apps/habit-tracker',
  });
  const next = new Map(threadMap.value);
  next.set(id, thread);
  threadMap.value = next;
}

describe('getContentTitle — WIP preview', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    wipPreviewThreadId.value = null;
    threadMap.value = new Map();
    activeMenuItem.value = 'apps';
  });

  it('app-ui overlay + no WIP active → returns the app name', () => {
    panelOverlay.value = { type: 'app-ui', app: fakeApp };
    expect(getContentTitle()).toBe('Habit Tracker');
  });

  it('app-ui overlay + WIP thread with title → returns "<app> (WIP by <thread-name>)"', () => {
    seedThread('thread-1', 'Fix the streak counter');
    panelOverlay.value = { type: 'app-ui', app: fakeApp };
    wipPreviewThreadId.value = 'thread-1';
    expect(getContentTitle()).toBe('Habit Tracker (WIP by Fix the streak counter)');
  });

  it('app-ui overlay + WIP thread whose title is the placeholder → returns "<app> (WIP)"', () => {
    seedThread('thread-2', PENDING_TITLE_PLACEHOLDER);
    panelOverlay.value = { type: 'app-ui', app: fakeApp };
    wipPreviewThreadId.value = 'thread-2';
    expect(getContentTitle()).toBe('Habit Tracker (WIP)');
  });

  it('app-ui overlay + WIP id pointing to a thread not in threadMap → falls back to app name', () => {
    panelOverlay.value = { type: 'app-ui', app: fakeApp };
    wipPreviewThreadId.value = 'unknown-thread';
    expect(getContentTitle()).toBe('Habit Tracker');
  });
});
