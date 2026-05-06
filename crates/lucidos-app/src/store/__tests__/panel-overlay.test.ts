import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  panelOverlay,
  activeMenuItem,
  activeInlineForm,
  currentApp,
  previewFile,
  panelUrl,
  viewingNotification,
  closeInlineForm,
} from '../store';
import type { PanelOverlay, InlineForm } from '../store';
import type { App, Notification } from '../types';
import { switchMenuItem } from '../actions/menu';
import { openAddCredential, openEditCredential } from '../actions/credentials';
import { statesEqual } from '../actions/navigation';
import type { NavEntry } from '../actions/navigation';

// switchMenuItem and openAddCredential trigger loaders (apps, notifications,
// credentials) as side effects — mock the API so tests don't fire real fetches
// (which fail in the JSDOM environment).
vi.mock('../../api/client', () => ({
  listAppsApi: vi.fn().mockResolvedValue([]),
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listCredentials: vi.fn().mockResolvedValue({ credentials: [] }),
}));

const fakeApp: App = {
  id: 'test-app',
  name: 'Test App',
  description: 'A test',
  knowhow: [],
};
const fakeNotification: Notification = {
  id: 'notif-1',
  title: 'Test',
  message: 'Hello',
  read: false,
  created_at: '2026-01-01T00:00:00Z',
};

describe('PanelOverlay discriminated union', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    activeMenuItem.value = 'files';
  });

  describe('computed aliases derive correctly from overlay', () => {
    it('null overlay → all computeds null', () => {
      panelOverlay.value = null;
      expect(activeInlineForm.value).toBeNull();
      expect(currentApp.value).toBeNull();
      expect(previewFile.value).toBeNull();
      expect(panelUrl.value).toBeNull();
      expect(viewingNotification.value).toBeNull();
    });

    it('form overlay → activeInlineForm set, others null', () => {
      const form: InlineForm = { type: 'credential' };
      panelOverlay.value = { type: 'form', form };
      expect(activeInlineForm.value).toEqual(form);
      expect(currentApp.value).toBeNull();
      expect(previewFile.value).toBeNull();
      expect(panelUrl.value).toBeNull();
      expect(viewingNotification.value).toBeNull();
    });

    it('app-ui overlay → currentApp set, others null', () => {
      panelOverlay.value = { type: 'app-ui', app: fakeApp };
      expect(currentApp.value).toEqual(fakeApp);
      expect(activeInlineForm.value).toBeNull();
      expect(previewFile.value).toBeNull();
      expect(panelUrl.value).toBeNull();
      expect(viewingNotification.value).toBeNull();
    });

    it('file-preview overlay → previewFile set, others null', () => {
      panelOverlay.value = { type: 'file-preview', path: 'docs/readme.md' };
      expect(previewFile.value).toBe('docs/readme.md');
      expect(activeInlineForm.value).toBeNull();
      expect(currentApp.value).toBeNull();
      expect(panelUrl.value).toBeNull();
      expect(viewingNotification.value).toBeNull();
    });

    it('url-preview overlay → panelUrl set, others null', () => {
      panelOverlay.value = { type: 'url-preview', url: 'https://example.com' };
      expect(panelUrl.value).toBe('https://example.com');
      expect(activeInlineForm.value).toBeNull();
      expect(currentApp.value).toBeNull();
      expect(previewFile.value).toBeNull();
      expect(viewingNotification.value).toBeNull();
    });

    it('notification-detail overlay → viewingNotification set, others null', () => {
      panelOverlay.value = { type: 'notification-detail', notification: fakeNotification };
      expect(viewingNotification.value).toEqual(fakeNotification);
      expect(activeInlineForm.value).toBeNull();
      expect(currentApp.value).toBeNull();
      expect(previewFile.value).toBeNull();
      expect(panelUrl.value).toBeNull();
    });
  });

  describe('mutual exclusivity — setting one overlay type replaces the previous', () => {
    it('app-ui → file-preview clears app', () => {
      panelOverlay.value = { type: 'app-ui', app: fakeApp };
      expect(currentApp.value).not.toBeNull();

      panelOverlay.value = { type: 'file-preview', path: 'test.md' };
      expect(currentApp.value).toBeNull();
      expect(previewFile.value).toBe('test.md');
    });

    it('form → url-preview clears form', () => {
      panelOverlay.value = { type: 'form', form: { type: 'credential' } };
      expect(activeInlineForm.value).not.toBeNull();

      panelOverlay.value = { type: 'url-preview', url: 'https://example.com' };
      expect(activeInlineForm.value).toBeNull();
      expect(panelUrl.value).toBe('https://example.com');
    });

    it('notification → form clears notification', () => {
      panelOverlay.value = { type: 'notification-detail', notification: fakeNotification };
      expect(viewingNotification.value).not.toBeNull();

      panelOverlay.value = { type: 'form', form: { type: 'new-app' } };
      expect(viewingNotification.value).toBeNull();
      expect(activeInlineForm.value?.type).toBe('new-app');
    });
  });

  describe('closeInlineForm clears any overlay', () => {
    it('clears form overlay', () => {
      panelOverlay.value = { type: 'form', form: { type: 'credential' } };
      closeInlineForm();
      expect(panelOverlay.value).toBeNull();
    });

    it('clears app-ui overlay', () => {
      panelOverlay.value = { type: 'app-ui', app: fakeApp };
      closeInlineForm();
      expect(panelOverlay.value).toBeNull();
    });
  });

  describe('closeInlineForm resets trigger list scroll', () => {
    // Save/Cancel/Escape on a trigger form all converge here, so the scroll
    // reset must live at this layer rather than per-button. Without it, the
    // user lands back at the row they edited (useScrollMemory restores the
    // pre-edit offset) instead of the top.
    beforeEach(() => localStorage.clear());

    it('clears the saved trigger list scroll when a trigger form closes', () => {
      localStorage.setItem('lucidos-scroll-content-triggers', '500');
      panelOverlay.value = { type: 'form', form: { type: 'trigger', taskId: 't1' } };

      closeInlineForm();

      expect(localStorage.getItem('lucidos-scroll-content-triggers')).toBeNull();
    });

    it('preserves the trigger list scroll when a non-trigger form closes', () => {
      // Other form types (credentials, app-edit) shouldn't blow away unrelated
      // saved positions just because they share the close path.
      localStorage.setItem('lucidos-scroll-content-triggers', '500');
      panelOverlay.value = { type: 'form', form: { type: 'credential' } };

      closeInlineForm();

      expect(localStorage.getItem('lucidos-scroll-content-triggers')).toBe('500');
    });
  });

  describe('switchMenuItem clears overlay (integration)', () => {
    it('clears form overlay when switching menu items', () => {
      panelOverlay.value = { type: 'form', form: { type: 'trigger' } };
      switchMenuItem('notifications');
      expect(panelOverlay.value).toBeNull();
      expect(activeInlineForm.value).toBeNull();
    });

    it('clears notification overlay when switching menu items', () => {
      panelOverlay.value = { type: 'notification-detail', notification: fakeNotification };
      switchMenuItem('files');
      expect(panelOverlay.value).toBeNull();
      expect(viewingNotification.value).toBeNull();
    });

    it('clears url-preview overlay when switching menu items', () => {
      panelOverlay.value = { type: 'url-preview', url: 'https://example.com' };
      switchMenuItem('apps');
      expect(panelOverlay.value).toBeNull();
      expect(panelUrl.value).toBeNull();
    });

    it('clears overlay even when re-selecting the same menu item', () => {
      activeMenuItem.value = 'notifications';
      panelOverlay.value = { type: 'app-ui', app: fakeApp };

      switchMenuItem('notifications');

      expect(panelOverlay.value).toBeNull();
      expect(currentApp.value).toBeNull();
    });
  });

  describe('invalid states are impossible', () => {
    it('cannot have both app UI and file preview active simultaneously', () => {
      panelOverlay.value = { type: 'app-ui', app: fakeApp };
      // There's no way to also have file preview — it's a single union value
      expect(previewFile.value).toBeNull();
      expect(currentApp.value).not.toBeNull();

      // Setting file preview replaces app UI entirely
      panelOverlay.value = { type: 'file-preview', path: 'test.md' };
      expect(currentApp.value).toBeNull();
      expect(previewFile.value).toBe('test.md');
    });

    it('cannot have both form and notification active simultaneously', () => {
      panelOverlay.value = { type: 'form', form: { type: 'credential' } };
      expect(viewingNotification.value).toBeNull();
      expect(activeInlineForm.value).not.toBeNull();
    });
  });
});

describe('overlaysEqual (via statesEqual)', () => {
  function makeNav(overlay: PanelOverlay = null): NavEntry {
    return { menuItem: 'files' as const, settingsSubview: 'main' as const, overlay };
  }

  it('both null → equal', () => {
    expect(statesEqual(makeNav(null), makeNav(null))).toBe(true);
  });

  it('null vs non-null → not equal', () => {
    expect(statesEqual(
      makeNav(null),
      makeNav({ type: 'file-preview', path: 'a.md' }),
    )).toBe(false);
  });

  it('same form type and fields → equal', () => {
    expect(statesEqual(
      makeNav({ type: 'form', form: { type: 'credential', editing: 'aws' } }),
      makeNav({ type: 'form', form: { type: 'credential', editing: 'aws' } }),
    )).toBe(true);
  });

  it('different form subtypes → not equal', () => {
    expect(statesEqual(
      makeNav({ type: 'form', form: { type: 'credential' } }),
      makeNav({ type: 'form', form: { type: 'new-app' } }),
    )).toBe(false);
  });

  it('same app-ui → equal', () => {
    expect(statesEqual(
      makeNav({ type: 'app-ui', app: fakeApp }),
      makeNav({ type: 'app-ui', app: fakeApp }),
    )).toBe(true);
  });

  it('different app-ui apps → not equal', () => {
    const otherApp: App = { ...fakeApp, id: 'other-app' };
    expect(statesEqual(
      makeNav({ type: 'app-ui', app: fakeApp }),
      makeNav({ type: 'app-ui', app: otherApp }),
    )).toBe(false);
  });

  it('same file-preview → equal', () => {
    expect(statesEqual(
      makeNav({ type: 'file-preview', path: 'a.md' }),
      makeNav({ type: 'file-preview', path: 'a.md' }),
    )).toBe(true);
  });

  it('different file-preview → not equal', () => {
    expect(statesEqual(
      makeNav({ type: 'file-preview', path: 'a.md' }),
      makeNav({ type: 'file-preview', path: 'b.md' }),
    )).toBe(false);
  });

  it('same url-preview → equal', () => {
    expect(statesEqual(
      makeNav({ type: 'url-preview', url: 'https://a.com' }),
      makeNav({ type: 'url-preview', url: 'https://a.com' }),
    )).toBe(true);
  });

  it('different url-preview → not equal', () => {
    expect(statesEqual(
      makeNav({ type: 'url-preview', url: 'https://a.com' }),
      makeNav({ type: 'url-preview', url: 'https://b.com' }),
    )).toBe(false);
  });

  it('same notification → equal', () => {
    expect(statesEqual(
      makeNav({ type: 'notification-detail', notification: fakeNotification }),
      makeNav({ type: 'notification-detail', notification: fakeNotification }),
    )).toBe(true);
  });

  it('different notification → not equal', () => {
    const other = { ...fakeNotification, id: 'notif-2' };
    expect(statesEqual(
      makeNav({ type: 'notification-detail', notification: fakeNotification }),
      makeNav({ type: 'notification-detail', notification: other }),
    )).toBe(false);
  });

  it('different overlay types → not equal', () => {
    expect(statesEqual(
      makeNav({ type: 'file-preview', path: 'a.md' }),
      makeNav({ type: 'url-preview', url: 'https://a.com' }),
    )).toBe(false);
  });

  // Two credential REQUEST forms (engine asking for different services) must
  // not be considered equal — otherwise the second request silently fails to
  // push a nav entry and the user can't navigate forward to it after going
  // back.
  it('credential request forms for different services → not equal', () => {
    expect(statesEqual(
      makeNav({ type: 'form', form: { type: 'credential', request: { service: 'helius' } } }),
      makeNav({ type: 'form', form: { type: 'credential', request: { service: 'github' } } }),
    )).toBe(false);
  });

  it('credential request vs blank Add Credential → not equal', () => {
    expect(statesEqual(
      makeNav({ type: 'form', form: { type: 'credential', request: { service: 'helius' } } }),
      makeNav({ type: 'form', form: { type: 'credential' } }),
    )).toBe(false);
  });

  // Email-confirm forms with different draft contents must be distinct entries
  // — otherwise opening a second confirmation while one is open silently fails.
  it('email-confirm forms for different drafts → not equal', () => {
    const draftA = { type: 'email-confirm' as const, request: {
      to: ['a@example.com'], subject: 'A', body: 'hi', account: 'work', from: 'me@example.com',
    } };
    const draftB = { type: 'email-confirm' as const, request: {
      to: ['b@example.com'], subject: 'B', body: 'hi', account: 'work', from: 'me@example.com',
    } };
    expect(statesEqual(
      makeNav({ type: 'form', form: draftA }),
      makeNav({ type: 'form', form: draftB }),
    )).toBe(false);
  });
});

describe('openAddCredential sets overlay after navigation', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    activeMenuItem.value = 'files';
  });

  it('openAddCredential results in credential form overlay being set', () => {
    openAddCredential();
    expect(panelOverlay.value).not.toBeNull();
    expect(panelOverlay.value?.type).toBe('form');
    expect(activeInlineForm.value?.type).toBe('credential');
  });

  it('openEditCredential results in credential edit form overlay being set', () => {
    openEditCredential('github');
    expect(panelOverlay.value).not.toBeNull();
    const form = activeInlineForm.value;
    expect(form?.type).toBe('credential');
    expect(form?.type === 'credential' && form.editing).toBe('github');
  });
});

describe('backward compat: old NavEntry format in localStorage', () => {
  it('statesEqual handles entries without overlay field gracefully', () => {
    const oldEntry = {
      menuItem: 'files',
      settingsSubview: 'main',
      inlineForm: null,
      notificationId: null,
      notification: null,
      appId: null,
      app: null,
      component: null,
      filePath: null,
      panelUrl: null,
    };
    expect(() => statesEqual(oldEntry as any, oldEntry as any)).not.toThrow();
  });
});
