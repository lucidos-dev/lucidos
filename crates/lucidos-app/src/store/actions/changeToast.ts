import { threadMap } from '../store';

export function changeToastMessage(action: string, threadId: string, detail?: string): string {
  const thread = threadMap.value.get(threadId);
  const title = thread?.meta?.title;
  const parts: string[] = [];
  if (title) parts.push(title);
  if (detail) parts.push(detail);
  if (parts.length === 0) return `${action}.`;
  return `${action}: ${parts.join(', ')}`;
}
