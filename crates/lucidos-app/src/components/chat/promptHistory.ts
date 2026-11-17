import { modeToInitiator, sortEventsChronologically, type ThreadState } from '../../store/thread-events';

/** Shell-style prompt history for the composer: Up recalls the prompts sent
 *  earlier in this thread, Down walks back to the draft that was being typed.
 *  See *prompt history* in `docs/glossary.md`. */

export type HistoryDirection = 'up' | 'down';

/** One thread's walk through its history.
 *  - `stash`: the draft typed before the first Up.
 *  - `shown`: the entry last written to the box.
 *  - `count`: the history length when the walk began. */
interface Browsing {
  index: number;
  stash: string;
  shown: string;
  count: number;
}

export interface HistoryKeypress {
  direction: HistoryDirection;
  /** The box's value and selection at keydown. */
  text: string;
  selectionStart: number;
  selectionEnd: number;
  /** Meta, Ctrl, Alt or Shift is held. Those keep their native meaning. */
  modified: boolean;
}

const browsingByThread = new Map<string, Browsing>();

/** The user's own sent prompts, oldest first. Agent and engine messages are not
 *  history, and neither are blank texts or an immediate repeat. */
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

/** Leave the walk, so the next Up starts at the newest prompt. Call on send. */
export function endPromptHistory(threadId: string): void {
  browsingByThread.delete(threadId);
}

/** A walk follows the history it began with. When a send or an older page
 *  shifts the entries, it restarts past the newest one but keeps the stash.
 *  Up then recalls the newest prompt, and Down still restores the draft. */
function liveBrowsing(threadId: string, history: string[]): Browsing | null {
  const browsing = browsingByThread.get(threadId);
  if (!browsing) return null;
  if (browsing.count === history.length && history[browsing.index] === browsing.shown) return browsing;
  const restarted = { ...browsing, index: history.length, count: history.length };
  browsingByThread.set(threadId, restarted);
  return restarted;
}

/** Whether the caret sits where this key means "history", not "move the caret".
 *  Up takes the start of the text, Down the end.
 *  A recalled entry left as it was takes the first line (Up) or the last
 *  (Down). A multi-line entry can then still be walked line by line. */
function atBoundary(press: HistoryKeypress, browsing: Browsing | null): boolean {
  const { text, selectionStart, selectionEnd, direction } = press;
  if (selectionStart !== selectionEnd) return false;
  const unchanged = browsing !== null && text === browsing.shown;
  if (direction === 'up') {
    return selectionStart === 0 || (unchanged && !text.slice(0, selectionStart).includes('\n'));
  }
  return selectionEnd === text.length || (unchanged && !text.slice(selectionEnd).includes('\n'));
}

/** The text to put in the box for this keypress, or null to leave the key to
 *  the browser. Moves the thread's walk as a side effect. */
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
