import type { CodingAgent } from '../api/types';
import type { ThreadState } from '../store/thread-events/thread-meta';

export function callIsOffered(thread: ThreadState | undefined, codingAgent: CodingAgent | null): boolean {
  return codingAgent === null && thread?.meta.home === true;
}
