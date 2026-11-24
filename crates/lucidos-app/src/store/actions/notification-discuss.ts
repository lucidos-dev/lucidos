import type { Notification } from '../types';
import { sendSeededPrompt } from './compose';
import { formatNotificationDate } from '../../utils/formatTime';
import { quoteBlock } from '../../utils/markdownQuote';

export function notificationDiscussPrompt(n: Notification): string {
  const when = formatNotificationDate(new Date(n.created_at));
  const title = n.title.trim() || 'Notification';
  const body = n.message.trim();
  const block = body ? `**${title}**\n\n${body}` : `**${title}**`;
  return `Let's discuss this notification (${when}):\n\n${quoteBlock(block)}`;
}

export async function discussNotification(n: Notification): Promise<void> {
  await sendSeededPrompt(
    notificationDiscussPrompt(n),
    'start a discussion about this notification',
  );
}
