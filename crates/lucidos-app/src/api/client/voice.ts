import { API, json } from './_core';

export async function mintSocketTicket(threadId: string): Promise<string> {
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
