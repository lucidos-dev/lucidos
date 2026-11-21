import { API, json } from './_core';

/** Mint the single-use ticket a call presents on its socket (ADR 0598).
 *
 *  A refusal throws `ApiError` carrying the engine's own message, which the
 *  voice client shows as written. */
export async function mintCallTicket(threadId: string): Promise<string> {
  const { ticket } = await json<{ ticket: string; expires_in_secs: number }>(
    `${API}/voice/ticket`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ thread_id: threadId }),
    },
  );
  return ticket;
}
