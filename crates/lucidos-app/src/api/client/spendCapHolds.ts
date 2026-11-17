import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** Mirrors the engine's SpendCapHold status snapshot. */
export interface SpendCapHold {
  provider: string;
  resets_at: string | null;
  held_until: string;
  message: string;
}

export function getSpendCapHolds(): Promise<SpendCapHold[]> {
  return json<SpendCapHold[]>(`${API}/models/spend-cap-holds`);
}

export async function clearSpendCapHold(provider: string): Promise<void> {
  const response = await mutatingFetch(`${API}/models/spend-cap-holds/${encodeURIComponent(provider)}/clear`, { method: 'POST' });
  await throwIfNotOk(response);
}
