import { renameThread, suggestTitle } from '../../api/threads';
import { errorDetail } from '../../utils/errorDetail';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { showPrompt, showToast, threadMap } from '../store';
import type { ThreadState } from '../thread-events';

/** The title to send for a rename, or null when there is nothing to send: a
 *  blank value, or one equal to the current title once trimmed. */
export function normalizeRename(newValue: string, currentTitle: string): string | null {
  const trimmed = newValue.trim();
  if (!trimmed || trimmed === currentTitle) return null;
  return trimmed;
}

/** Whether a rename would show. A draft is titled by its compose text, so a
 *  rename there would change nothing on screen. */
export function canRenameThread(thread: ThreadState | undefined): thread is ThreadState {
  return !!thread && thread.meta.state !== 'composing';
}

/** Whether to offer a suggested name. Only the user names the home thread, so
 *  it is renamable but never suggested a name (ADR 0362). */
export function canSuggestThreadName(thread: ThreadState | undefined): thread is ThreadState {
  return canRenameThread(thread) && !thread.meta.home;
}

/** The thread's title, or null when `allowed` rules the thread out. */
function currentTitle(
  threadId: string,
  allowed: (thread: ThreadState | undefined) => thread is ThreadState,
): string | null {
  const thread = threadMap.value.get(threadId);
  return allowed(thread) ? threadDisplayTitle(thread) : null;
}

async function rename(threadId: string, title: string, next: string): Promise<void> {
  try {
    await renameThread(threadId, next);
  } catch (err) {
    showToast(`Could not rename thread "${title}": ${errorDetail(err)}`, 'error');
  }
}

/** Ask for a new name in a dialog prefilled with the current one. Its Suggest
 *  link fills the field with a name from the engine, where one is offered. */
export async function promptRenameThread(threadId: string): Promise<void> {
  const thread = threadMap.value.get(threadId);
  const title = currentTitle(threadId, canRenameThread);
  if (title === null) return;
  const answer = await showPrompt('Give this thread a new name.', {
    title: 'Rename thread',
    defaultValue: title,
    okLabel: 'Rename',
    suggest: canSuggestThreadName(thread) ? () => suggestName(threadId, title) : undefined,
  });
  const next = answer === null ? null : normalizeRename(answer, title);
  if (next !== null) await rename(threadId, title, next);
}

/** A name from the engine, or null when it fails or finds none better. Each
 *  null case says why in a toast. */
async function suggestName(threadId: string, title: string): Promise<string | null> {
  let suggestion: string;
  try {
    suggestion = (await suggestTitle(threadId)).trim();
  } catch (err) {
    showToast(`Could not suggest a name for "${title}": ${errorDetail(err)}`, 'error');
    return null;
  }
  if (!suggestion || suggestion === title) {
    showToast(`"${title}" already fits this thread.`, 'info', { title: 'No better name' });
    return null;
  }
  return suggestion;
}
