import { API, json, mutatingFetch, throwIfNotOk } from './_core';

// --- Successor threads ---
//
// GET /threads/:id/context reports a coding-agent thread's size and what it
// calls for. POST /threads/:id/successor starts the thread that carries on.

/** A thread's context size, as the engine measures it. The engine owns the
 *  thresholds and knows which threads can hand off, so callers act on `offer`
 *  and `nudge` and never compare `input_tokens` themselves. */
export interface ThreadContext {
  /** Input tokens of the thread's latest main-session model call. `null`
   *  when it has made none. */
  input_tokens: number | null;
  /** The thread may start a successor at all, at any size. */
  can_hand_off: boolean;
  /** Large enough to offer a new thread after an Apply. */
  offer: boolean;
  /** Large enough to nudge an idle thread to continue in a new one. */
  nudge: boolean;
  /** The thread has an applied change, so a new request can go to a
   *  successor after it. */
  applied: boolean;
}

export async function fetchThreadContext(threadId: string): Promise<ThreadContext> {
  return json<ThreadContext>(`${API}/threads/${encodeURIComponent(threadId)}/context`);
}

export interface SuccessorSpawned {
  successor_thread_id: string;
  /** A markdown link to the successor. */
  link: string;
}

/** Send the user's next message to a new *successor thread*. It wakes
 *  nothing on the old thread. */
export async function sendToSuccessor(threadId: string, message: string): Promise<SuccessorSpawned> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/successor`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ message }),
  });
  await throwIfNotOk(res);
  return res.json();
}
