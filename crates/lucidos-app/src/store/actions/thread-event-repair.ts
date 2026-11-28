import { threadMap } from '../store';
import { loadThreadEvents } from './thread-loading';

export function rebuildCorruptedThreadEvents(threadId: string): void {
  const thread = threadMap.value.get(threadId);
  if (!thread) return;
  thread.events = new Map(thread.events);
  thread.eventsLoaded = false;
  thread.lastDbSeq = 0;
  void loadThreadEvents(threadId);
}

