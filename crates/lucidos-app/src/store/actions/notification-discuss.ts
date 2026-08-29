import type { Notification } from '../types';
import { sendSeededPrompt } from './compose';
import { formatNotificationDate } from '../../utils/formatTime';
import { quoteBlock } from '../../utils/markdownQuote';

/** The message the Discuss button sends: a lead-in and the notification quoted.
 *
 *  It is a complete message rather than a lead-in the user finishes, because
 *  Discuss sends it. Quoting the notification is what carries it to the agent,
 *  which has no other way to read the inbox.
 *
 *  Pure, so the shape is testable without a store. */
export function notificationDiscussPrompt(n: Notification): string {
  const when = formatNotificationDate(new Date(n.created_at));
  const title = n.title.trim() || 'Notification';
  const body = n.message.trim();
  const block = body ? `**${title}**\n\n${body}` : `**${title}**`;
  return `Let's discuss this notification (${when}):\n\n${quoteBlock(block)}`;
}

/** Start a conversation about a notification that has no thread of its own.
 *
 *  `sendSeededPrompt` owns the whole gesture: it confirms before replacing a
 *  draft in progress, forces the Lucidos Agent destination, reveals the thread
 *  pane, sends, and toasts on failure. The thread id is allocated client-side,
 *  so there is no submit-then-navigate step: the user is already looking at the
 *  thread when the request goes out.
 *
 *  Never writes `panelOverlay`. The notification detail stays open behind the
 *  conversation, so the reader keeps their place in the inbox. */
export async function discussNotification(n: Notification): Promise<void> {
  await sendSeededPrompt(
    notificationDiscussPrompt(n),
    'start a discussion about this notification',
  );
}
