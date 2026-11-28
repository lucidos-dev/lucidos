import { continueInSuccessor as postContinue, type SuccessorImplementer } from '../../api/client';
import { threadMap } from '../store';
import { draftIsEmpty, getDraft } from '../composeDrafts';
import { requestPromptCaretToEnd } from '../../components/chat/promptValueSync';
import { updateCompose } from './compose';
import { focusThreadOrBootstrap } from './threads';

export async function continueInSuccessor(threadId: string, implementer?: SuccessorImplementer): Promise<void> {
  const message = getDraft(threadId).text.trim() || undefined;
  await postContinue(threadId, message, implementer);
  if (message) updateCompose(threadId, { text: '' });
}

export function latestSuccessor(threadId: string): string | null {
  const seen = new Set([threadId]);
  let latest: string | null = null;
  let link = threadMap.value.get(threadId)?.meta.successor;
  while (link && !seen.has(link.thread_id)) {
    latest = link.thread_id;
    seen.add(latest);
    link = threadMap.value.get(latest)?.meta.successor;
  }
  return latest;
}

export function moveDraftToSuccessor(threadId: string): void {
  const target = latestSuccessor(threadId);
  if (!target) return;
  const moved = getDraft(threadId);
  if (!draftIsEmpty(moved) && threadMap.value.has(target) && draftIsEmpty(getDraft(target))) {
    updateCompose(target, { text: moved.text, image_hashes: moved.image_hashes });
    updateCompose(threadId, { text: '', image_hashes: [] });
  }
  focusThreadOrBootstrap(target);
  requestPromptCaretToEnd();
}
