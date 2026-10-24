import { continueInSuccessor as postContinue, type SuccessorImplementer } from '../../api/client';
import { threadMap } from '../store';
import { draftIsEmpty, getDraft } from '../composeDrafts';
import { requestPromptCaretToEnd } from '../../components/chat/promptValueSync';
import { updateCompose } from './compose';
import { focusThreadOrBootstrap } from './threads';

/** Continue on the *Successor card*, or the handoff a rung switch offers. The
 *  engine picks the path by cache warmth (ADR 0539), and the *successor
 *  session* runs in this same thread, so nothing moves the focus.
 *
 *  A typed message goes along as the session's first request. It leaves the
 *  composer only once the engine took it, so a refused Continue loses nothing. */
export async function continueInSuccessor(threadId: string, implementer?: SuccessorImplementer): Promise<void> {
  const message = getDraft(threadId).text.trim() || undefined;
  await postContinue(threadId, message, implementer);
  if (message) updateCompose(threadId, { text: '' });
}

/** The newest thread in `threadId`'s handoff chain: its successor's
 *  successor, and so on, as far as the loaded threads reach. */
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

/** Move the message typed in a handed-off thread to the newest thread in its
 *  chain, as a draft there, unsent, and focus that thread. The message moves
 *  only into a loaded thread with no draft: a merge could pass the engine's
 *  compose limits, and an unloaded thread may hold a draft never read here.
 *  Otherwise the message stays put and only the focus moves. */
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
