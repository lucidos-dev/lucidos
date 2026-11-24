import { describe, it, expect, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { panelOverlay } from '../store';
import type { Notification } from '../types';

const sendSeededPrompt = vi.fn(async () => true);
vi.mock('./compose', () => ({ sendSeededPrompt }));

vi.mock('../../utils/formatTime', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/formatTime')>()),
  formatNotificationDate: () => 'Today 09:15',
}));

const { notificationDiscussPrompt, discussNotification } = await import('./notification-discuss');

function notif(over: Partial<Notification> = {}): Notification {
  return {
    id: 'n1',
    title: 'Build failed',
    message: '3 tests are red.',
    read: false,
    created_at: new Date(0).toISOString(),
    ...over,
  };
}

describe('notificationDiscussPrompt', () => {
  it('quotes the notification into a message that stands on its own', () => {
    expect(notificationDiscussPrompt(notif())).toBe(
      "Let's discuss this notification (Today 09:15):\n\n"
      + '> **Build failed**\n>\n> 3 tests are red.',
    );
  });

  it('drops the body block when the notification carries no message', () => {
    expect(notificationDiscussPrompt(notif({ message: '   ' }))).toBe(
      "Let's discuss this notification (Today 09:15):\n\n> **Build failed**",
    );
  });

  it('keeps a multi-paragraph body one quote', () => {
    const prompt = notificationDiscussPrompt(notif({ message: 'Line one.\n\nLine two.' }));
    expect(prompt).toContain('> **Build failed**\n>\n> Line one.\n>\n> Line two.');
  });

  it('names an untitled notification rather than quoting empty bold', () => {
    expect(notificationDiscussPrompt(notif({ title: '' }))).toContain('> **Notification**');
  });
});

describe('discussNotification', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sendSeededPrompt.mockResolvedValue(true);
    panelOverlay.value = null;
  });

  it('sends the seeded message, naming the gesture for the failure toast', async () => {
    const n = notif();
    await discussNotification(n);
    expect(sendSeededPrompt).toHaveBeenCalledWith(
      notificationDiscussPrompt(n),
      'start a discussion about this notification',
    );
  });

  it('leaves the open notification detail where it is', async () => {
    const n = notif();
    const overlay = { type: 'notification-detail', notification: n } as const;
    panelOverlay.value = overlay;
    await discussNotification(n);
    expect(panelOverlay.value).toBe(overlay);
  });

  it('swallows nothing on a declined or failed send: the seam toasts', async () => {
    sendSeededPrompt.mockResolvedValue(false);
    await expect(discussNotification(notif())).resolves.toBeUndefined();
  });

  it('is reachable: the detail wires a button to this action', () => {
    const here: string = dirname(fileURLToPath(import.meta.url));
    const source = readFileSync(
      resolve(here, '../../components/notifications/NotificationDetailInline.tsx'),
      'utf-8',
    );
    expect(source).toContain('void discussNotification(detail!)');
    expect(source).toContain('{...discussHandlers}');
    expect(source).toMatch(/composeHandlers\(\s*\(\) => \{ void discussNotification\(detail!\); \},\s*\(\) => \{\},/);
  });
});
