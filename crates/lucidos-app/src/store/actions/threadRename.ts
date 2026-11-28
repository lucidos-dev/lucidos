import { renameThread, suggestTitle } from '../../api/threads';
import { errorDetail } from '../../utils/errorDetail';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { showPrompt, showToast, threadMap } from '../store';
import type { ThreadState } from '../thread-events';

export function normalizeRename(newValue: string, currentTitle: string): string | null {
  const trimmed = newValue.trim();
  if (!trimmed || trimmed === currentTitle) return null;
  return trimmed;
}

export function canRenameThread(thread: ThreadState | undefined): thread is ThreadState {
  return !!thread && thread.meta.state !== 'composing';
}

export function canSuggestThreadName(thread: ThreadState | undefined): thread is ThreadState {
  return canRenameThread(thread) && !thread.meta.home;
}

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
