import type { CodingAgent } from '../api/types';
import type { ThreadState } from '../store/thread-events/thread-meta';

/** Whether a call may run on this thread. Voice sessions live in the home
 *  thread alone (ADR 0362), and a call reaches the Lucidos Agent only
 *  (ADR 0165). So the compose view, which has no thread yet, offers none. */
export function callIsOffered(thread: ThreadState | undefined, codingAgent: CodingAgent | null): boolean {
  return codingAgent === null && thread?.meta.home === true;
}
