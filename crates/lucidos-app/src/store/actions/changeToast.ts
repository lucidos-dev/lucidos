import { threadMap } from '../store';

/** Build a change toast message with thread title and description/error.
 *  Lives in its own module (not thread-sync) so both the SSE handlers
 *  (thread-sync), the Apply-Now action (chat-claude-code), and the resume
 *  reconcile (chat-changes) can import it without a thread-sync ↔ chat-changes
 *  import cycle. */
export function changeToastMessage(action: string, threadId: string, detail?: string): string {
  const thread = threadMap.value.get(threadId);
  const title = thread?.meta?.title;
  const parts: string[] = [];
  if (title) parts.push(title);
  if (detail) parts.push(detail);
  if (parts.length === 0) return `${action}.`;
  return `${action}: ${parts.join(' — ')}`;
}
