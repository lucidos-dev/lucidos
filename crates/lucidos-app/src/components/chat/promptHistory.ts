import { modeToInitiator, sortEventsChronologically, type ThreadState } from '../../store/thread-events';


export type HistoryDirection = 'up' | 'down';

interface Browsing {
  index: number;
  stash: string;
  shown: string;
  count: number;
}

export interface HistoryKeypress {
  direction: HistoryDirection;
  text: string;
  selectionStart: number;
  selectionEnd: number;
  modified: boolean;
}

const browsingByThread = new Map<string, Browsing>();

export function sentPrompts(thread: ThreadState | undefined): string[] {
  if (!thread) return [];
  const prompts: string[] = [];
  for (const { event } of sortEventsChronologically(thread.events)) {
    if (event.type !== 'MessageReceived' || modeToInitiator(event.mode) !== 'user') continue;
    if (event.text.trim() === '' || prompts[prompts.length - 1] === event.text) continue;
    prompts.push(event.text);
  }
  return prompts;
}

export function endPromptHistory(threadId: string): void {
  browsingByThread.delete(threadId);
}

function liveBrowsing(threadId: string, history: string[]): Browsing | null {
  const browsing = browsingByThread.get(threadId);
  if (!browsing) return null;
  if (browsing.count === history.length && history[browsing.index] === browsing.shown) return browsing;
  const restarted = { ...browsing, index: history.length, count: history.length };
  browsingByThread.set(threadId, restarted);
  return restarted;
}

function atBoundary(press: HistoryKeypress, browsing: Browsing | null): boolean {
  const { text, selectionStart, selectionEnd, direction } = press;
  if (selectionStart !== selectionEnd) return false;
  const unchanged = browsing !== null && text === browsing.shown;
  if (direction === 'up') {
    return selectionStart === 0 || (unchanged && !text.slice(0, selectionStart).includes('\n'));
  }
  return selectionEnd === text.length || (unchanged && !text.slice(selectionEnd).includes('\n'));
}

export function stepPromptHistory(
  threadId: string,
  press: HistoryKeypress,
  history: string[],
): string | null {
  if (press.modified) return null;
  const browsing = liveBrowsing(threadId, history);
  if (!atBoundary(press, browsing)) return null;

  if (press.direction === 'up') {
    const index = browsing ? browsing.index - 1 : history.length - 1;
    if (index < 0) return null;
    browsingByThread.set(threadId, {
      index,
      stash: browsing ? browsing.stash : press.text,
      shown: history[index],
      count: history.length,
    });
    return history[index];
  }

  if (!browsing) return null;
  if (browsing.index + 1 < history.length) {
    const index = browsing.index + 1;
    browsingByThread.set(threadId, { ...browsing, index, shown: history[index] });
    return history[index];
  }
  browsingByThread.delete(threadId);
  return browsing.stash;
}
