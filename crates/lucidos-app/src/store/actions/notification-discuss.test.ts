import { describe, it, expect, beforeEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { panelOverlay } from '../store';
import type { Notification } from '../types';

// `sendSeededPrompt` owns the whole gesture and compose.test.ts covers it: the
// confirm before replacing a draft, the forced Lucidos Agent destination, the
// thread-pane reveal, the send and the failure toast. Here it is a seam. These
// tests are about what the Discuss action hands it.
const sendSeededPrompt = vi.fn(async () => true);
vi.mock('./compose', () => ({ sendSeededPrompt }));

// Pinned so the message is asserted against a clock the test owns, not the one
// the suite happens to run at. Only this one export is replaced: the store pulls
// other formatters from the same module, and a bare factory would strip them.
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
    // Sent as written, so no trailing blank line for the user to type into.
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
    // A blank line inside the body takes a bare `>`. So the quote does not split
    // in two, with the second paragraph falling out of it.
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
    // Discuss starts a conversation beside the inbox. It does not navigate away,
    // so the reader keeps the notification on screen behind the thread.
    const n = notif();
    const overlay = { type: 'notification-detail', notification: n } as const;
    panelOverlay.value = overlay;
    await discussNotification(n);
    expect(panelOverlay.value).toBe(overlay);
  });

  it('swallows nothing on a declined or failed send: the seam toasts', async () => {
    // `sendSeededPrompt` reports every failure itself, so a false return needs
    // no second message here. The action must not throw on it either.
    sendSeededPrompt.mockResolvedValue(false);
    await expect(discussNotification(notif())).resolves.toBeUndefined();
  });

  // Nothing renders `NotificationDetailInline` in a test, so without this the
  // button could be deleted and the suite would stay green. A source scan is
  // what the notifications directory already uses for its other contract.
  it('is reachable: the detail wires a button to this action', () => {
    const here: string = dirname(fileURLToPath(import.meta.url));
    const source = readFileSync(
      resolve(here, '../../components/notifications/NotificationDetailInline.tsx'),
      'utf-8',
    );
    expect(source).toContain('void discussNotification(detail!)');
    expect(source).toContain('{...discussHandlers}');
    // The second argument is the empty focus function: the wrapper is kept for
    // its touch dedup, and Discuss must not raise a keyboard over the reply.
    expect(source).toMatch(/composeHandlers\(\s*\(\) => \{ void discussNotification\(detail!\); \},\s*\(\) => \{\},/);
  });
});
