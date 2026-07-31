import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, wipPreviewThreadId, threadMap, activeMenuItem, settingsSubview, CODING_AGENT_CHANNEL } from '../../../store/store';
import type { App } from '../../../store/types';
import { MENU_ITEMS } from '../../../store/types';
import { makeOptimisticThreadState, PENDING_TITLE_PLACEHOLDER } from '../../../store/thread-events';
import { CHANNEL_OPTIONS, getContentTitle, navEntryTitle } from '../headerHelpers';

vi.mock('../../../api/client', () => ({
  listAppsApi: vi.fn().mockResolvedValue([]),
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listCredentials: vi.fn().mockResolvedValue({ credentials: [] }),
}));

const fakeApp: App = {
  id: 'habit-tracker',
  name: 'Habit Tracker',
  description: 'A test app',
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

describe('getContentTitle — notification detail', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    wipPreviewThreadId.value = null;
    threadMap.value = new Map();
    activeMenuItem.value = 'notifications';
  });

  const fakeNotification = {
    id: 'n1',
    title: 'Backup completed',
    message: 'All good',
    read: false,
    created_at: new Date('2026-06-26T10:00:00Z').toISOString(),
  };

  it('prefixes the notification title with "Notification - "', () => {
    panelOverlay.value = { type: 'notification-detail', notification: fakeNotification };
    expect(getContentTitle()).toBe('Notification - Backup completed');
  });

  it('falls back to bare "Notification" when the title is empty', () => {
    panelOverlay.value = {
      type: 'notification-detail',
      notification: { ...fakeNotification, title: '' },
    };
    expect(getContentTitle()).toBe('Notification');
  });
});

describe('getContentTitle — email confirm', () => {
  const request = {
    to: ['recipient@example.com'],
    subject: 'Quarterly numbers',
    body: 'Body',
    account: 'work',
    from: 'me@example.com',
  };

  beforeEach(() => {
    panelOverlay.value = null;
    activeMenuItem.value = 'files';
  });

  it('a pending draft reads "Confirm Email"', () => {
    panelOverlay.value = { type: 'form', form: { type: 'email-confirm', request } };
    expect(getContentTitle()).toBe('Confirm Email');
  });

  it('a sent receipt reads "Email Sent" — the same label its nav-history row carries', () => {
    const form = { type: 'email-confirm' as const, request, sentAt: '2026-07-29T09:15:00.000Z' };
    panelOverlay.value = { type: 'form', form };
    expect(getContentTitle()).toBe('Email Sent');
    expect(navEntryTitle({
      menuItem: 'files',
      settingsSubview: 'main',
      overlay: { type: 'form', form },
      wipPreviewThreadId: null,
    })).toBe('Email Sent');
  });
});

describe('getContentTitle — file preview', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    activeMenuItem.value = 'files';
  });

  it('shows the base name of a data-tree path', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/research/notes.md' };
    expect(getContentTitle()).toBe('notes.md');
  });

  it('unwraps a repo-encoded path so the repo id never leaks into the title', () => {
    panelOverlay.value = { type: 'file-preview', path: 'repo:repo-1:file:src/transforms/x.jslt' };
    expect(getContentTitle()).toBe('x.jslt');
  });

  it('unwraps a repo file at the clone root (no slash to split on)', () => {
    const overlay = { type: 'file-preview' as const, path: 'repo:repo-1:file:pom.xml' };
    panelOverlay.value = overlay;
    expect(getContentTitle()).toBe('pom.xml');
    expect(navEntryTitle({
      menuItem: 'files',
      settingsSubview: 'main',
      overlay,
      wipPreviewThreadId: null,
    })).toBe('pom.xml');
  });
});

describe('getContentTitle — menu labels', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    wipPreviewThreadId.value = null;
    settingsSubview.value = 'main';
    activeMenuItem.value = 'apps';
  });

  it('every MENU_ITEMS value renders a non-empty content-header title', () => {
    for (const item of MENU_ITEMS) {
      activeMenuItem.value = item;
      expect(getContentTitle(), `menu item "${item}" must have a header label`).not.toBe('');
    }
  });

  it('Thread Queue renders its canonical title from the Settings → System subview', () => {
    // Thread Queue is no longer a top-level menu item — it's a System subpanel,
    // so its title comes from the settings subview label, not menuLabels.
    activeMenuItem.value = 'settings';
    settingsSubview.value = 'thread-queue';
    expect(getContentTitle()).toBe('Thread Queue');
  });

  it('apps renders its canonical "Apps" title (the Store moved out to the Plugins panel)', () => {
    activeMenuItem.value = 'apps';
    expect(getContentTitle()).toBe('Apps');
  });
});

describe('CHANNEL_OPTIONS', () => {
  it('labels the coding-agent channel generically for the thread filter', () => {
    expect(CHANNEL_OPTIONS.find(opt => opt.value === CODING_AGENT_CHANNEL)?.label).toBe('Coding Agent');
  });
});
