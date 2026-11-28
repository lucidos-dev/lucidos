import { threadMap } from '../store';
import { PENDING_TITLE_PLACEHOLDER } from '../thread-events';

export function formatThreadLabel(threadId: string): string {
  const title = threadMap.value.get(threadId)?.meta.title;
  return title && title !== PENDING_TITLE_PLACEHOLDER ? `“${title}”` : 'thread';
}
